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
    let retry_count = {
        let mut requests = requests.lock().unwrap();
        requests.push(request.clone());
        requests.iter().filter(|prior| *prior == &request).count()
    };
    if request.contains("/retry-forever")
        || (request.contains("/retry-once")
            && request.contains("range: bytes=0-0")
            && retry_count == 1)
    {
        stream.write_all(
            b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )?;
        return Ok(());
    }
    if request.contains("/not-found") {
        stream.write_all(
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )?;
        return Ok(());
    }
    if request.contains("/name-redirect/") {
        stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: /Readable-report.pdf\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")?;
        return Ok(());
    }
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
    if request.contains("/name-early/") {
        headers += "Content-Disposition: attachment; filename=report.pdf; filename*=UTF-8''Quarterly%20caf%C3%A9%20report.pdf\r\n";
    }
    if request.contains("/name-late/") && range != Some("0-0") {
        headers += "Content-Disposition: attachment; filename=\"Quarterly; final report.pdf\"\r\n";
    }
    if request.contains("/name-late/") || request.contains("/type-only/") {
        headers += "Content-Type: application/pdf\r\n";
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
        // Bundled media startup/probing needs more time when Windows runs the suite in parallel.
        let timeout = if data.protocol == strata::protocol::Protocol::Video {
            45
        } else {
            15
        };
        assert!(
            start.elapsed() < Duration::from_secs(timeout),
            "Timed out: {:?}, error {:?}",
            data.status,
            data.error
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn transient_probe_failure_retries_and_finishes_with_auto_connections() {
    let server = Server::new();
    let root = tempfile::tempdir().unwrap();
    let manager = Manager::new(root.path().join("history.json")).unwrap();
    let id = manager
        .add(
            &server.url("/retry-once"),
            root.path().into(),
            strata::engine::AUTO_CONNECTIONS,
        )
        .unwrap();

    wait(&manager, id, |d| d.status == Status::Retrying);
    let result = wait(&manager, id, |d| {
        matches!(d.status, Status::Complete | Status::Failed)
    });
    assert_eq!(result.status, Status::Complete, "{:?}", result.error);
    assert_eq!(result.parts.len(), 1);
    assert_eq!(std::fs::read(result.path()).unwrap(), *server.bytes);
    assert_eq!(server.requests.lock().unwrap().len(), 3);
}

#[test]
fn retry_wait_can_be_paused_and_missing_files_fail_immediately() {
    let server = Server::new();
    let root = tempfile::tempdir().unwrap();
    let manager = Manager::new(root.path().join("history.json")).unwrap();
    let id = manager
        .add(&server.url("/retry-forever"), root.path().into(), 1)
        .unwrap();
    wait(&manager, id, |d| d.status == Status::Retrying);
    manager.pause(id);
    wait(&manager, id, |d| d.status == Status::Paused);

    let id = manager
        .add(&server.url("/not-found"), root.path().into(), 1)
        .unwrap();
    let result = wait(&manager, id, |d| {
        matches!(d.status, Status::Failed | Status::Retrying)
    });
    assert_eq!(result.status, Status::Failed);
    assert_eq!(server.requests.lock().unwrap().len(), 2);
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
    use sha2::Digest;
    assert_eq!(d.integrity.len(), 1);
    assert_eq!(d.integrity[0].size, server.bytes.len() as u64);
    assert_eq!(
        d.integrity[0].sha256,
        format!("{:x}", sha2::Sha256::digest(&*server.bytes))
    );
    assert!(!d.parts_dir().exists());
    assert!(
        d.completed_at
            .is_some_and(|completed| completed >= d.created)
    );
    manager.shutdown();
}

#[test]
fn completion_actions_are_session_only_and_consumed_after_success() {
    use strata::engine::CompletionAction;

    let server = Server::new();
    let root = tempfile::tempdir().unwrap();
    let history = root.path().join("history.json");
    let manager = Manager::new(history.clone()).unwrap();
    let id = manager
        .add(&server.url("/retry-once"), root.path().into(), 1)
        .unwrap();
    manager
        .set_completion_action(id, Some(CompletionAction::Open))
        .unwrap();
    manager.persist();
    {
        let restored = Manager::new(history).unwrap();
        assert!(restored.snapshots()[0].completion_action.is_none());
        assert!(restored.take_completion_actions().is_empty());
    }
    wait(&manager, id, |d| {
        if d.status != Status::Complete {
            return false;
        }
        let actions = manager.take_completion_actions();
        if actions.is_empty() {
            return false;
        }
        assert_eq!(actions, vec![(id, CompletionAction::Open)]);
        true
    });
    assert!(manager.take_completion_actions().is_empty());
    assert!(manager.snapshots()[0].completion_action.is_none());
    assert!(
        manager
            .set_completion_action(id, Some(CompletionAction::Open))
            .is_err()
    );
}

#[test]
fn completion_chime_is_requested_once_and_can_be_disabled() {
    let server = Server::new();
    let root = tempfile::tempdir().unwrap();
    let history = root.path().join("history.json");
    let manager = Manager::new(history.clone()).unwrap();
    let id = manager
        .add(&server.url("/chime.txt"), root.path().into(), 1)
        .unwrap();
    wait(&manager, id, |d| {
        d.status == Status::Complete && manager.take_completion_chime()
    });
    assert!(!manager.take_completion_chime());

    manager.settings.lock().unwrap().completion_chime = false;
    let id = manager
        .add(&server.url("/muted.txt"), root.path().into(), 1)
        .unwrap();
    wait(&manager, id, |d| d.status == Status::Complete);
    manager.shutdown();
    assert!(!manager.take_completion_chime());

    let restored = Manager::new(history).unwrap();
    assert!(!restored.settings.lock().unwrap().completion_chime);
    assert!(!restored.take_completion_chime());
}

#[test]
fn dragged_download_keeps_its_new_location_and_integrity_after_restart() {
    let server = Server::new();
    let root = tempfile::tempdir().unwrap();
    let history = root.path().join("history.json");
    let manager = Manager::new(history.clone()).unwrap();
    let id = manager
        .add(&server.url("/move.txt"), root.path().into(), 1)
        .unwrap();
    let complete = wait(&manager, id, |d| d.status == Status::Complete);
    manager.shutdown();
    let manager = Manager::new(history.clone()).unwrap();
    let original = complete.path();
    let folder = root.path().join("moved");
    std::fs::create_dir(&folder).unwrap();
    let destination = folder.join("Renamed.txt");
    manager
        .drag_completed_download(id, &original, |path| {
            std::fs::rename(path, &destination)?;
            Ok(Some(destination.clone()))
        })
        .unwrap();
    let restored = Manager::new(history).unwrap();
    let current = restored.snapshots().pop().unwrap();
    assert_eq!(current.status, Status::Complete);
    assert_eq!(current.path(), destination);
    assert_eq!(current.completed_at, complete.completed_at);
    assert_eq!(current.integrity[0].sha256, complete.integrity[0].sha256);
    assert!(!original.exists());
}

#[test]
fn completed_downloads_detect_corruption_and_missing_files_across_restart() {
    let server = Server::new();
    let directory = tempfile::tempdir().unwrap();
    let history = directory.path().join("history.json");
    let manager = Manager::new(history.clone()).unwrap();
    let id = manager
        .add(&server.url("/file.bin"), directory.path().into(), 8)
        .unwrap();
    let done = wait(&manager, id, |d| d.status == Status::Complete);
    let mut corrupt = server.bytes.to_vec();
    corrupt[0] ^= 1;
    std::fs::write(done.path(), &corrupt).unwrap();
    manager.verify_integrity(id);
    let failed = wait(&manager, id, |d| d.status == Status::Failed);
    assert!(failed.error.unwrap().contains("checksum mismatch"));
    manager.resume(id);
    assert_eq!(manager.snapshots()[0].status, Status::Failed);
    assert_eq!(std::fs::read(done.path()).unwrap(), corrupt);
    std::fs::write(done.path(), &*server.bytes).unwrap();
    manager.verify_integrity(id);
    wait(&manager, id, |d| d.status == Status::Complete);
    std::fs::remove_file(done.path()).unwrap();
    wait(&manager, id, |d| d.status == Status::Missing);
    manager.shutdown();
    drop(manager);

    let restored = Manager::new(history.clone()).unwrap();
    let saved = restored.snapshots().remove(0);
    assert_eq!(saved.status, Status::Missing);
    assert_eq!(saved.integrity[0].sha256, done.integrity[0].sha256);
    restored.resume(id);
    assert_eq!(restored.snapshots()[0].status, Status::Missing);
    std::fs::write(done.path(), &*server.bytes).unwrap();
    wait(&restored, id, |d| d.status == Status::Complete);
    restored.shutdown();
    drop(restored);
    std::fs::remove_file(done.path()).unwrap();
    let restored = Manager::new(history).unwrap();
    assert_eq!(restored.snapshots()[0].status, Status::Missing);
    restored.delete(id);
    wait_removed(&restored, id);
    restored.shutdown();
}

#[test]
fn legacy_downloads_check_size_record_a_baseline_and_detect_later_corruption() {
    let server = Server::new();
    let directory = tempfile::tempdir().unwrap();
    let history = directory.path().join("history.json");
    let manager = Manager::new(history.clone()).unwrap();
    let id = manager
        .add(&server.url("/file.bin"), directory.path().into(), 8)
        .unwrap();
    let done = wait(&manager, id, |d| d.status == Status::Complete);
    manager.shutdown();
    drop(manager);
    let mut stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&history).unwrap()).unwrap();
    stored["downloads"][0]
        .as_object_mut()
        .unwrap()
        .remove("integrity");
    stored["downloads"][0]
        .as_object_mut()
        .unwrap()
        .remove("integrity_baseline_only");
    std::fs::write(&history, serde_json::to_vec(&stored).unwrap()).unwrap();
    let restored = Manager::new(history).unwrap();
    let legacy = restored.snapshots().remove(0);
    assert!(legacy.integrity.is_empty());
    assert!(legacy.can_verify_integrity());
    for status in [
        Status::Queued,
        Status::Downloading,
        Status::Verifying,
        Status::Deleting,
    ] {
        let mut busy = legacy.clone();
        busy.status = status;
        assert!(!busy.can_verify_integrity());
    }
    std::fs::write(done.path(), b"truncated").unwrap();
    restored.verify_integrity(id);
    let failed = wait(&restored, id, |d| d.status == Status::Failed);
    assert!(failed.error.unwrap().contains("recorded download size"));
    std::fs::write(done.path(), &*server.bytes).unwrap();
    restored.verify_integrity(id);
    let baseline = wait(&restored, id, |d| d.status == Status::Complete);
    assert!(baseline.integrity_baseline_only);
    assert_eq!(baseline.integrity.len(), 1);
    restored.verify_integrity(id);
    let verified = wait(&restored, id, |d| d.status == Status::Complete);
    assert!(!verified.integrity_baseline_only);
    let mut corrupt = server.bytes.to_vec();
    corrupt[0] ^= 1;
    std::fs::write(done.path(), corrupt).unwrap();
    restored.verify_integrity(id);
    assert!(
        wait(&restored, id, |d| d.status == Status::Failed)
            .error
            .unwrap()
            .contains("checksum mismatch")
    );
    restored.shutdown();
}

#[test]
fn local_and_global_speed_caps_preserve_bytes_and_active_rename_resumes() {
    use strata::engine::DownloadChange;
    let server = Server::new();
    let dir = tempfile::tempdir().unwrap();
    let manager = Manager::new(dir.path().join("history.json")).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime
        .block_on(manager.set_global_speed_limit(2 * 1024 * 1024))
        .unwrap()
        .unwrap();
    let first = manager
        .add(&server.url("/file.bin"), dir.path().into(), 4)
        .unwrap();
    runtime
        .block_on(manager.change_download(first, DownloadChange::SpeedLimit(1024 * 1024)))
        .unwrap()
        .unwrap();
    let baseline = manager.snapshots()[0].downloaded;
    let started = Instant::now();
    let second = manager
        .add(&server.url("/file.bin"), dir.path().into(), 4)
        .unwrap();
    runtime
        .block_on(manager.change_download(
            first,
            DownloadChange::Rename("renamed while active.bin".into()),
        ))
        .unwrap()
        .unwrap();
    let a = wait(&manager, first, |d| {
        matches!(d.status, Status::Complete | Status::Failed)
    });
    let b = wait(&manager, second, |d| {
        matches!(d.status, Status::Complete | Status::Failed)
    });
    assert_eq!(a.status, Status::Complete, "{:?}", a.error);
    assert_eq!(b.status, Status::Complete, "{:?}", b.error);
    let minimum = Duration::from_secs_f64(
        (server.bytes.len() as u64 * 2 - baseline) as f64 / (2 * 1024 * 1024) as f64,
    );
    assert!(
        started.elapsed() >= minimum.mul_f64(0.9),
        "The shared cap was exceeded"
    );
    assert_eq!(std::fs::read(a.path()).unwrap(), *server.bytes);
    assert_eq!(std::fs::read(b.path()).unwrap(), *server.bytes);
    assert_eq!(a.name, "renamed while active.bin");
    assert!(
        a.completed_at
            .is_some_and(|completed| completed >= a.created)
    );
    assert!(
        b.completed_at
            .is_some_and(|completed| completed >= b.created)
    );
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
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime
        .block_on(manager.set_global_speed_limit(2 * 1024 * 1024))
        .unwrap()
        .unwrap();
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
    assert_eq!(done.integrity.len(), 1);
    assert_eq!(done.integrity[0].size, server.bytes.len() as u64);
    assert!(!done.parts_dir().exists());
    manager.shutdown();
    assert!(server.requests.lock().unwrap().iter().any(|request| {
        request
            .lines()
            .any(|line| line.starts_with("range: bytes=") && !line.starts_with("range: bytes=0-"))
    }));
}

fn wait_removed(manager: &Manager, id: u64) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while let Some(download) = manager.snapshots().into_iter().find(|d| d.id == id) {
        assert_ne!(download.status, Status::Failed, "{:?}", download.error);
        assert!(Instant::now() < deadline, "Deletion did not finish");
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn deleting_entries_stops_writes_and_removes_only_their_downloaded_data() {
    let server = Server::new();
    let directory = tempfile::tempdir().unwrap();
    let history = directory.path().join("history.json");
    let sentinel = directory.path().join("unrelated.txt");
    std::fs::write(&sentinel, "keep me").unwrap();
    let manager = Manager::new(history.clone()).unwrap();
    let id = manager
        .add(&server.url("/plain"), directory.path().into(), 1)
        .unwrap();
    let file = wait(&manager, id, |d| {
        matches!(d.status, Status::Complete | Status::Failed)
    });
    assert_eq!(file.status, Status::Complete, "{:?}", file.error);
    manager.delete(id);
    wait_removed(&manager, id);
    assert!(!file.path().exists());
    let id = manager
        .add(&server.url("/ranged"), directory.path().into(), 4)
        .unwrap();
    let active = wait(&manager, id, |d| d.downloaded > 0);
    manager.delete(id);
    manager.resume(id);
    wait_removed(&manager, id);
    assert!(!active.path().exists());
    assert!(!active.parts_dir().exists());
    let id = manager
        .add(&server.url("/ranged"), directory.path().into(), 4)
        .unwrap();
    let active = wait(&manager, id, |d| d.downloaded > 0);
    std::fs::write(
        active.path(),
        "created outside Strata while the transfer was pending",
    )
    .unwrap();
    manager.delete(id);
    wait_removed(&manager, id);
    assert_eq!(
        std::fs::read_to_string(active.path()).unwrap(),
        "created outside Strata while the transfer was pending"
    );
    assert!(!active.parts_dir().exists());
    assert_eq!(std::fs::read_to_string(sentinel).unwrap(), "keep me");
    manager.shutdown();
    assert!(Manager::new(history).unwrap().snapshots().is_empty());
}

#[test]
fn deleting_rejects_history_paths_outside_the_download_directory() {
    let server = Server::new();
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("output");
    std::fs::create_dir(&output).unwrap();
    let sentinel = directory.path().join("outside.txt");
    std::fs::write(&sentinel, "must survive").unwrap();
    let history = directory.path().join("history.json");
    let manager = Manager::new(history.clone()).unwrap();
    let id = manager.add(&server.url("/plain"), output, 1).unwrap();
    wait(&manager, id, |d| d.status == Status::Complete);
    manager.shutdown();
    drop(manager);
    let mut stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&history).unwrap()).unwrap();
    stored["downloads"][0]["name"] = "../outside.txt".into();
    stored["downloads"][0]["status"] = "Deleting".into();
    std::fs::write(&history, serde_json::to_vec(&stored).unwrap()).unwrap();
    let manager = Manager::new(history).unwrap();
    assert!(
        manager.snapshots()[0]
            .error
            .as_ref()
            .unwrap()
            .contains("Deletion was interrupted")
    );
    manager.delete(id);
    let failed = wait(&manager, id, |d| d.status == Status::Failed);
    assert!(
        failed
            .error
            .unwrap()
            .contains("recorded filename is invalid")
    );
    assert_eq!(std::fs::read_to_string(sentinel).unwrap(), "must survive");
    manager.shutdown();
}

#[cfg(windows)]
#[test]
fn a_locked_file_stays_in_history_until_device_deletion_can_be_retried() {
    use std::os::windows::fs::OpenOptionsExt;
    let server = Server::new();
    let directory = tempfile::tempdir().unwrap();
    let manager = Manager::new(directory.path().join("history.json")).unwrap();
    let id = manager
        .add(&server.url("/plain"), directory.path().into(), 1)
        .unwrap();
    let done = wait(&manager, id, |d| d.status == Status::Complete);
    let locked = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(done.path())
        .unwrap();
    manager.delete(id);
    let failed = wait(&manager, id, |d| d.status == Status::Failed);
    assert!(failed.error.unwrap().contains("Could not delete download"));
    assert!(done.path().exists());
    drop(locked);
    manager.delete(id);
    wait_removed(&manager, id);
    assert!(!done.path().exists());
    manager.shutdown();
}

#[test]
fn recovers_real_filenames_from_early_and_late_headers_redirects_and_types() {
    let server = Server::new();
    let directory = tempfile::tempdir().unwrap();
    let manager = Manager::new(directory.path().join("history.json")).unwrap();
    let guid = "0976f308-1892-4a34-aeca-7b155f43121e";
    for (route, expected) in [
        ("name-early", "Quarterly café report.pdf".to_string()),
        ("name-late", "Quarterly; final report.pdf".to_string()),
        ("name-redirect", "Readable-report.pdf".to_string()),
        ("type-only", format!("{guid}.pdf")),
    ] {
        let id = manager
            .add(
                &server.url(&format!("/{route}/{guid}")),
                directory.path().into(),
                8,
            )
            .unwrap();
        let done = wait(&manager, id, |d| {
            matches!(d.status, Status::Complete | Status::Failed)
        });
        assert_eq!(done.status, Status::Complete, "{:?}", done.error);
        assert_eq!(done.name, expected);
        assert_eq!(std::fs::read(done.path()).unwrap(), *server.bytes);
    }
    manager.settings.lock().unwrap().directory = directory.path().into();
    let id = manager
        .add_from_browser(
            &server.url(&format!("/name-early/{guid}")),
            Some("My chosen name.pdf"),
            None,
        )
        .unwrap();
    let done = wait(&manager, id, |d| {
        matches!(d.status, Status::Complete | Status::Failed)
    });
    assert_eq!(done.name, "My chosen name.pdf");
    assert_eq!(std::fs::read(done.path()).unwrap(), *server.bytes);
    let id = manager
        .add_from_browser(
            &server.url(&format!("/name-early/{guid}")),
            Some(guid),
            None,
        )
        .unwrap();
    let done = wait(&manager, id, |d| {
        matches!(d.status, Status::Complete | Status::Failed)
    });
    assert_eq!(done.name, "Quarterly café report (1).pdf");
    assert_eq!(std::fs::read(done.path()).unwrap(), *server.bytes);
    assert_eq!(
        std::fs::read(directory.path().join("Quarterly café report.pdf")).unwrap(),
        *server.bytes
    );
    manager.shutdown();
}

#[test]
fn discovered_filenames_survive_resume_and_do_not_change_metalink_entry_paths() {
    use sha2::Digest;
    let server = Server::new();
    let directory = tempfile::tempdir().unwrap();
    let history = directory.path().join("history.json");
    let guid = "0976f308-1892-4a34-aeca-7b155f43121e";
    let manager = Manager::new(history.clone()).unwrap();
    let id = manager
        .add(
            &server.url(&format!("/name-early/{guid}")),
            directory.path().into(),
            8,
        )
        .unwrap();
    wait(&manager, id, |d| d.downloaded > 64 * 1024);
    manager.pause(id);
    let paused = wait(&manager, id, |d| {
        matches!(d.status, Status::Paused | Status::Failed)
    });
    assert_eq!(paused.status, Status::Paused, "{:?}", paused.error);
    assert_eq!(paused.name, "Quarterly café report.pdf");
    manager.shutdown();
    let restored = Manager::new(history).unwrap();
    restored.resume(id);
    let done = wait(&restored, id, |d| {
        matches!(d.status, Status::Complete | Status::Failed)
    });
    assert_eq!(done.name, paused.name);
    assert_eq!(std::fs::read(done.path()).unwrap(), *server.bytes);
    let descriptor = directory.path().join("test.meta4");
    let digest = format!("{:x}", sha2::Sha256::digest(&*server.bytes));
    std::fs::write(&descriptor, format!("<metalink xmlns=\"urn:ietf:params:xml:ns:metalink\"><file name=\"mirror-payload.pdf\"><hash type=\"sha-256\">{digest}</hash><url>{}</url></file></metalink>", server.url(&format!("/name-early/{guid}")))).unwrap();
    let id = restored
        .add(descriptor.to_str().unwrap(), directory.path().into(), 8)
        .unwrap();
    let done = wait(&restored, id, |d| {
        matches!(d.status, Status::Complete | Status::Failed)
    });
    assert_eq!(done.status, Status::Complete, "{:?}", done.error);
    assert_eq!(
        std::fs::read(done.path().join("mirror-payload.pdf")).unwrap(),
        *server.bytes
    );
    assert_eq!(done.integrity.len(), 1);
    assert_eq!(
        done.integrity[0].path,
        std::path::Path::new("mirror-payload.pdf")
    );
    std::fs::remove_file(done.path().join("mirror-payload.pdf")).unwrap();
    wait(&restored, id, |d| d.status == Status::Missing);
    std::fs::write(done.path().join("mirror-payload.pdf"), &*server.bytes).unwrap();
    wait(&restored, id, |d| d.status == Status::Complete);
    restored.delete(id);
    wait_removed(&restored, id);
    assert!(!done.path().exists());
    assert!(!done.parts_dir().exists());
    assert!(descriptor.exists());
    restored.shutdown();
}
