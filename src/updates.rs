use anyhow::{Context, Result, bail, ensure};
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use std::time::Duration;

const RELEASE_API: &str = "https://api.github.com/repos/tarus-eugene/Strata/releases/latest";
const RELEASE_DOWNLOADS: &str = "https://github.com/tarus-eugene/Strata/releases/download";
const MAX_RELEASE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Update {
    pub version: String,
    pub installer_url: String,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
    state: String,
}

/// Performs a blocking check; call from a background worker, never the UI thread.
pub fn check() -> Result<Option<Update>> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let client = Client::builder()
            .https_only(true)
            .user_agent(concat!("101Downloader/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .build()?;
        fetch(&client, RELEASE_API).await
    })
}

async fn fetch(client: &Client, endpoint: &str) -> Result<Option<Update>> {
    let mut response = client
        .get(endpoint)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .context("Could not reach GitHub. Check your connection and try again.")?;
    match response.status() {
        StatusCode::NOT_FOUND => bail!("No public GitHub release is available yet."),
        StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS => {
            bail!("GitHub refused the update check or its rate limit was reached. Try again later.")
        }
        _ => response.error_for_status_ref()?,
    };
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            body.len() + chunk.len() <= MAX_RELEASE_BYTES,
            "GitHub release metadata is too large."
        );
        body.extend_from_slice(&chunk);
    }
    available_release(&body, env!("CARGO_PKG_VERSION"))
}

fn version(value: &str) -> Result<[u64; 3]> {
    let value = value.strip_prefix('v').unwrap_or(value);
    let parts = value.split('.').collect::<Vec<_>>();
    ensure!(parts.len() == 3, "Unsupported release version: {value}");
    let mut numbers = [0; 3];
    for (index, part) in parts.iter().enumerate() {
        ensure!(
            !part.is_empty()
                && part.bytes().all(|b| b.is_ascii_digit())
                && (part.len() == 1 || !part.starts_with('0')),
            "Unsupported release version: {value}"
        );
        numbers[index] = part.parse().context("Release version is out of range")?;
    }
    Ok(numbers)
}

fn available_release(body: &[u8], current: &str) -> Result<Option<Update>> {
    let release: Release =
        serde_json::from_slice(body).context("Invalid GitHub release metadata")?;
    if release.draft || release.prerelease {
        return Ok(None);
    }
    if version(&release.tag_name)? <= version(current)? {
        return Ok(None);
    }
    let version = release
        .tag_name
        .strip_prefix('v')
        .unwrap_or(&release.tag_name);
    let filename = format!("101-Downloader-Setup-{version}-x64.exe");
    let installer_url = format!("{RELEASE_DOWNLOADS}/{}/{filename}", release.tag_name);
    let asset = release.assets.iter().find(|asset| asset.name == filename);
    ensure!(
        asset.is_some_and(|asset| asset.state == "uploaded"
            && asset.size > 0
            && asset.browser_download_url == installer_url),
        "Release {version} does not have a ready Windows x64 installer. Try again later."
    );
    Ok(Some(Update {
        version: version.to_owned(),
        installer_url,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn release(tag: &str) -> Value {
        let version = tag.strip_prefix('v').unwrap_or(tag);
        let filename = format!("101-Downloader-Setup-{version}-x64.exe");
        json!({
            "tag_name": tag, "draft": false, "prerelease": false,
            "assets": [{"name": filename, "state": "uploaded", "size": 123,
                "browser_download_url": format!("{RELEASE_DOWNLOADS}/{tag}/{filename}")}]
        })
    }

    fn available(value: &Value) -> Result<Option<Update>> {
        available_release(&serde_json::to_vec(value).unwrap(), "0.2.1")
    }

    #[test]
    fn selects_newer_installer_with_numeric_version_ordering() {
        for tag in ["v0.2.2", "0.10.0", "v1.0.0"] {
            let update = available(&release(tag)).unwrap().unwrap();
            assert_eq!(update.version, tag.trim_start_matches('v'));
            assert!(update.installer_url.starts_with(RELEASE_DOWNLOADS));
        }
        for tag in ["v0.2.1", "v0.2.0", "v0.1.99"] {
            assert_eq!(available(&release(tag)).unwrap(), None);
        }
    }

    #[test]
    fn skips_drafts_and_prereleases() {
        for field in ["draft", "prerelease"] {
            let mut value = release("v0.3.0-beta.1");
            value[field] = json!(true);
            assert_eq!(available(&value).unwrap(), None);
        }
    }

    #[test]
    fn rejects_missing_unready_and_untrusted_installers() {
        for (field, value) in [
            ("name", json!("101-downloader-browser-extension.zip")),
            ("state", json!("new")),
            ("size", json!(0)),
            (
                "browser_download_url",
                json!("https://example.com/setup.exe"),
            ),
            (
                "browser_download_url",
                json!(format!(
                    "{RELEASE_DOWNLOADS}/v0.2.1/101-Downloader-Setup-0.2.2-x64.exe"
                )),
            ),
        ] {
            let mut body = release("v0.2.2");
            body["assets"][0][field] = value;
            assert!(available(&body).is_err(), "{field}");
        }
        let mut body = release("v0.2.2");
        body["assets"] = json!([]);
        assert!(available(&body).is_err());
    }

    #[test]
    fn rejects_malformed_metadata_and_versions() {
        assert!(available_release(b"not JSON", "0.2.1").is_err());
        for tag in [
            "",
            "v1.0",
            "v01.0.0",
            "v1.0.0.1",
            "v1.0.0-beta",
            "v1.0.+2",
            "v1.0.99999999999999999999999",
        ] {
            assert!(available(&release(tag)).is_err(), "{tag}");
        }
    }

    #[tokio::test]
    async fn checks_http_responses_and_bounds_metadata() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let newer = format!(
            "v{}.0.0",
            version(env!("CARGO_PKG_VERSION")).unwrap()[0] + 1
        );

        for (status, body, expected) in [
            ("200 OK", serde_json::to_vec(&release(&newer)).unwrap(), ""),
            ("404 Not Found", vec![], "No public GitHub release"),
            ("403 Forbidden", vec![], "rate limit"),
            ("429 Too Many Requests", vec![], "rate limit"),
            ("500 Internal Server Error", vec![], "500"),
            ("200 OK", vec![b' '; MAX_RELEASE_BYTES + 1], "too large"),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let endpoint = format!("http://{}", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    assert_eq!(stream.read(&mut byte).unwrap(), 1);
                    request.push(byte[0]);
                }
                assert!(
                    String::from_utf8(request)
                        .unwrap()
                        .to_lowercase()
                        .contains("accept: application/vnd.github+json")
                );
                let header = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream
                    .write_all(header.as_bytes())
                    .and_then(|_| stream.write_all(&body));
            });
            let client = Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap();
            let result = fetch(&client, &endpoint).await;
            server.join().unwrap();
            if expected.is_empty() {
                assert!(result.unwrap().is_some());
            } else {
                assert!(result.unwrap_err().to_string().contains(expected));
            }
        }
    }
}
