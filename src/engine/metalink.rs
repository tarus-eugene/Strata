use super::*;
use md5::Md5;
use sha2::{Digest, Sha256, Sha512};

struct Entry {
    name: PathBuf,
    size: Option<u64>,
    hashes: Vec<(String, String)>,
    mirrors: Vec<String>,
}

fn parse(bytes: &[u8], base: &Url) -> Result<Vec<Entry>> {
    let text = std::str::from_utf8(bytes).context("Metalink must be UTF-8 XML")?;
    let document = roxmltree::Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: 250_000,
        },
    )?;
    let root = document.root_element();
    ensure!(
        root.tag_name().name() == "metalink",
        "This is not a Metalink document"
    );
    let version4 = root.tag_name().namespace() == Some("urn:ietf:params:xml:ns:metalink");
    ensure!(
        version4
            || root.tag_name().namespace() == Some("http://www.metalinker.org/")
            || root
                .attribute("version")
                .is_some_and(|v| v.starts_with("3.")),
        "Expected Metalink v3 or v4"
    );
    let files = if version4 {
        root
    } else {
        root.children()
            .find(|n| n.has_tag_name("files"))
            .context("Metalink v3 has no files section")?
    };
    let mut entries: Vec<Entry> = Vec::new();
    for file in files
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() == "file")
    {
        ensure!(
            entries.len() < 1024,
            "A Metalink may contain at most 1024 files"
        );
        let name = safe_name(
            file.attribute("name")
                .context("Metalink file has no name")?,
        )?;
        let comparable = name.to_string_lossy().to_lowercase();
        ensure!(
            !entries.iter().any(|e| {
                let old = e.name.to_string_lossy().to_lowercase();
                old == comparable
                    || old.starts_with(&(comparable.clone() + "\\"))
                    || comparable.starts_with(&(old + "\\"))
            }),
            "Metalink file names overlap or collide"
        );
        let size = file
            .children()
            .find(|n| n.is_element() && n.tag_name().name() == "size")
            .and_then(|n| n.text())
            .map(str::trim)
            .map(str::parse::<u64>)
            .transpose()?;
        let verification = if version4 {
            file
        } else {
            file.children()
                .find(|n| n.is_element() && n.tag_name().name() == "verification")
                .context("Metalink file has no checksums")?
        };
        let mut hashes = Vec::new();
        for hash in verification
            .children()
            .filter(|n| n.is_element() && n.tag_name().name() == "hash")
        {
            let algorithm = hash
                .attribute("type")
                .unwrap_or("")
                .to_ascii_lowercase()
                .replace('-', "");
            let length = match algorithm.as_str() {
                "sha256" => 64,
                "sha512" => 128,
                "md5" => 32,
                _ => continue,
            };
            let value = hash.text().unwrap_or("").trim().to_ascii_lowercase();
            ensure!(
                value.len() == length && value.bytes().all(|b| b.is_ascii_hexdigit()),
                "Invalid Metalink {algorithm} checksum"
            );
            hashes.push((algorithm, value));
        }
        ensure!(
            !hashes.is_empty(),
            "Every Metalink file needs a whole-file SHA-256, SHA-512, or MD5 checksum"
        );
        let resources = if version4 {
            file
        } else {
            file.children()
                .find(|n| n.is_element() && n.tag_name().name() == "resources")
                .context("Metalink file has no resources")?
        };
        let mut mirrors = Vec::new();
        for node in resources
            .children()
            .filter(|n| n.is_element() && n.tag_name().name() == "url")
        {
            let url = base.join(node.text().unwrap_or("").trim())?;
            if !matches!(
                url.scheme(),
                "http" | "https" | "ftp" | "sftp" | "webdav" | "webdavs"
            ) {
                continue;
            }
            let priority = node
                .attribute("priority")
                .map(str::parse::<u32>)
                .transpose()?
                .unwrap_or_else(|| {
                    100_000_u32.saturating_sub(
                        node.attribute("preference")
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(50),
                    )
                });
            mirrors.push((priority, url.to_string()));
        }
        mirrors.sort_by_key(|(priority, _)| *priority);
        ensure!(
            !mirrors.is_empty(),
            "Metalink file has no supported mirrors"
        );
        entries.push(Entry {
            name,
            size,
            hashes,
            mirrors: mirrors.into_iter().map(|(_, url)| url).collect(),
        });
    }
    ensure!(!entries.is_empty(), "Metalink contains no files");
    Ok(entries)
}

fn safe_name(name: &str) -> Result<PathBuf> {
    let normalized = name.replace('\\', "/");
    let mut path = PathBuf::new();
    for part in normalized.split('/') {
        ensure!(
            !part.is_empty() && part != "." && part != ".." && sanitize_filename(part) == part,
            "Unsafe Metalink file name"
        );
        path.push(part);
    }
    ensure!(!path.is_absolute(), "Metalink file names must be relative");
    Ok(path)
}

async fn verify(path: &Path, entry: &Entry, job: &Arc<Job>) -> Result<()> {
    ensure!(
        entry
            .size
            .is_none_or(|n| fs::metadata(path).is_ok_and(|m| m.len() == n)),
        "Metalink file size mismatch"
    );
    let mut file = tokio::fs::File::open(path).await?;
    let (mut sha256, mut sha512, mut md5) = (Sha256::new(), Sha512::new(), Md5::new());
    let mut buffer = vec![0; 128 * 1024];
    loop {
        check_running(job)?;
        let count = tokio::io::AsyncReadExt::read(&mut file, &mut buffer).await?;
        if count == 0 {
            break;
        }
        sha256.update(&buffer[..count]);
        sha512.update(&buffer[..count]);
        md5.update(&buffer[..count]);
    }
    let actual = [
        ("sha256", format!("{:x}", sha256.finalize())),
        ("sha512", format!("{:x}", sha512.finalize())),
        ("md5", format!("{:x}", md5.finalize())),
    ];
    for (algorithm, expected) in &entry.hashes {
        ensure!(
            actual
                .iter()
                .any(|(kind, value)| kind == algorithm && value == expected),
            "Metalink {algorithm} checksum mismatch"
        );
    }
    Ok(())
}

impl Manager {
    pub(super) async fn execute_metalink(&self, job: &Arc<Job>) -> Result<()> {
        let parent = job.data.lock().unwrap().clone();
        let entries = parse(&self.descriptor(job).await?, &Url::parse(&parent.url)?)?;
        let root = parent.parts_dir();
        let content = root.join("content");
        fs::create_dir_all(&content)?;
        let total = entries
            .iter()
            .try_fold(0_u64, |sum, e| sum.checked_add(e.size?));
        {
            let mut data = job.data.lock().unwrap();
            data.total = total;
            data.downloaded = 0;
            data.status = Status::Downloading;
        }
        let mut completed = 0_u64;
        for (index, entry) in entries.iter().enumerate() {
            check_running(job)?;
            let destination = content.join(&entry.name);
            if destination.is_file() && verify(&destination, entry, job).await.is_ok() {
                completed += fs::metadata(&destination)?.len();
                continue;
            }
            if destination.exists() {
                fs::remove_file(&destination)?;
            }
            let work = root.join(format!("entry-{index}"));
            fs::create_dir_all(&work)?;
            let sidecar = work.join("state.json");
            let mut last_error = None;
            let mut accepted = false;
            for mirror in &entry.mirrors {
                check_running(job)?;
                if parent.options.proxy.is_some()
                    && matches!(Url::parse(mirror)?.scheme(), "ftp" | "sftp")
                {
                    last_error = Some(anyhow::anyhow!(
                        "This mirror cannot use the configured HTTP proxy"
                    ));
                    continue;
                }
                let mut options = TransferOptions {
                    proxy: parent.options.proxy.clone(),
                    identity: parent.options.identity.clone(),
                    known_hosts: parent.options.known_hosts.clone(),
                    ..Default::default()
                };
                if Url::parse(mirror)?.origin() == Url::parse(&parent.url)?.origin() {
                    options.username = parent.options.username.clone();
                    options.password = parent.options.password.clone();
                }
                let source = Source::parse(mirror, options)?;
                let mut data = fs::read(&sidecar)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<Download>(&bytes).ok())
                    .filter(|d| d.url == source.url.as_str())
                    .unwrap_or_else(|| Download {
                        id: parent.id,
                        url: source.url.to_string(),
                        name: "payload".into(),
                        directory: work.clone(),
                        total: None,
                        downloaded: 0,
                        status: Status::Connecting,
                        parts: vec![],
                        validator: None,
                        ranged: false,
                        error: None,
                        connections: parent.connections,
                        created: parent.created,
                        completed_at: None,
                        completion_action: None,
                        referrer: None,
                        protocol: source.protocol,
                        options: source.options.clone(),
                        preserve_name: true,
                        owns_output: false,
                        priority: parent.priority,
                        speed_limit: parent.speed_limit,
                        torrent_files: Vec::new(),
                        selected_files: None,
                        integrity: Vec::new(),
                        integrity_baseline_only: false,
                    });
                data.id = parent.id;
                data.directory = work.clone();
                data.name = "payload".into();
                data.preserve_name = true;
                data.owns_output = false;
                data.protocol = source.protocol;
                data.options = source.options;
                data.speed_limit = parent.speed_limit;
                if data.path().exists() {
                    fs::remove_file(data.path())?;
                }
                let child = Arc::new(Job {
                    data: Mutex::new(data),
                    control: job.control.clone(),
                    running: AtomicBool::new(true),
                    name_hint: Mutex::new(None),
                    deleting: AtomicBool::new(false),
                    editing: AtomicBool::new(false),
                    limiter: job.limiter.clone(),
                    global_limiter: job.global_limiter.clone(),
                });
                let result = {
                    let transfer = async {
                        match source.protocol {
                            Protocol::Http | Protocol::WebDav => self.execute_http(&child).await,
                            Protocol::Ftp => self.execute_ftp(&child).await,
                            Protocol::Sftp => self.execute_sftp(&child).await,
                            _ => bail!("A Metalink mirror must be a direct file transfer"),
                        }
                    };
                    tokio::pin!(transfer);
                    let mut interval = tokio::time::interval(Duration::from_millis(200));
                    loop {
                        tokio::select! {
                            result = &mut transfer => break result,
                            _ = interval.tick() => {
                                let current = child.data.lock().unwrap().clone();
                                let mut data = job.data.lock().unwrap(); data.downloaded = completed + current.downloaded; data.parts = current.parts.clone();
                                fs::write(&sidecar, serde_json::to_vec(&current)?)?;
                            }
                        }
                    }
                };
                let current = child.data.lock().unwrap().clone();
                fs::write(&sidecar, serde_json::to_vec(&current)?)?;
                let result = match result {
                    Ok(()) => verify(&current.path(), entry, job).await,
                    Err(error) => Err(error),
                };
                if let Err(error) = result {
                    last_error = Some(error);
                    continue;
                }
                fs::create_dir_all(destination.parent().unwrap())?;
                fs::rename(current.path(), &destination)?;
                completed += fs::metadata(&destination)?.len();
                accepted = true;
                break;
            }
            ensure!(
                accepted,
                "Every mirror failed for {}: {}",
                entry.name.display(),
                last_error.map_or_else(|| "No available mirror".into(), |e| format!("{e:#}"))
            );
            job.data.lock().unwrap().downloaded = completed;
        }
        tools::publish_directory(job, &content).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_v3_v4_hashes_mirrors_and_rejects_traversal() {
        let hash = "0".repeat(64);
        let v4 = format!(
            r#"<metalink xmlns="urn:ietf:params:xml:ns:metalink"><file name="folder/a"><size>4</size><hash type="sha-256">{hash}</hash><url priority="2">https://second/a</url><url priority="1">https://first/a</url></file></metalink>"#
        );
        let base = Url::parse("https://example.com/a.meta4").unwrap();
        let files = parse(v4.as_bytes(), &base).unwrap();
        assert_eq!(files[0].mirrors[0], "https://first/a");
        assert_eq!(files[0].size, Some(4));
        let v3 = format!(
            r#"<metalink version="3.0" xmlns="http://www.metalinker.org/"><files><file name="a"><verification><hash type="sha256">{hash}</hash></verification><resources><url preference="100">https://first/a</url></resources></file></files></metalink>"#
        );
        assert_eq!(parse(v3.as_bytes(), &base).unwrap().len(), 1);
        for name in [
            "../escape",
            "C:/escape",
            "/escape",
            "a/../../escape",
            "a/CON",
            "a/.",
        ] {
            assert!(safe_name(name).is_err());
        }
        assert!(parse(v4.replace("folder/a", "../escape").as_bytes(), &base).is_err());
        assert!(
            parse(
                b"<!DOCTYPE metalink [<!ENTITY x SYSTEM 'file:///secret'>]><metalink/>",
                &base
            )
            .is_err()
        );
    }
}
