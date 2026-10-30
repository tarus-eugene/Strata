use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use strata::engine::{Manager, Status, filename, parse_content_range, partition, validate_url};

struct Server {
    address: String,
    bytes: Arc<Vec<u8>>,
    stop: Arc<AtomicBool>,
    changed: Arc<AtomicBool>,
    peak: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<String>>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Server {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let bytes = Arc::new(
            (0..4 * 1024 * 1024)
                .map(|i| (i % 251) as u8)
                .collect::<Vec<_>>(),
        );
        let stop = Arc::new(AtomicBool::new(false));
        let changed = Arc::new(AtomicBool::new(false));
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(Mutex::new(vec![]));
        let (s, b, c, a, p, r) = (
            stop.clone(),
            bytes.clone(),
            changed.clone(),
            active.clone(),
            peak.clone(),
            requests.clone(),
        );
        let task = thread::spawn(move || {
            while !s.load(Ordering::SeqCst) {
                if let Ok((stream, _)) = listener.accept() {
                    let (b, c, a, p, r) = (b.clone(), c.clone(), a.clone(), p.clone(), r.clone());
                    thread::spawn(move || {
                        let n = a.fetch_add(1, Ordering::SeqCst) + 1;
                        p.fetch_max(n, Ordering::SeqCst);
                        if let Err(error) = serve(stream, &b, c.load(Ordering::SeqCst), &r) {
                            eprintln!("Test connection ended: {error}");
                        }
                        a.fetch_sub(1, Ordering::SeqCst);
                    });
                } else {
                    thread::sleep(Duration::from_millis(2));
                }
            }
        });
        Self {
            address,
            bytes,
            stop,
            changed,
            peak,
            requests,
            thread: Some(task),
        }
    }
    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.address)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.thread.take().unwrap().join().unwrap();
    }
}

fn serve(
    mut stream: TcpStream,
    bytes: &[u8],
    changed: bool,
    requests: &Mutex<Vec<String>>,
) -> std::io::Result<()> {
    // Winsock can inherit the listener's nonblocking mode on accepted sockets.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    let mut request = Vec::new();
    while !request.ends_with(b"\r\n\r\n") {
        let mut b = [0u8; 1];
        if stream.read(&mut b)? == 0 {
            return Ok(());
        }
        request.push(b[0]);
        if request.len() > 16384 {
            return Ok(());
        }
    }
    let request = String::from_utf8_lossy(&request).to_lowercase();
    requests.lock().unwrap().push(request.clone());
    let plain = request.contains("/plain");
    let bad = request.contains("/bad");
    let empty = request.contains("/empty");
    let corrupt = request.contains("/corrupt");
    if request.contains("/basic") && !request.contains("authorization: basic ") {
        stream.write_all(
            b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )?;
        return Ok(());
    }
    let range = request
        .lines()
        .find_map(|l| l.strip_prefix("range: bytes="));
    let total = if empty { 0 } else { bytes.len() };
    let (start, end, partial) = if !plain && total > 0 {
        if let Some(range) = range {
            let (a, b) = range.split_once('-').unwrap();
            (
                a.parse::<usize>().unwrap(),
                if b.is_empty() {
                    total - 1
                } else {
                    b.parse::<usize>().unwrap()
                },
                true,
            )
        } else {
            (0, total.saturating_sub(1), false)
        }
    } else {
        (0, total.saturating_sub(1), false)
    };
    let status = if partial {
        "206 Partial Content"
    } else {
        "200 OK"
    };
    let length = if total == 0 { 0 } else { end - start + 1 };
    let mut headers = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {length}\r\nETag: \"{}\"\r\nConnection: close\r\n",
        if changed { "v2" } else { "v1" }
    );
    if partial {
        let begin = if bad && range != Some("0-0") {
            start + 1
        } else {
            start
        };
        headers += &format!("Content-Range: bytes {begin}-{end}/{total}\r\n");
    }
    if request.contains("/video.mp4") {
        headers += "Content-Type: video/mp4\r\n";
    }
    stream.write_all(format!("{headers}\r\n").as_bytes())?;
    if total > 0 {
        for chunk in bytes[start..=end].chunks(8192) {
            if changed || corrupt {
                let data: Vec<_> = chunk.iter().map(|v| v.wrapping_add(1)).collect();
                stream.write_all(&data)?;
            } else {
                stream.write_all(chunk)?;
            }
            thread::sleep(Duration::from_millis(3));
        }
    }
    Ok(())
}

fn wait(
    manager: &Manager,
    id: u64,
    predicate: impl Fn(&strata::engine::Download) -> bool,
) -> strata::engine::Download {
    let start = Instant::now();
    loop {
        let data = manager
            .snapshots()
            .into_iter()
            .find(|d| d.id == id)
            .unwrap();
        if predicate(&data) {
            return data;
        }
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "Timed out: {:?}, error {:?}",
            data.status,
            data.error
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn partitions_cover_every_byte_exactly_once() {
    for size in [
        0,
        1,
        2,
        15,
        16,
        17,
        1000,
        4194305,
        u32::MAX as u64,
        u64::MAX,
    ] {
        for count in [0, 1, 2, 8, 16, 100] {
            let parts = partition(size, count);
            if size == 0 {
                assert!(parts.is_empty());
                continue;
            }
            assert_eq!(parts.first().unwrap().start, 0);
            assert_eq!(parts.last().unwrap().end, size - 1);
            for pair in parts.windows(2) {
                assert_eq!(pair[0].end + 1, pair[1].start);
            }
            assert_eq!(
                parts.iter().map(|p| p.len() as u128).sum::<u128>(),
                size as u128
            );
        }
    }
}

#[test]
fn validates_urls_names_and_ranges() {
    assert!(validate_url("file:///etc/passwd").is_err());
    assert!(validate_url("https://user:pass@example.com/file").is_err());
    assert_eq!(
        filename(&validate_url("https://example.com/%2e%2e%2fCON.txt").unwrap()),
        "_CON.txt"
    );
    assert_eq!(
        filename(&validate_url("https://example.com/CON.txt").unwrap()),
        "download_CON.txt"
    );
    assert_eq!(parse_content_range("bytes 2-4/5"), Some((2, 4, 5)));
    assert_eq!(parse_content_range("bytes 4-2/5"), None);
    assert_eq!(parse_content_range("bytes 0-5/5"), None);
    assert_eq!(parse_content_range("bytes */5"), None);
}

#[test]
fn downloads_eight_ranges_concurrently_and_assembles_exact_bytes() {
    let server = Server::new();
    let dir = tempfile::tempdir().unwrap();
    let manager = Manager::new(dir.path().join("history.json")).unwrap();
    let id = manager
        .add(&server.url("/file.bin"), dir.path().into(), 8)
        .unwrap();
    let d = wait(&manager, id, |d| {
        matches!(d.status, Status::Complete | Status::Failed)
    });
    assert_eq!(d.status, Status::Complete, "{:?}", d.error);
    assert_eq!(d.parts.len(), 8);
    assert!(d.ranged);
    assert!(
        server.peak.load(Ordering::SeqCst) >= 2,
        "Connections did not overlap"
    );
    assert_eq!(std::fs::read(d.path()).unwrap(), *server.bytes);
    assert!(!d.parts_dir().exists());
    manager.shutdown();
}

#[test]
fn falls_back_when_server_ignores_ranges_and_handles_empty_files() {
    let server = Server::new();
    let dir = tempfile::tempdir().unwrap();
    let manager = Manager::new(dir.path().join("history.json")).unwrap();
    for route in ["/plain.bin", "/empty.bin"] {
        let id = manager
            .add(&server.url(route), dir.path().into(), 8)
            .unwrap();
        let d = wait(&manager, id, |d| {
            matches!(d.status, Status::Complete | Status::Failed)
        });
        assert_eq!(
            d.status,
            Status::Complete,
            "{route}: {:?}, {} bytes received",
            d.error,
            d.downloaded
        );
        assert!(!d.ranged);
        assert_eq!(d.parts.len(), 1);
        assert_eq!(
            std::fs::read(d.path()).unwrap(),
            if route.contains("empty") {
                vec![]
            } else {
                server.bytes.to_vec()
            }
        );
    }
    manager.shutdown();
}

#[test]
fn pauses_and_resumes_from_saved_offsets_after_restart() {
    let server = Server::new();
    let dir = tempfile::tempdir().unwrap();
    let history = dir.path().join("history.json");
    let manager = Manager::new(history.clone()).unwrap();
    let id = manager
        .add(&server.url("/file.bin"), dir.path().into(), 8)
        .unwrap();
    wait(&manager, id, |d| d.downloaded > 65536);
    manager.pause(id);
    let paused = wait(&manager, id, |d| d.status == Status::Paused);
    assert!(paused.downloaded < paused.total.unwrap());
    assert!(!paused.path().exists());
    manager.shutdown();
    drop(manager);
    let manager = Manager::new(history).unwrap();
    assert_eq!(manager.snapshots()[0].status, Status::Paused);
    server.requests.lock().unwrap().clear();
    manager.resume(id);
    let done = wait(&manager, id, |d| {
        matches!(d.status, Status::Complete | Status::Failed)
    });
    assert_eq!(done.status, Status::Complete, "{:?}", done.error);
    assert_eq!(std::fs::read(done.path()).unwrap(), *server.bytes);
    assert!(
        server
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.contains("if-range: \"v1\""))
    );
    manager.shutdown();
}

#[test]
fn changed_remote_file_restarts_instead_of_combining_versions() {
    let server = Server::new();
    let dir = tempfile::tempdir().unwrap();
    let manager = Manager::new(dir.path().join("history.json")).unwrap();
    let id = manager
        .add(&server.url("/file.bin"), dir.path().into(), 8)
        .unwrap();
    wait(&manager, id, |d| d.downloaded > 65536);
    manager.pause(id);
    wait(&manager, id, |d| d.status == Status::Paused);
    server.changed.store(true, Ordering::SeqCst);
    manager.resume(id);
    let done = wait(&manager, id, |d| {
        matches!(d.status, Status::Complete | Status::Failed)
    });
    assert_eq!(done.status, Status::Complete, "{:?}", done.error);
    assert_eq!(
        std::fs::read(done.path()).unwrap(),
        server
            .bytes
            .iter()
            .map(|b| b.wrapping_add(1))
            .collect::<Vec<_>>()
    );
    manager.shutdown();
}

#[test]
fn rejects_incorrect_ranges_and_never_overwrites_existing_files() {
    let server = Server::new();
    let dir = tempfile::tempdir().unwrap();
    let manager = Manager::new(dir.path().join("history.json")).unwrap();
    let id = manager
        .add(&server.url("/bad.bin"), dir.path().into(), 8)
        .unwrap();
    let d = wait(&manager, id, |d| {
        matches!(d.status, Status::Complete | Status::Failed)
    });
    assert_eq!(d.status, Status::Failed);
    assert!(!d.path().exists());
    let id = manager
        .add(&server.url("/file.bin"), dir.path().into(), 8)
        .unwrap();
    let d = wait(&manager, id, |d| d.status == Status::Downloading);
    std::fs::write(d.path(), b"keep me").unwrap();
    let d = wait(&manager, id, |d| {
        matches!(d.status, Status::Complete | Status::Failed)
    });
    assert_eq!(d.status, Status::Failed);
    assert_eq!(std::fs::read(d.path()).unwrap(), b"keep me");
    manager.shutdown();
}

#[test]
fn browser_handoff_preserves_safe_name_and_referrer() {
    let server = Server::new();
    let dir = tempfile::tempdir().unwrap();
    let manager = Manager::new(dir.path().join("history.json")).unwrap();
    manager.settings.lock().unwrap().directory = dir.path().into();
    let referrer = server.url("/downloads");
    let id = manager
        .add_from_browser(
            &server.url("/file.bin"),
            Some(r"C:\Downloads\..\browser-file.bin"),
            Some(&referrer),
        )
        .unwrap();
    let done = wait(&manager, id, |d| {
        matches!(d.status, Status::Complete | Status::Failed)
    });
    assert_eq!(done.status, Status::Complete, "{:?}", done.error);
    assert_eq!(done.name, "browser-file.bin");
    assert_eq!(done.directory, dir.path());
    assert_eq!(std::fs::read(done.path()).unwrap(), *server.bytes);
    assert!(
        server
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|r| r.contains(&format!("referer: {referrer}")))
    );
    manager.shutdown();
}

#[test]
fn refuses_browser_handoff_if_history_cannot_be_saved() {
    let dir = tempfile::tempdir().unwrap();
    let blocked = dir.path().join("blocked");
    std::fs::write(&blocked, b"file").unwrap();
    let manager = Manager::new(blocked.join("history.json")).unwrap();
    manager.settings.lock().unwrap().directory = dir.path().into();
    assert!(
        manager
            .add_from_browser("https://example.com/file.bin", None, None)
            .is_err()
    );
    assert!(manager.snapshots().is_empty());
    manager.shutdown();
}

#[test]
fn webdav_authentication_is_forwarded_without_persisting_passwords() {
    let server = Server::new();
    let dir = tempfile::tempdir().unwrap();
    let manager = Manager::new(dir.path().join("history.json")).unwrap();
    let url = server
        .url("/basic.bin")
        .replace("http://", "webdav://alice:test-password@");
    let id = manager.add(&url, dir.path().into(), 8).unwrap();
    let done = wait(&manager, id, |d| {
        matches!(d.status, Status::Complete | Status::Failed)
    });
    assert_eq!(done.status, Status::Complete, "{:?}", done.error);
    assert_eq!(done.protocol, strata::protocol::Protocol::WebDav);
    assert_eq!(std::fs::read(done.path()).unwrap(), *server.bytes);
    assert!(
        server
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|r| r.contains("authorization: basic "))
    );
    manager.shutdown();
    let saved = std::fs::read_to_string(dir.path().join("history.json")).unwrap();
    assert!(!saved.contains("test-password"));
    assert!(!done.url.contains("alice@"));
}

#[test]
fn downloads_byte_exact_ranges_through_an_explicit_http_proxy() {
    let proxy = Server::new();
    let dir = tempfile::tempdir().unwrap();
    let manager = Manager::new(dir.path().join("history.json")).unwrap();
    let options = strata::protocol::TransferOptions {
        proxy: Some(strata::protocol::Proxy::parse(&proxy.address).unwrap()),
        ..Default::default()
    };
    assert!(
        manager
            .add_with_options("sftp://host/file", dir.path().into(), 1, options.clone())
            .is_err()
    );
    assert!(manager.snapshots().is_empty());
    let id = manager
        .add_with_options(
            "http://must-not-resolve.invalid/file.bin",
            dir.path().into(),
            8,
            options,
        )
        .unwrap();
    let done = wait(&manager, id, |d| {
        matches!(d.status, Status::Complete | Status::Failed)
    });
    assert_eq!(done.status, Status::Complete, "{:?}", done.error);
    assert_eq!(std::fs::read(done.path()).unwrap(), *proxy.bytes);
    assert!(
        proxy
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|r| r.starts_with("get http://must-not-resolve.invalid/"))
    );
    manager.shutdown();
}

#[test]
fn metalink_tries_another_mirror_on_hash_mismatch_and_verifies_all_hash_types() {
    use sha2::Digest;
    let server = Server::new();
    let dir = tempfile::tempdir().unwrap();
    let sha256 = format!("{:x}", sha2::Sha256::digest(&*server.bytes));
    let sha512 = format!("{:x}", sha2::Sha512::digest([]));
    let md5 = format!("{:x}", md5::Md5::digest([]));
    let descriptor = format!(
        r#"<metalink xmlns="urn:ietf:params:xml:ns:metalink"><file name="folder/file.bin"><size>{}</size><hash type="sha-256">{sha256}</hash><url priority="1">{}</url><url priority="2">{}</url></file><file name="empty.bin"><size>0</size><hash type="sha-512">{sha512}</hash><hash type="md5">{md5}</hash><url>{}</url></file></metalink>"#,
        server.bytes.len(),
        server.url("/corrupt"),
        server.url("/good"),
        server.url("/empty")
    );
    let path = dir.path().join("test.meta4");
    std::fs::write(&path, descriptor).unwrap();
    let manager = Manager::new(dir.path().join("history.json")).unwrap();
    let id = manager
        .add(&path.to_string_lossy(), dir.path().into(), 8)
        .unwrap();
    let done = wait(&manager, id, |d| {
        matches!(d.status, Status::Complete | Status::Failed)
    });
    assert_eq!(done.status, Status::Complete, "{:?}", done.error);
    assert_eq!(
        std::fs::read(done.path().join("folder/file.bin")).unwrap(),
        *server.bytes
    );
    assert_eq!(
        std::fs::metadata(done.path().join("empty.bin"))
            .unwrap()
            .len(),
        0
    );
    let requests = server.requests.lock().unwrap();
    assert!(requests.iter().any(|r| r.contains("/corrupt")));
    assert!(requests.iter().any(|r| r.contains("/good")));
    assert!(!done.parts_dir().exists());
    manager.shutdown();
}

struct SocksProxy {
    url: String,
    connections: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    task: Option<thread::JoinHandle<()>>,
}
impl SocksProxy {
    fn new(target: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!(
            "socks5h://bob:proxy-test-password@{}",
            listener.local_addr().unwrap()
        );
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let connections = Arc::new(AtomicUsize::new(0));
        let count = connections.clone();
        let task = thread::spawn(move || {
            while !stopping.load(Ordering::SeqCst) {
                if let Ok((mut client, _)) = listener.accept() {
                    let target = target.clone();
                    let count = count.clone();
                    thread::spawn(move || {
                        let mut run = || -> std::io::Result<()> {
                            client.set_nonblocking(false)?;
                            client.set_read_timeout(Some(Duration::from_secs(5)))?;
                            let mut greeting = [0; 2];
                            client.read_exact(&mut greeting)?;
                            let mut methods = vec![0; greeting[1] as usize];
                            client.read_exact(&mut methods)?;
                            if greeting[0] != 5 || !methods.contains(&2) {
                                return Err(std::io::ErrorKind::InvalidData.into());
                            }
                            client.write_all(&[5, 2])?;
                            let mut auth = [0; 2];
                            client.read_exact(&mut auth)?;
                            let mut user = vec![0; auth[1] as usize];
                            client.read_exact(&mut user)?;
                            let mut len = [0];
                            client.read_exact(&mut len)?;
                            let mut password = vec![0; len[0] as usize];
                            client.read_exact(&mut password)?;
                            if user != b"bob" || password != b"proxy-test-password" {
                                return Err(std::io::ErrorKind::PermissionDenied.into());
                            }
                            client.write_all(&[1, 0])?;
                            let mut connect = [0; 4];
                            client.read_exact(&mut connect)?;
                            if connect != [5, 1, 0, 3] {
                                return Err(std::io::ErrorKind::InvalidData.into());
                            }
                            client.read_exact(&mut len)?;
                            let mut hostname = vec![0; len[0] as usize];
                            client.read_exact(&mut hostname)?;
                            if hostname != b"resolve-via-proxy.invalid" {
                                return Err(std::io::ErrorKind::InvalidData.into());
                            }
                            let mut port = [0; 2];
                            client.read_exact(&mut port)?;
                            let mut upstream = TcpStream::connect(&target)?;
                            upstream.set_read_timeout(Some(Duration::from_secs(5)))?;
                            client.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 80])?;
                            count.fetch_add(1, Ordering::SeqCst);
                            let mut from_client = client.try_clone()?;
                            let mut to_server = upstream.try_clone()?;
                            let writer = thread::spawn(move || {
                                let _ = std::io::copy(&mut from_client, &mut to_server);
                            });
                            let _ = std::io::copy(&mut upstream, &mut client);
                            let _ = client.shutdown(std::net::Shutdown::Both);
                            let _ = writer.join();
                            Ok(())
                        };
                        let _ = run();
                    });
                } else {
                    thread::sleep(Duration::from_millis(3));
                }
            }
        });
        Self {
            url,
            connections,
            stop,
            task: Some(task),
        }
    }
}
impl Drop for SocksProxy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.task.take().unwrap().join();
    }
}

#[test]
fn socks5_proxy_authentication_remote_dns_and_parallel_ranges_are_byte_exact() {
    let server = Server::new();
    let socks = SocksProxy::new(server.address.trim_start_matches("http://").to_owned());
    let directory = tempfile::tempdir().unwrap();
    let manager = Manager::new(directory.path().join("history.json")).unwrap();
    let options = strata::protocol::TransferOptions {
        proxy: Some(strata::protocol::Proxy::parse(&socks.url).unwrap()),
        ..Default::default()
    };
    let id = manager
        .add_with_options(
            "http://resolve-via-proxy.invalid/file.bin",
            directory.path().into(),
            8,
            options,
        )
        .unwrap();
    let done = wait(&manager, id, |d| {
        matches!(d.status, Status::Complete | Status::Failed)
    });
    assert_eq!(done.status, Status::Complete, "{:?}", done.error);
    assert_eq!(std::fs::read(done.path()).unwrap(), *server.bytes);
    assert!(socks.connections.load(Ordering::SeqCst) > 1);
    manager.shutdown();
    assert!(
        !std::fs::read_to_string(directory.path().join("history.json"))
            .unwrap()
            .contains("proxy-test-password")
    );
}

#[test]
#[ignore = "requires pinned yt-dlp media helpers; run scripts/fetch-media-tools.ps1 first"]
fn ytdlp_download_pause_and_resume_are_byte_exact() {
    let server = Server::new();
    let directory = tempfile::tempdir().unwrap();
    let manager = Manager::new(directory.path().join("history.json")).unwrap();
    let options = strata::protocol::TransferOptions {
        protocol: Some(strata::protocol::Protocol::Video),
        ..Default::default()
    };
    let id = manager
        .add_with_options(
            &server.url("/video.mp4"),
            directory.path().into(),
            4,
            options,
        )
        .unwrap();
    let active = wait(&manager, id, |d| {
        d.downloaded > 64 * 1024 || matches!(d.status, Status::Failed | Status::Complete)
    });
    assert_eq!(active.status, Status::Downloading, "{:?}", active.error);
    manager.pause(id);
    let paused = wait(&manager, id, |d| {
        matches!(d.status, Status::Paused | Status::Failed)
    });
    assert_eq!(paused.status, Status::Paused, "{:?}", paused.error);
    manager.resume(id);
    let done = wait(&manager, id, |d| {
        matches!(d.status, Status::Complete | Status::Failed)
    });
    assert_eq!(done.status, Status::Complete, "{:?}", done.error);
    let files = std::fs::read_dir(done.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_file())
        .collect::<Vec<_>>();
    assert_eq!(files.len(), 1);
    assert_eq!(std::fs::read(&files[0]).unwrap(), *server.bytes);
    assert!(!done.parts_dir().exists());
    manager.shutdown();
    assert!(server.requests.lock().unwrap().iter().any(|request| {
        request
            .lines()
            .any(|line| line.starts_with("range: bytes=") && !line.starts_with("range: bytes=0-"))
    }));
}
