use crate::{
    bridge::Request,
    protocol::{Protocol, Proxy, Secret, Source, TransferOptions},
};
use anyhow::{Context, Result, ensure};
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "strata-cli", version, about = "Queue a transfer in the Strata desktop app",
    group(clap::ArgGroup::new("protocol").args(["ftp", "sftp", "webdav", "metalink", "ytdlp"]).multiple(false)))]
pub struct Cli {
    /// URL, magnet link, or local .torrent / .meta4 / .metalink descriptor
    pub source: String,
    #[arg(long)]
    pub ftp: bool,
    #[arg(long)]
    pub sftp: bool,
    #[arg(long)]
    pub webdav: bool,
    #[arg(long)]
    pub metalink: bool,
    #[arg(long)]
    pub ytdlp: bool,
    /// HTTP(S) or SOCKS5 proxy URL; credentials are protected in saved history
    #[arg(long)]
    pub proxy: Option<String>,
    #[arg(long, short = 'u')]
    pub user: Option<String>,
    /// Read a password from standard input instead of putting it in the command line
    #[arg(long)]
    pub password_stdin: bool,
    /// OpenSSH private key for SFTP (otherwise try ~/.ssh/id_ed25519 and id_rsa)
    #[arg(long)]
    pub identity: Option<PathBuf>,
    #[arg(long)]
    pub key_passphrase_stdin: bool,
    /// OpenSSH known_hosts file; unknown or changed keys are refused
    #[arg(long)]
    pub known_hosts: Option<PathBuf>,
    #[arg(long, short = 'd')]
    pub directory: Option<PathBuf>,
    #[arg(long, short = 'c', value_parser = clap::value_parser!(u8).range(1..=16))]
    pub connections: Option<u8>,
}

impl Cli {
    pub fn request(self) -> Result<Request> {
        let protocol = if self.ftp {
            Some(Protocol::Ftp)
        } else if self.sftp {
            Some(Protocol::Sftp)
        } else if self.webdav {
            Some(Protocol::WebDav)
        } else if self.metalink {
            Some(Protocol::Metalink)
        } else if self.ytdlp {
            Some(Protocol::Video)
        } else {
            None
        };
        let password = self.password_stdin.then(read_secret).transpose()?;
        let key_passphrase = self.key_passphrase_stdin.then(read_secret).transpose()?;
        let options = TransferOptions {
            protocol,
            proxy: self.proxy.as_deref().map(Proxy::parse).transpose()?,
            username: self.user,
            password,
            identity: self.identity.map(std::path::absolute).transpose()?,
            known_hosts: self.known_hosts.map(std::path::absolute).transpose()?,
            key_passphrase,
        };
        let mut input = self.source;
        if !input.contains("://")
            && !std::path::Path::new(&input).is_file()
            && let Some(scheme) = match protocol {
                Some(Protocol::Ftp) => Some("ftp"),
                Some(Protocol::Sftp) => Some("sftp"),
                Some(Protocol::WebDav) => Some("webdav"),
                _ => None,
            }
        {
            input = format!("{scheme}://{input}");
        }
        let source = Source::parse(&input, options)?;
        Ok(Request::Transfer {
            source: source.url.to_string(),
            directory: self.directory.map(std::path::absolute).transpose()?,
            connections: self.connections.map(usize::from),
            options: Box::new(TransferOptions {
                protocol: Some(source.protocol),
                ..source.options
            }),
        })
    }
}

fn read_secret() -> Result<Secret> {
    let mut value = String::new();
    std::io::stdin()
        .read_line(&mut value)
        .context("Cannot read a password from standard input")?;
    while value.ends_with(['\r', '\n']) {
        value.pop();
    }
    ensure!(
        !value.is_empty(),
        "No password was supplied on standard input"
    );
    Secret::new(&value)
}

pub fn enqueue(cli: Cli) -> Result<u64> {
    let request = cli.request()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let reply = runtime.block_on(crate::bridge::send(
        &request,
        &crate::engine::history_path(),
        true,
    ))?;
    ensure!(
        reply.ok,
        "{}",
        reply
            .error
            .unwrap_or_else(|| "The app rejected this transfer".into())
    );
    reply.id.context("The app did not return a transfer ID")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_protocol_flags_and_rejects_conflicts() {
        let cli =
            Cli::try_parse_from(["strata-cli", "--ftp", "example.com/a", "--connections", "4"])
                .unwrap();
        let Request::Transfer {
            source, options, ..
        } = cli.request().unwrap()
        else {
            panic!()
        };
        assert_eq!(source, "ftp://example.com/a");
        assert_eq!(options.protocol, Some(Protocol::Ftp));
        assert!(Cli::try_parse_from(["strata-cli", "--sftp", "--ftp", "ftp://host/a"]).is_err());
        assert!(
            Cli::try_parse_from(["strata-cli", "https://host/a", "--connections", "0"]).is_err()
        );
        let request = Cli::try_parse_from([
            "strata-cli",
            "--sftp",
            "alice@host/a",
            "--directory",
            ".",
            "--identity",
            "id_ed25519",
        ])
        .unwrap()
        .request()
        .unwrap();
        let Request::Transfer {
            directory, options, ..
        } = request
        else {
            panic!()
        };
        assert!(directory.unwrap().is_absolute());
        assert!(options.identity.unwrap().is_absolute());
    }
}
