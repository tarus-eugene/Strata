use std::{
    io::{BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use strata::{
    engine::{Download, Manager, Status},
    protocol::TransferOptions,
};

fn wait(manager: &Manager, id: u64, predicate: impl Fn(&Download) -> bool) -> Download {
    let start = Instant::now();
    loop {
        let d = manager
            .snapshots()
            .into_iter()
            .find(|d| d.id == id)
            .unwrap();
        if predicate(&d) {
            return d;
        }
        assert!(
            start.elapsed() < Duration::from_secs(25),
            "Transfer timed out: {d:?}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

struct FtpServer {
    address: String,
    bytes: Arc<Vec<u8>>,
    offsets: Arc<Mutex<Vec<u64>>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl FtpServer {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("ftp://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let bytes = Arc::new(
            (0..4 * 1024 * 1024)
                .map(|n| (n % 251) as u8)
                .collect::<Vec<_>>(),
        );
        let offsets = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (payload, rest, stopping) = (bytes.clone(), offsets.clone(), stop.clone());
        let task = thread::spawn(move || {
            while !stopping.load(Ordering::SeqCst) {
                if let Ok((stream, _)) = listener.accept() {
                    let bytes = payload.clone();
                    let offsets = rest.clone();
                    thread::spawn(move || {
                        let _ = ftp_connection(stream, &bytes, &offsets);
                    });
                } else {
                    thread::sleep(Duration::from_millis(3));
                }
            }
        });
        Self {
            address,
            bytes,
            offsets,
            stop,
            thread: Some(task),
        }
    }
}
impl Drop for FtpServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.thread.take().unwrap().join();
    }
}

fn ftp_connection(
    mut stream: TcpStream,
    bytes: &[u8],
    offsets: &Mutex<Vec<u64>>,
) -> std::io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut passive = None;
    let mut offset = 0;
    let mut user = String::new();
    stream.write_all(b"220 Strata local test FTP\r\n")?;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let (command, value) = line
            .trim_end()
            .split_once(' ')
            .unwrap_or((line.trim_end(), ""));
        let response = match command {
            "USER" => {
                user = value.to_owned();
                "331 Password required\r\n".to_owned()
            }
            "PASS" => {
                if user == "anonymous" || (user == "alice" && value == "ftp-test-password") {
                    "230 Logged in\r\n".into()
                } else {
                    "530 Invalid credentials\r\n".into()
                }
            }
            "PWD" => "257 \"/\"\r\n".into(),
            "TYPE" | "CWD" => "200 OK\r\n".into(),
            "MDTM" => "213 20261030120000\r\n".into(),
            "SIZE" => format!("213 {}\r\n", bytes.len()),
            "REST" => {
                offset = value.parse::<u64>().unwrap();
                offsets.lock().unwrap().push(offset);
                "350 Restart accepted\r\n".into()
            }
            "EPSV" => {
                let listener = TcpListener::bind("127.0.0.1:0")?;
                let port = listener.local_addr()?.port();
                passive = Some(listener);
                format!("229 Entering Extended Passive Mode (|||{port}|)\r\n")
            }
            "RETR" => {
                stream.write_all(b"150 Opening data connection\r\n")?;
                let (mut data, _) = passive.take().unwrap().accept()?;
                data.set_write_timeout(Some(Duration::from_secs(3)))?;
                for chunk in bytes[offset as usize..].chunks(8192) {
                    data.write_all(chunk)?;
                    thread::sleep(Duration::from_millis(3));
                }
                drop(data);
                "226 Transfer complete\r\n".into()
            }
            "QUIT" => {
                stream.write_all(b"221 Bye\r\n")?;
                break;
            }
            _ => "502 Unsupported command\r\n".into(),
        };
        stream.write_all(response.as_bytes())?;
    }
    Ok(())
}

#[test]
fn ftp_anonymous_authenticated_and_rest_resume_are_byte_exact() {
    let server = FtpServer::new();
    let directory = tempfile::tempdir().unwrap();
    let history = directory.path().join("history.json");
    let manager = Manager::new(history.clone()).unwrap();
    let source = server
        .address
        .replace("ftp://", "ftp://alice:ftp-test-password@")
        + "/file.bin";
    let id = manager.add(&source, directory.path().into(), 8).unwrap();
    let active = wait(&manager, id, |d| {
        d.downloaded > 128 * 1024 || matches!(d.status, Status::Failed | Status::Complete)
    });
    assert_eq!(active.status, Status::Downloading, "{:?}", active.error);
    manager.pause(id);
    let paused = wait(&manager, id, |d| {
        matches!(d.status, Status::Paused | Status::Failed)
    });
    assert_eq!(paused.status, Status::Paused, "{:?}", paused.error);
    assert!(paused.downloaded > 0);
    manager.shutdown();
    drop(manager);
    assert!(
        !std::fs::read_to_string(&history)
            .unwrap()
            .contains("ftp-test-password")
    );
    let manager = Manager::new(history).unwrap();
    manager.resume(id);
    let done = wait(&manager, id, |d| {
        matches!(d.status, Status::Complete | Status::Failed)
    });
    assert_eq!(done.status, Status::Complete, "{:?}", done.error);
    assert_eq!(std::fs::read(done.path()).unwrap(), *server.bytes);
    assert!(
        server
            .offsets
            .lock()
            .unwrap()
            .iter()
            .any(|offset| *offset > 0)
    );
    let anonymous = manager
        .add(
            &(server.address.clone() + "/anonymous.bin"),
            directory.path().into(),
            1,
        )
        .unwrap();
    let done = wait(&manager, anonymous, |d| {
        matches!(d.status, Status::Complete | Status::Failed)
    });
    assert_eq!(done.status, Status::Complete, "{:?}", done.error);
    assert_eq!(std::fs::read(done.path()).unwrap(), *server.bytes);
    manager.shutdown();
}

struct SshHandler {
    channel: Option<russh::Channel<russh::server::Msg>>,
    bytes: Arc<Vec<u8>>,
    offsets: Arc<Mutex<Vec<u64>>>,
    key: russh::keys::PublicKey,
}
impl russh::server::Handler for SshHandler {
    type Error = anyhow::Error;
    async fn auth_password(
        &mut self,
        user: &str,
        password: &str,
    ) -> anyhow::Result<russh::server::Auth> {
        Ok(if user == "alice" && password == "sftp-test-password" {
            russh::server::Auth::Accept
        } else {
            russh::server::Auth::reject()
        })
    }
    async fn auth_publickey(
        &mut self,
        user: &str,
        key: russh::server::VerifiedPublicKey<'_>,
    ) -> anyhow::Result<russh::server::Auth> {
        Ok(if user == "alice" && key.public_key == &self.key {
            russh::server::Auth::Accept
        } else {
            russh::server::Auth::reject()
        })
    }
    async fn channel_open_session(
        &mut self,
        channel: russh::Channel<russh::server::Msg>,
        reply: russh::server::ChannelOpenHandle,
        _: &mut russh::server::Session,
    ) -> anyhow::Result<()> {
        self.channel = Some(channel);
        reply.accept().await;
        Ok(())
    }
    async fn subsystem_request(
        &mut self,
        id: russh::ChannelId,
        name: &str,
        session: &mut russh::server::Session,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(name == "sftp", "Unexpected subsystem");
        session.channel_success(id)?;
        let channel = self.channel.take().unwrap();
        let server = SftpHandler {
            bytes: self.bytes.clone(),
            offsets: self.offsets.clone(),
        };
        tokio::spawn(async move {
            russh_sftp::server::run(channel.into_stream(), server).await;
        });
        Ok(())
    }
}
struct SftpHandler {
    bytes: Arc<Vec<u8>>,
    offsets: Arc<Mutex<Vec<u64>>>,
}
impl SftpHandler {
    fn attributes(&self) -> russh_sftp::protocol::FileAttributes {
        russh_sftp::protocol::FileAttributes {
            size: Some(self.bytes.len() as u64),
            mtime: Some(100),
            ..Default::default()
        }
    }
}
impl russh_sftp::server::Handler for SftpHandler {
    type Error = russh_sftp::protocol::StatusCode;
    fn unimplemented(&self) -> Self::Error {
        Self::Error::OpUnsupported
    }
    async fn open(
        &mut self,
        id: u32,
        path: String,
        _: russh_sftp::protocol::OpenFlags,
        _: russh_sftp::protocol::FileAttributes,
    ) -> Result<russh_sftp::protocol::Handle, Self::Error> {
        Ok(russh_sftp::protocol::Handle { id, handle: path })
    }
    async fn stat(
        &mut self,
        id: u32,
        _: String,
    ) -> Result<russh_sftp::protocol::Attrs, Self::Error> {
        Ok(russh_sftp::protocol::Attrs {
            id,
            attrs: self.attributes(),
        })
    }
    async fn fstat(
        &mut self,
        id: u32,
        _: String,
    ) -> Result<russh_sftp::protocol::Attrs, Self::Error> {
        Ok(russh_sftp::protocol::Attrs {
            id,
            attrs: self.attributes(),
        })
    }
    async fn read(
        &mut self,
        id: u32,
        _: String,
        offset: u64,
        len: u32,
    ) -> Result<russh_sftp::protocol::Data, Self::Error> {
        if offset >= self.bytes.len() as u64 {
            return Err(Self::Error::Eof);
        }
        self.offsets.lock().unwrap().push(offset);
        tokio::time::sleep(Duration::from_millis(5)).await;
        let end = (offset as usize + len as usize).min(self.bytes.len());
        Ok(russh_sftp::protocol::Data {
            id,
            data: self.bytes[offset as usize..end].to_vec(),
        })
    }
    async fn close(
        &mut self,
        id: u32,
        _: String,
    ) -> Result<russh_sftp::protocol::Status, Self::Error> {
        Ok(russh_sftp::protocol::Status {
            id,
            status_code: Self::Error::Ok,
            error_message: String::new(),
            language_tag: String::new(),
        })
    }
}

#[test]
fn sftp_password_key_authentication_and_seek_resume_preserve_bytes() {
    use russh::keys::{PrivateKey, ssh_key::private::Ed25519Keypair};
    let directory = tempfile::tempdir().unwrap();
    let history = directory.path().join("history.json");
    let server_key = PrivateKey::from(Ed25519Keypair::from_seed(&[11; 32]));
    let user_key = PrivateKey::from(Ed25519Keypair::from_seed(&[12; 32]));
    let identity = directory.path().join("id_ed25519");
    std::fs::write(
        &identity,
        user_key
            .to_openssh(russh::keys::ssh_key::LineEnding::LF)
            .unwrap()
            .as_bytes(),
    )
    .unwrap();
    let bytes = Arc::new(
        (0..4 * 1024 * 1024)
            .map(|n| (n % 251) as u8)
            .collect::<Vec<_>>(),
    );
    let offsets = Arc::new(Mutex::new(Vec::new()));
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let listener = runtime
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let known = directory.path().join("known_hosts");
    russh::keys::known_hosts::learn_known_hosts_path(
        "127.0.0.1",
        port,
        server_key.public_key(),
        &known,
    )
    .unwrap();
    let config = Arc::new(russh::server::Config {
        keys: vec![server_key],
        auth_rejection_time: Duration::ZERO,
        auth_rejection_time_initial: Some(Duration::ZERO),
        ..Default::default()
    });
    let (payload, reads) = (bytes.clone(), offsets.clone());
    let public_key = user_key.public_key().clone();
    let task = runtime.spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let handler = SshHandler {
                channel: None,
                bytes: payload.clone(),
                offsets: reads.clone(),
                key: public_key.clone(),
            };
            let config = config.clone();
            tokio::spawn(async move {
                if let Ok(session) = russh::server::run_stream(config, stream, handler).await {
                    let _ = session.await;
                }
            });
        }
    });
    let manager = Manager::new(history.clone()).unwrap();
    let options = TransferOptions {
        known_hosts: Some(known.clone()),
        ..Default::default()
    };
    let id = manager
        .add_with_options(
            &format!("sftp://alice:sftp-test-password@127.0.0.1:{port}/file.bin"),
            directory.path().into(),
            1,
            options,
        )
        .unwrap();
    let active = wait(&manager, id, |d| {
        d.downloaded > 128 * 1024 || matches!(d.status, Status::Complete | Status::Failed)
    });
    assert_eq!(active.status, Status::Downloading, "{:?}", active.error);
    manager.pause(id);
    let paused = wait(&manager, id, |d| {
        matches!(d.status, Status::Paused | Status::Failed)
    });
    assert_eq!(paused.status, Status::Paused, "{:?}", paused.error);
    manager.shutdown();
    drop(manager);
    offsets.lock().unwrap().clear();
    assert!(
        !std::fs::read_to_string(&history)
            .unwrap()
            .contains("sftp-test-password")
    );
    let manager = Manager::new(history).unwrap();
    manager.resume(id);
    let done = wait(&manager, id, |d| {
        matches!(d.status, Status::Complete | Status::Failed)
    });
    assert_eq!(done.status, Status::Complete, "{:?}", done.error);
    assert_eq!(std::fs::read(done.path()).unwrap(), *bytes);
    assert!(
        offsets
            .lock()
            .unwrap()
            .first()
            .is_some_and(|offset| *offset > 0)
    );
    let options = TransferOptions {
        identity: Some(identity),
        known_hosts: Some(known),
        ..Default::default()
    };
    let id = manager
        .add_with_options(
            &format!("sftp://alice@127.0.0.1:{port}/key-file.bin"),
            directory.path().into(),
            1,
            options,
        )
        .unwrap();
    let done = wait(&manager, id, |d| {
        matches!(d.status, Status::Complete | Status::Failed)
    });
    assert_eq!(done.status, Status::Complete, "{:?}", done.error);
    assert_eq!(std::fs::read(done.path()).unwrap(), *bytes);
    manager.shutdown();
    task.abort();
}
