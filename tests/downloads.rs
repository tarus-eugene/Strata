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
    let range = request
        .lines()
        .find_map(|l| l.strip_prefix("range: bytes="));
    let total = if empty { 0 } else { bytes.len() };
    let (start, end, partial) = if !plain && total > 0 {
        if let Some(range) = range {
            let (a, b) = range.split_once('-').unwrap();
            (
                a.parse::<usize>().unwrap(),
                b.parse::<usize>().unwrap(),
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
    stream.write_all(format!("{headers}\r\n").as_bytes())?;
    if total > 0 {
        for chunk in bytes[start..=end].chunks(8192) {
            if changed {
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
