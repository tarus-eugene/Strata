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
use strata::engine::{Manager, Status};

struct Server {
    address: String,
    bytes: Arc<Vec<u8>>,
    stop: Arc<AtomicBool>,
    gate: Arc<AtomicBool>,
    accepted: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<(usize, usize)>>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Server {
    fn new(size: usize, slow: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = format!("http://{}/file.bin", listener.local_addr().unwrap());
        let bytes = Arc::new((0..size).map(|i| (i % 251) as u8).collect::<Vec<_>>());
        let stop = Arc::new(AtomicBool::new(false));
        let gate = Arc::new(AtomicBool::new(slow));
        let accepted = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (b, s, g, c, a, p, r) = (
            bytes.clone(),
            stop.clone(),
            gate.clone(),
            accepted.clone(),
            active,
            peak.clone(),
            requests.clone(),
        );
        let handle = thread::spawn(move || {
            let mut clients = Vec::new();
            while !s.load(Ordering::SeqCst) {
                if let Ok((stream, _)) = listener.accept() {
                    c.fetch_add(1, Ordering::SeqCst);
                    let (b, s, g, a, p, r) = (
                        b.clone(),
                        s.clone(),
                        g.clone(),
                        a.clone(),
                        p.clone(),
                        r.clone(),
                    );
                    clients.push(thread::spawn(move || {
                        let _ = serve(stream, &b, &s, &g, &a, &p, &r, slow);
                    }));
                } else {
                    thread::sleep(Duration::from_millis(2));
                }
            }
            for client in clients {
                client.join().unwrap();
            }
        });
        Self {
            address,
            bytes,
            stop,
            gate,
            accepted,
            peak,
            requests,
            thread: Some(handle),
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.gate.store(false, Ordering::SeqCst);
        self.thread.take().unwrap().join().unwrap();
    }
}

#[allow(clippy::too_many_arguments)]
fn serve(
    mut stream: TcpStream,
    bytes: &[u8],
    stop: &AtomicBool,
    gate: &AtomicBool,
    active: &AtomicUsize,
    peak: &AtomicUsize,
    requests: &Mutex<Vec<(usize, usize)>>,
    slow: bool,
) -> std::io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_millis(200)))?;
    stream.set_write_timeout(Some(Duration::from_millis(500)))?;
    loop {
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            if stop.load(Ordering::SeqCst) {
                return Ok(());
            }
            let mut byte = [0];
            match stream.read(&mut byte) {
                Ok(0) => return Ok(()),
                Ok(_) => request.push(byte[0]),
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    continue;
                }
                Err(error) => return Err(error),
            }
            if request.len() > 16384 {
                return Ok(());
            }
        }
        let request = String::from_utf8_lossy(&request).to_ascii_lowercase();
        let range = request
            .lines()
            .find_map(|line| line.strip_prefix("range: bytes="))
            .unwrap();
        let (start, end) = range.split_once('-').unwrap();
        let start = start.parse::<usize>().unwrap();
        let end = end.parse::<usize>().unwrap();
        requests.lock().unwrap().push((start, end));
        let current = active.fetch_add(1, Ordering::SeqCst) + 1;
        peak.fetch_max(current, Ordering::SeqCst);
        let response: std::io::Result<()> = (|| {
            write!(
                stream,
                "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{end}/{}\r\nETag: \"chunks-v1\"\r\nConnection: keep-alive\r\n\r\n",
                end - start + 1,
                bytes.len()
            )?;
            let probe = start == 0 && end == 0;
            if slow && start > 0 && start < bytes.len() / 2 {
                while gate.load(Ordering::SeqCst) && !stop.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(5));
                }
            }
            for chunk in bytes[start..=end].chunks(32768) {
                if stop.load(Ordering::SeqCst) {
                    return Ok(());
                }
                stream.write_all(chunk)?;
                if slow && !probe {
                    thread::sleep(Duration::from_millis(if start == 0 { 40 } else { 4 }));
                }
            }
            Ok(())
        })();
        active.fetch_sub(1, Ordering::SeqCst);
        response?;
    }
}

fn wait(
    manager: &Manager,
    id: u64,
    condition: impl Fn(&strata::engine::Download) -> bool,
) -> strata::engine::Download {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let download = manager
            .snapshots()
            .into_iter()
            .find(|d| d.id == id)
            .unwrap();
        assert_ne!(download.status, Status::Failed, "{:?}", download.error);
        if condition(&download) {
            return download;
        }
        assert!(
            Instant::now() < deadline,
            "Timed out waiting for chunk progress"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn shared_queue_reuses_connections_for_more_chunks_than_workers() {
    let server = Server::new(24 * 1024 * 1024, false);
    let directory = tempfile::tempdir().unwrap();
    let manager = Manager::new(directory.path().join("history.json")).unwrap();
    let id = manager
        .add(&server.address, directory.path().into(), 2)
        .unwrap();
    let done = wait(&manager, id, |d| d.status == Status::Complete);
    assert_eq!(done.parts.len(), 6);
    assert_eq!(std::fs::read(done.path()).unwrap(), *server.bytes);
    assert!(server.requests.lock().unwrap().len() >= 7);
    assert!(
        server.accepted.load(Ordering::SeqCst) <= 2,
        "HTTP connections were not reused"
    );
    assert!(server.peak.load(Ordering::SeqCst) <= 2);
    manager.shutdown();
}

#[test]
fn slow_remaining_ranges_split_and_the_appended_plan_resumes_exactly() {
    let server = Server::new(8 * 1024 * 1024, true);
    let directory = tempfile::tempdir().unwrap();
    let history = directory.path().join("history.json");
    let manager = Manager::new(history.clone()).unwrap();
    let id = manager
        .add(&server.address, directory.path().into(), 2)
        .unwrap();
    let split = wait(&manager, id, |d| d.parts.len() > 2);
    assert_eq!(split.status, Status::Downloading);
    manager.pause(id);
    wait(&manager, id, |d| d.status == Status::Paused);
    manager.shutdown();
    server.gate.store(false, Ordering::SeqCst);
    let restored = Manager::new(history).unwrap();
    restored.resume(id);
    let done = wait(&restored, id, |d| d.status == Status::Complete);
    assert!(done.parts.len() > 2);
    assert_eq!(std::fs::read(done.path()).unwrap(), *server.bytes);
    assert!(
        server
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|(start, _)| *start > 0 && *start < server.bytes.len() / 2)
    );
    restored.shutdown();
}
