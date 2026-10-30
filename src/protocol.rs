use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use url::Url;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
pub enum Protocol {
    #[default]
    Http,
    Ftp,
    Sftp,
    WebDav,
    Torrent,
    Metalink,
    Video,
}

impl Protocol {
    pub fn label(self) -> &'static str {
        match self {
            Self::Http => "HTTP / HTTPS",
            Self::Ftp => "FTP",
            Self::Sftp => "SFTP",
            Self::WebDav => "WebDAV",
            Self::Torrent => "BitTorrent",
            Self::Metalink => "Metalink",
            Self::Video => "Video · yt-dlp",
        }
    }
    pub fn folder_output(self) -> bool {
        matches!(self, Self::Torrent | Self::Metalink | Self::Video)
    }
}

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TransferOptions {
    pub protocol: Option<Protocol>,
    pub proxy: Option<Proxy>,
    pub username: Option<String>,
    pub password: Option<Secret>,
    pub identity: Option<PathBuf>,
    pub key_passphrase: Option<Secret>,
    pub known_hosts: Option<PathBuf>,
}

impl TransferOptions {
    pub fn with_defaults(mut self, defaults: &Self) -> Self {
        self.proxy = self.proxy.or_else(|| defaults.proxy.clone());
        self.identity = self.identity.or_else(|| defaults.identity.clone());
        self.known_hosts = self.known_hosts.or_else(|| defaults.known_hosts.clone());
        self
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Proxy {
    pub url: String,
    pub username: Option<String>,
    pub password: Option<Secret>,
}

impl Proxy {
    pub fn parse(input: &str) -> Result<Self> {
        let mut url = Url::parse(input)
            .context("Enter an http://, https://, socks5:// or socks5h:// proxy URL")?;
        ensure!(
            matches!(url.scheme(), "http" | "https" | "socks5" | "socks5h")
                && url.host_str().is_some(),
            "Unsupported proxy URL"
        );
        ensure!(
            url.query().is_none() && url.fragment().is_none() && matches!(url.path(), "" | "/"),
            "A proxy URL must not contain a path, query, or fragment"
        );
        let (username, password) = credentials(&mut url)?;
        Ok(Self {
            url: url.to_string(),
            username,
            password,
        })
    }
    pub(crate) fn connection_url(&self) -> Result<Url> {
        let mut url = Url::parse(&self.url)?;
        if let Some(user) = &self.username {
            url.set_username(user)
                .map_err(|_| anyhow::anyhow!("Invalid proxy username"))?;
        }
        if let Some(password) = &self.password {
            url.set_password(Some(&password.expose()?))
                .map_err(|_| anyhow::anyhow!("Invalid proxy password"))?;
        }
        Ok(url)
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Secret(Vec<u8>);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[protected]")
    }
}

impl Secret {
    pub fn new(value: &str) -> Result<Self> {
        Ok(Self(protect(value.as_bytes(), false)?))
    }
    pub fn expose(&self) -> Result<String> {
        String::from_utf8(protect(&self.0, true)?)
            .context("Saved credentials are invalid; enter them again")
    }
    pub(crate) fn redact(&self, message: &mut String) -> Result<()> {
        let value = self.expose()?;
        if !value.is_empty() {
            *message = message.replace(&value, "[redacted]");
            let mut encoded = Url::parse("https://redacted@localhost/")?;
            encoded
                .set_password(Some(&value))
                .map_err(|_| anyhow::anyhow!("Cannot encode credential"))?;
            if let Some(password) = encoded.password() {
                *message = message.replace(password, "[redacted]");
            }
        }
        Ok(())
    }
}

#[cfg(windows)]
fn protect(bytes: &[u8], decrypt: bool) -> Result<Vec<u8>> {
    use windows::Win32::{
        Foundation::{HLOCAL, LocalFree},
        Security::Cryptography::*,
    };
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len().try_into()?,
        pbData: bytes.as_ptr() as *mut _,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        if decrypt {
            CryptUnprotectData(
                &input,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )?;
        } else {
            CryptProtectData(
                &input,
                windows::core::w!("101 Downloader credentials"),
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )?;
        }
        let result = if output.cbData == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec()
        };
        let _ = LocalFree(Some(HLOCAL(output.pbData.cast())));
        Ok(result)
    }
}

#[cfg(not(windows))]
fn protect(_: &[u8], _: bool) -> Result<Vec<u8>> {
    bail!("Credential protection currently requires Windows")
}

pub struct Source {
    pub url: Url,
    pub protocol: Protocol,
    pub options: TransferOptions,
}

impl Source {
    pub fn parse(input: &str, mut options: TransferOptions) -> Result<Self> {
        let input = input.trim();
        ensure!(
            !input.is_empty() && input.len() <= 64 * 1024 && !input.chars().any(char::is_control),
            "Enter a download URL, magnet link, torrent, or Metalink file"
        );
        let mut url = if Path::new(input).is_file() {
            Url::from_file_path(std::fs::canonicalize(input)?)
                .map_err(|_| anyhow::anyhow!("Invalid local file path"))?
        } else {
            Url::parse(input).context(
                "Enter a full download URL or an existing .torrent / .meta4 / .metalink file",
            )?
        };
        let extension = url
            .path()
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let detected = match url.scheme() {
            "magnet" => Protocol::Torrent,
            "ftp" => Protocol::Ftp,
            "sftp" => Protocol::Sftp,
            "webdav" | "webdavs" => Protocol::WebDav,
            "http" | "https" | "file" => match extension.as_str() {
                "torrent" => Protocol::Torrent,
                "meta4" | "metalink" => Protocol::Metalink,
                _ if url.scheme() != "file" && video_host(url.host_str().unwrap_or("")) => {
                    Protocol::Video
                }
                _ => Protocol::Http,
            },
            _ => bail!(
                "Unsupported protocol. Use HTTP(S), FTP, SFTP, WebDAV, a magnet, torrent, or Metalink."
            ),
        };
        let protocol = options.protocol.unwrap_or(detected);
        if url.scheme() == "file" {
            ensure!(
                matches!(protocol, Protocol::Torrent | Protocol::Metalink),
                "Only local torrent and Metalink descriptor files are accepted"
            );
            ensure!(
                url.to_file_path().ok().is_some_and(|p| p.is_file()),
                "The descriptor file does not exist"
            );
        } else if url.scheme() == "magnet" {
            ensure!(
                protocol == Protocol::Torrent
                    && url.query_pairs().any(|(k, v)| k == "xt"
                        && (v.starts_with("urn:btih:") || v.starts_with("urn:btmh:"))),
                "Invalid BitTorrent magnet link"
            );
        } else {
            ensure!(url.host_str().is_some(), "The source needs a host name");
        }
        match protocol {
            Protocol::Ftp => ensure!(url.scheme() == "ftp", "FTP requires an ftp:// URL"),
            Protocol::Sftp => ensure!(url.scheme() == "sftp", "SFTP requires an sftp:// URL"),
            Protocol::Http | Protocol::WebDav | Protocol::Video => ensure!(
                matches!(url.scheme(), "http" | "https" | "webdav" | "webdavs"),
                "This protocol requires an HTTP(S) or WebDAV URL"
            ),
            Protocol::Metalink => ensure!(
                matches!(url.scheme(), "http" | "https" | "file"),
                "Metalink descriptors must be local files or HTTP(S) URLs"
            ),
            Protocol::Torrent => ensure!(
                matches!(url.scheme(), "http" | "https" | "file" | "magnet"),
                "Torrents must be local files, HTTP(S) URLs, or magnet links"
            ),
        }
        if matches!(protocol, Protocol::Ftp | Protocol::Sftp) {
            for value in [Some(url.path()), url.query()].into_iter().flatten() {
                ensure!(
                    !percent_encoding::percent_decode_str(value)
                        .any(|byte| byte.is_ascii_control()),
                    "Remote paths must not contain control characters"
                );
            }
        }
        #[cfg(not(feature = "torrent-native"))]
        ensure!(
            protocol != Protocol::Torrent,
            "This build has no torrent support. Build with --features torrent-native."
        );
        let (username, password) = credentials(&mut url)?;
        options.username = username.or(options.username);
        options.password = password.or(options.password);
        if let Some(username) = &options.username {
            ensure!(
                !username.contains(':') && !username.chars().any(char::is_control),
                "Usernames must not contain colons or control characters"
            );
        }
        if let Some(password) = &options.password {
            ensure!(
                !password.expose()?.chars().any(char::is_control),
                "Passwords must not contain control characters"
            );
        }
        if matches!(url.scheme(), "webdav" | "webdavs") {
            let scheme = if url.scheme() == "webdavs" {
                "https"
            } else {
                "http"
            };
            // url::Url cannot change a non-special scheme into HTTP in place.
            url = Url::parse(&format!(
                "{scheme}:{}",
                &url.as_str()[url.scheme().len() + 1..]
            ))?;
        }
        if url.scheme() != "magnet" {
            url.set_fragment(None);
        }
        Ok(Self {
            url,
            protocol,
            options,
        })
    }
}

fn credentials(url: &mut Url) -> Result<(Option<String>, Option<Secret>)> {
    let decode = |s: &str| -> Result<String> {
        let decoded = percent_encoding::percent_decode_str(s)
            .decode_utf8()?
            .into_owned();
        ensure!(
            !decoded.chars().any(char::is_control),
            "Credentials must not contain control characters"
        );
        Ok(decoded)
    };
    let username = (!url.username().is_empty())
        .then(|| decode(url.username()))
        .transpose()?;
    let password = url
        .password()
        .map(decode)
        .transpose()?
        .map(|p| Secret::new(&p))
        .transpose()?;
    if username.is_some() || password.is_some() {
        url.set_password(None)
            .map_err(|_| anyhow::anyhow!("This URL cannot contain credentials"))?;
        url.set_username("")
            .map_err(|_| anyhow::anyhow!("This URL cannot contain credentials"))?;
    }
    Ok((username, password))
}

fn video_host(host: &str) -> bool {
    [
        "youtube.com",
        "youtu.be",
        "vimeo.com",
        "twitch.tv",
        "tiktok.com",
        "instagram.com",
    ]
    .iter()
    .any(|site| host == *site || host.ends_with(&format!(".{site}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn routes_protocols_and_rejects_unsafe_sources() {
        for (url, expected) in [
            ("https://example.com/a.bin", Protocol::Http),
            ("ftp://example.com/a", Protocol::Ftp),
            ("sftp://user@example.com/a", Protocol::Sftp),
            ("webdav://example.com/a", Protocol::WebDav),
            ("https://example.com/a.meta4?token=x", Protocol::Metalink),
            ("https://example.com/a.torrent", Protocol::Torrent),
            ("https://www.youtube.com/watch?v=a", Protocol::Video),
            ("https://youtube.com.attacker.example/a", Protocol::Http),
        ] {
            if expected == Protocol::Torrent && !cfg!(feature = "torrent-native") {
                assert!(Source::parse(url, TransferOptions::default()).is_err());
                continue;
            }
            assert_eq!(
                Source::parse(url, TransferOptions::default())
                    .unwrap()
                    .protocol,
                expected
            );
        }
        for url in [
            "file:///C:/Windows/win.ini",
            "javascript:alert(1)",
            "magnet:?dn=empty",
            "ftp://host/a\n--output=bad",
            "ftp://host/a%0d%0aDELE%20file",
        ] {
            assert!(Source::parse(url, TransferOptions::default()).is_err());
        }
    }
    #[test]
    fn credentials_are_protected_and_webdav_is_normalized() {
        let source = Source::parse(
            "webdav://alice:secret%20password@example.com/a",
            TransferOptions::default(),
        )
        .unwrap();
        assert_eq!(source.url.as_str(), "http://example.com/a");
        assert_eq!(
            source.options.password.as_ref().unwrap().expose().unwrap(),
            "secret password"
        );
        let saved = serde_json::to_string(&source.options).unwrap();
        assert!(!saved.contains("secret password"));
        let restored: TransferOptions = serde_json::from_str(&saved).unwrap();
        assert_eq!(
            restored.password.unwrap().expose().unwrap(),
            "secret password"
        );
        let proxy = Proxy::parse("socks5h://bob:proxy-secret@localhost:1080").unwrap();
        assert!(
            !serde_json::to_string(&proxy)
                .unwrap()
                .contains("proxy-secret")
        );
        assert_eq!(
            proxy.connection_url().unwrap().password(),
            Some("proxy-secret")
        );
        let mut diagnostic = "secret password / secret%20password".to_owned();
        Secret::new("secret password")
            .unwrap()
            .redact(&mut diagnostic)
            .unwrap();
        assert_eq!(diagnostic, "[redacted] / [redacted]");
    }
}
