use super::*;
use russh::{
    client,
    keys::{self, PrivateKeyWithHashAlg, PublicKeyOrCertificate},
};
use russh_sftp::client::SftpSession;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

struct HostKey {
    host: String,
    port: u16,
    known_hosts: PathBuf,
}

impl client::Handler for HostKey {
    type Error = anyhow::Error;
    async fn check_server_key(&mut self, presented: &PublicKeyOrCertificate) -> Result<bool> {
        let key = presented.public_key();
        let text = fs::read_to_string(&self.known_hosts).unwrap_or_default();
        for line in text.lines() {
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() >= 4
                && fields[0] == "@revoked"
                && keys::parse_public_key_base64(fields[3]).is_ok_and(|revoked| revoked == key)
            {
                bail!("The SFTP server's host key is revoked");
            }
        }
        let trusted = keys::check_known_hosts_path(&self.host, self.port, &key, &self.known_hosts)
            .context("SFTP host key changed; verify the server before updating known_hosts")?;
        ensure!(
            trusted,
            "Untrusted SFTP host key {}. Verify this fingerprint independently and add the server to {} before retrying.",
            key.fingerprint(keys::HashAlg::Sha256),
            self.known_hosts.display()
        );
        Ok(true)
    }
}

impl Manager {
    pub(super) async fn execute_sftp(&self, job: &Arc<Job>) -> Result<()> {
        let d = job.data.lock().unwrap().clone();
        let url = Url::parse(&d.url)?;
        let host = url
            .host_str()
            .context("SFTP host is missing")?
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_owned();
        let port = url.port().unwrap_or(22);
        let home = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .context("Cannot locate your SSH configuration folder")?;
        let known_hosts = d
            .options
            .known_hosts
            .clone()
            .unwrap_or_else(|| home.join(".ssh/known_hosts"));
        let username = d
            .options
            .username
            .clone()
            .or_else(|| std::env::var("USERNAME").ok())
            .context("Specify an SFTP username in the URL or --user")?;
        let handler = HostKey {
            host: host.clone(),
            port,
            known_hosts,
        };
        let config = Arc::new(client::Config {
            inactivity_timeout: Some(Duration::from_secs(45)),
            ..Default::default()
        });
        let mut session = tokio::select! {
            result = tokio::time::timeout(Duration::from_secs(30), client::connect(config, (host.as_str(), port), handler)) => result.context("SFTP connection timed out")??,
            _ = stopped(job) => bail!("Transfer stopped"),
        };
        let authentication = async {
            let passphrase = d
                .options
                .key_passphrase
                .as_ref()
                .map(|s| s.expose())
                .transpose()?;
            let identities = d
                .options
                .identity
                .clone()
                .map(|p| vec![p])
                .unwrap_or_else(|| vec![home.join(".ssh/id_ed25519"), home.join(".ssh/id_rsa")]);
            for path in identities {
                if !path.is_file() {
                    ensure!(
                        d.options.identity.is_none(),
                        "SFTP private key does not exist"
                    );
                    continue;
                }
                ensure!(
                    fs::metadata(&path)?.len() <= 1024 * 1024,
                    "SFTP private key file is too large"
                );
                let key = match keys::load_secret_key(&path, passphrase.as_deref()) {
                    Ok(key) => key,
                    Err(error) if d.options.identity.is_some() => return Err(error).context(
                        "Cannot read the SFTP private key; an encrypted key needs its passphrase",
                    ),
                    Err(_) => continue,
                };
                let hash = session.best_supported_rsa_hash().await?.flatten();
                if session
                    .authenticate_publickey(
                        &username,
                        PrivateKeyWithHashAlg::new(Arc::new(key), hash),
                    )
                    .await?
                    .success()
                {
                    return Ok(());
                }
            }
            if let Some(password) = &d.options.password
                && session
                    .authenticate_password(&username, password.expose()?)
                    .await?
                    .success()
            {
                return Ok(());
            }
            bail!(
                "SFTP authentication failed. Supply a password in the URL / --password-stdin, or choose a valid SSH private key."
            )
        };
        tokio::select! { result = tokio::time::timeout(Duration::from_secs(45), authentication) => result.context("SFTP authentication timed out")??, _ = stopped(job) => bail!("Transfer stopped") }
        let channel = tokio::select! { r = session.channel_open_session() => r?, _ = stopped(job) => bail!("Transfer stopped") };
        channel.request_subsystem(true, "sftp").await?;
        let sftp = SftpSession::new(channel.into_stream()).await?;
        sftp.set_timeout(30);
        let remote_path = percent_encoding::percent_decode_str(url.path())
            .decode_utf8()?
            .into_owned();
        ensure!(!remote_path.contains('\0'), "Invalid SFTP path");
        let metadata = sftp.metadata(&remote_path).await?;
        ensure!(
            !metadata.is_dir(),
            "SFTP source is a directory; choose a file"
        );
        let size = metadata
            .size
            .context("SFTP server did not return the file size")?;
        let validator = metadata.mtime.map(|time| format!("{size}:{time}"));
        let path = tools::begin_file(job, Some(size), validator)?;
        let mut position = fs::metadata(&path)?.len();
        let mut remote = sftp.open(&remote_path).await?;
        remote.seek(std::io::SeekFrom::Start(position)).await?;
        let mut local = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .await?;
        let mut buffer = vec![0_u8; 128 * 1024];
        loop {
            let count = tokio::select! { r = remote.read(&mut buffer) => r?, _ = stopped(job) => { local.flush().await?; bail!("Transfer stopped"); } };
            if count == 0 {
                break;
            }
            ensure!(
                position + count as u64 <= size,
                "The SFTP file grew during download"
            );
            if let Err(error) = job.throttle(count).await {
                local.flush().await?;
                return Err(error);
            }
            local.write_all(&buffer[..count]).await?;
            position += count as u64;
            tools::update_file_progress(job, position);
        }
        local.flush().await?;
        local.sync_all().await?;
        drop(local);
        drop(remote);
        let after = sftp.metadata(&remote_path).await?;
        ensure!(
            after.size == Some(size) && after.mtime == metadata.mtime,
            "The SFTP file changed during download; retry to restart safely"
        );
        let _ = sftp.close().await;
        let _ = session
            .disconnect(russh::Disconnect::ByApplication, "Download finished", "en")
            .await;
        tools::finish_file(job, &path).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh::client::Handler;
    #[tokio::test]
    async fn unknown_changed_and_revoked_host_keys_are_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("known_hosts");
        let key =
            keys::PrivateKey::from(keys::ssh_key::private::Ed25519Keypair::from_seed(&[7; 32]));
        let presented = PublicKeyOrCertificate::from(key.public_key().clone());
        let mut handler = HostKey {
            host: "localhost".into(),
            port: 2222,
            known_hosts: path.clone(),
        };
        assert!(handler.check_server_key(&presented).await.is_err());
        keys::known_hosts::learn_known_hosts_path("localhost", 2222, key.public_key(), &path)
            .unwrap();
        assert!(handler.check_server_key(&presented).await.unwrap());
        let other =
            keys::PrivateKey::from(keys::ssh_key::private::Ed25519Keypair::from_seed(&[8; 32]));
        assert!(
            handler
                .check_server_key(&PublicKeyOrCertificate::from(other.public_key().clone()))
                .await
                .is_err()
        );
        let line = fs::read_to_string(&path).unwrap();
        fs::write(&path, format!("{line}\n@revoked\t{}", line.trim())).unwrap();
        assert!(handler.check_server_key(&presented).await.is_err());
    }
}
