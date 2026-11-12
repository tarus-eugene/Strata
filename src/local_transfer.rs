use crate::engine::Manager;
use anyhow::{Context, Result, ensure};
use russh::{Channel, Disconnect, client, keys, server};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

mod folder;
use tokio::{
    fs::File,
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
    runtime::Handle,
    sync::{Semaphore, mpsc, oneshot, watch},
    task::{JoinHandle, JoinSet},
    time::{Instant, timeout},
};

pub const PORT: u16 = 46275;
const USER: &str = "strata-transfer-v1";
const FOLDER_USER: &str = "strata-folder-transfer-v1";
const DISCOVERY: &[u8] = b"strata-transfer-v1 discovery";
const SETUP_TIMEOUT: Duration = Duration::from_secs(10);
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(300);
const IO_TIMEOUT: Duration = Duration::from_secs(45);
const MAX_OFFER: usize = 4096;
const ACCEPT: u8 = 1;
const DECLINE: u8 = 2;
const SAVED: u8 = 3;
const FAILED: u8 = 4;

#[derive(Clone, Debug)]
pub struct Target {
    pub address: Ipv4Addr,
    pub name: String,
    fingerprint: Option<String>,
}

impl Target {
    pub fn manual(address: &str) -> Result<Self> {
        let address = address
            .trim()
            .parse()
            .context("Enter the target computer's Ethernet IPv4 address")?;
        ensure!(
            local_address(address),
            "Enter a private or link-local IPv4 address"
        );
        Ok(Self {
            address,
            name: address.to_string(),
            fingerprint: None,
        })
    }
}

#[derive(Clone, Debug)]
pub enum State {
    Connecting,
    Waiting(String),
    Transferring { bytes: u64, total: u64 },
    Finished(Option<PathBuf>),
    Declined,
    Failed(String),
}

pub struct Operation {
    progress: watch::Receiver<State>,
    _cancel: oneshot::Sender<()>,
}

impl Operation {
    pub fn state(&self) -> State {
        let state = self.progress.borrow().clone();
        if matches!(
            state,
            State::Connecting | State::Waiting(_) | State::Transferring { .. }
        ) && self.progress.has_changed().is_err()
        {
            return State::Failed("The other computer disconnected".into());
        }
        state
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Offer {
    pub name: String,
    pub size: u64,
    pub sender: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_entries: Option<u32>,
}

pub struct Incoming {
    pub offer: Offer,
    pub address: SocketAddr,
    pub operation: Operation,
    decision: Option<oneshot::Sender<PathBuf>>,
}

impl Incoming {
    pub fn accept(&mut self, directory: PathBuf, name: &str) -> Result<()> {
        folder::validate_component(name)?;
        ensure!(
            !directory.as_os_str().is_empty() && directory.is_dir(),
            "Choose an existing download folder"
        );
        let path = std::path::absolute(directory)?.join(name);
        ensure!(
            !path.try_exists()?,
            "The destination already exists. Choose another name or folder."
        );
        self.decision
            .take()
            .context("This transfer is no longer awaiting approval")?
            .send(path)
            .map_err(|_| anyhow::anyhow!("The sender is no longer connected"))
    }

    pub fn decline(&mut self) {
        self.decision.take();
    }
}

#[derive(Serialize, Deserialize)]
struct Announcement {
    name: String,
    fingerprint: String,
}

pub struct Service {
    runtime: Handle,
    incoming: mpsc::Receiver<Incoming>,
    listener: Option<JoinHandle<()>>,
    discovery: Option<JoinHandle<()>>,
    identity: Option<String>,
    pub receive_error: Option<String>,
}

impl Service {
    pub fn start(manager: &Manager) -> Self {
        Self::bind(manager.runtime_handle(), Ipv4Addr::UNSPECIFIED, PORT)
    }

    fn bind(runtime: Handle, ip: Ipv4Addr, port: u16) -> Self {
        let (sender, incoming) = mpsc::channel(4);
        let mut service = Self {
            runtime,
            incoming,
            listener: None,
            discovery: None,
            identity: None,
            receive_error: None,
        };
        if let Err(error) = service.listen((ip, port), sender) {
            service.receive_error = Some(format!("Incoming transfers unavailable: {error:#}"));
        }
        service
    }

    fn listen(&mut self, address: (Ipv4Addr, u16), incoming: mpsc::Sender<Incoming>) -> Result<()> {
        let key = keys::PrivateKey::random(&mut rand::rng(), keys::Algorithm::Ed25519)?;
        let fingerprint = fingerprint(key.public_key());
        let config = Arc::new(server::Config {
            keys: vec![key],
            inactivity_timeout: Some(APPROVAL_TIMEOUT + IO_TIMEOUT),
            ..Default::default()
        });
        let _entered = self.runtime.enter();
        let tcp = std::net::TcpListener::bind(address)
            .context("Could not open the local transfer port")?;
        tcp.set_nonblocking(true)?;
        let tcp = TcpListener::from_std(tcp)?;
        let port = tcp.local_addr()?.port();
        if let Ok(udp) = std::net::UdpSocket::bind((address.0, port))
            && udp.set_nonblocking(true).is_ok()
            && let Ok(udp) = UdpSocket::from_std(udp)
        {
            let info = serde_json::to_vec(&Announcement {
                name: computer_name(),
                fingerprint: fingerprint.clone(),
            })?;
            self.discovery = Some(self.runtime.spawn(announce(udp, info)));
        }
        self.listener = Some(
            self.runtime
                .spawn(listen(tcp, config, code(&fingerprint), incoming)),
        );
        self.identity = Some(fingerprint);
        Ok(())
    }

    pub fn discover(&self) -> JoinHandle<Result<Vec<Target>>> {
        let identity = self.identity.clone();
        self.runtime
            .spawn(async move { discover((Ipv4Addr::BROADCAST, PORT), identity.as_deref()).await })
    }

    pub fn next_incoming(&mut self) -> Option<Incoming> {
        self.incoming.try_recv().ok()
    }

    pub fn send(&self, target: Target, path: PathBuf) -> Operation {
        let (progress, cancel, operation) = operation(State::Connecting);
        let identity = self.identity.clone();
        self.runtime.spawn(async move {
            let result = tokio::select! {
                biased;
                _ = cancel => return,
                result = send(target, path, PORT, identity, &progress) => result,
            };
            publish(&progress, result);
        });
        operation
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        for task in [self.listener.take(), self.discovery.take()]
            .into_iter()
            .flatten()
        {
            task.abort();
        }
    }
}

fn operation(initial: State) -> (watch::Sender<State>, oneshot::Receiver<()>, Operation) {
    let (progress, state) = watch::channel(initial);
    let (cancel, cancelled) = oneshot::channel();
    (
        progress,
        cancelled,
        Operation {
            progress: state,
            _cancel: cancel,
        },
    )
}

fn publish(progress: &watch::Sender<State>, result: Result<State>) {
    progress.send_replace(result.unwrap_or_else(|error| State::Failed(format!("{error:#}"))));
}

fn local_address(ip: Ipv4Addr) -> bool {
    ip.is_private() || ip.is_link_local() || ip.is_loopback()
}

fn local_peer(address: SocketAddr) -> bool {
    matches!(address.ip(), IpAddr::V4(ip) if local_address(ip))
}

fn clean_name(name: &str) -> String {
    name.chars()
        .filter(|ch| {
            !ch.is_control() && !matches!(ch, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .take(64)
        .collect()
}

fn computer_name() -> String {
    std::env::var("COMPUTERNAME")
        .ok()
        .map(|name| clean_name(&name))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "101 Downloader".into())
}

fn fingerprint(key: &keys::PublicKey) -> String {
    key.fingerprint(keys::HashAlg::Sha256).to_string()
}

fn code(fingerprint: &str) -> String {
    fingerprint
        .trim_start_matches("SHA256:")
        .chars()
        .take(8)
        .collect()
}

async fn announce(socket: UdpSocket, info: Vec<u8>) {
    let mut buffer = [0; 128];
    while let Ok((len, source)) = socket.recv_from(&mut buffer).await {
        if local_peer(source) && &buffer[..len] == DISCOVERY {
            let _ = socket.send_to(&info, source).await;
        }
    }
}

async fn discover(destination: (Ipv4Addr, u16), identity: Option<&str>) -> Result<Vec<Target>> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await?;
    socket.set_broadcast(true)?;
    socket.send_to(DISCOVERY, destination).await?;
    let mut peers = BTreeMap::new();
    let mut buffer = [0; 1024];
    let deadline = Instant::now() + Duration::from_millis(900);
    while let Ok(response) = tokio::time::timeout_at(deadline, socket.recv_from(&mut buffer)).await
    {
        let (size, source) = response?;
        let IpAddr::V4(address) = source.ip() else {
            continue;
        };
        if source.port() == destination.1
            && local_address(address)
            && let Ok(info) = serde_json::from_slice::<Announcement>(&buffer[..size])
            && info.fingerprint.starts_with("SHA256:")
            && info.fingerprint.len() == 50
            && identity != Some(info.fingerprint.as_str())
        {
            peers.insert(
                address,
                Target {
                    address,
                    name: clean_name(&info.name),
                    fingerprint: Some(info.fingerprint),
                },
            );
            if peers.len() == 32 {
                break;
            }
        }
    }
    Ok(peers.into_values().collect())
}

type ServerChannel = Channel<server::Msg>;

struct Receiver {
    channel: Option<oneshot::Sender<(ServerChannel, bool)>>,
    folder: bool,
}

impl server::Handler for Receiver {
    type Error = anyhow::Error;

    async fn auth_none(&mut self, user: &str) -> Result<server::Auth> {
        self.folder = user == FOLDER_USER;
        Ok(if user == USER || self.folder {
            server::Auth::Accept
        } else {
            server::Auth::reject()
        })
    }

    async fn channel_open_session(
        &mut self,
        channel: ServerChannel,
        reply: server::ChannelOpenHandle,
        _: &mut server::Session,
    ) -> Result<()> {
        if let Some(deliver) = self.channel.take() {
            reply.accept().await;
            let _ = deliver.send((channel, self.folder));
        }
        Ok(())
    }
}

async fn listen(
    tcp: TcpListener,
    config: Arc<server::Config>,
    code: String,
    incoming: mpsc::Sender<Incoming>,
) {
    let slots = Arc::new(Semaphore::new(8));
    let mut sessions = JoinSet::new();
    loop {
        tokio::select! {
            _ = sessions.join_next(), if !sessions.is_empty() => {},
            connection = tcp.accept() => {
                let Ok((socket, address)) = connection else { break };
                if !local_peer(address) { continue; }
                let Ok(slot) = slots.clone().try_acquire_owned() else { continue };
                let (config, code, incoming) = (config.clone(), code.clone(), incoming.clone());
                sessions.spawn(async move {
                    let _slot = slot;
                    let _ = receive_connection(config, socket, address, code, incoming).await;
                });
            }
        }
    }
}

async fn receive_connection(
    config: Arc<server::Config>,
    socket: TcpStream,
    address: SocketAddr,
    code: String,
    incoming: mpsc::Sender<Incoming>,
) -> Result<()> {
    let (deliver, channel) = oneshot::channel();
    let mut session = timeout(
        SETUP_TIMEOUT,
        server::run_stream(
            config,
            socket,
            Receiver {
                channel: Some(deliver),
                folder: false,
            },
        ),
    )
    .await??;
    let disconnect = session.handle();
    tokio::select! {
        _ = &mut session => {},
        _ = async {
            if let Ok(Ok((channel, folder))) = timeout(SETUP_TIMEOUT, channel).await {
                let _ = receive(channel.into_stream(), address, code, incoming, folder).await;
            }
        } => {
            let _ = timeout(SETUP_TIMEOUT, disconnect.disconnect(Disconnect::ByApplication, "Transfer closed".into(), String::new())).await;
        }
    }
    Ok(())
}

async fn receive(
    mut stream: impl AsyncRead + AsyncWrite + Unpin,
    address: SocketAddr,
    code: String,
    incoming: mpsc::Sender<Incoming>,
    folder_session: bool,
) -> Result<()> {
    let mut offer: Offer = timeout(SETUP_TIMEOUT, read_metadata(&mut stream, MAX_OFFER)).await??;
    folder::validate_component(&offer.name)?;
    let folder_entries = offer.folder_entries;
    ensure!(
        folder_entries.is_some() == folder_session,
        "Transfer kind does not match the connection"
    );
    if let Some(entries) = folder_entries {
        folder::validate_offer(entries, offer.size)?;
    }
    offer.sender = clean_name(&offer.sender);
    let size = offer.size;
    let (progress, mut cancel, operation) = operation(State::Waiting(code));
    let (decision, choice) = oneshot::channel();
    if incoming
        .try_send(Incoming {
            offer,
            address,
            operation,
            decision: Some(decision),
        })
        .is_err()
    {
        stream.write_u8(DECLINE).await?;
        return Ok(());
    }

    let choice = tokio::select! {
        biased;
        _ = stream.read_u8() => {
            publish(&progress, Err(anyhow::anyhow!("The sender disconnected")));
            return Ok(());
        }
        choice = timeout(APPROVAL_TIMEOUT, choice) => choice.ok().and_then(Result::ok),
    };
    let Some(path) = choice else {
        stream.write_u8(DECLINE).await?;
        stream.flush().await?;
        progress.send_replace(State::Declined);
        return Ok(());
    };
    let result = tokio::select! {
        _ = &mut cancel => Err(anyhow::anyhow!("Transfer cancelled")),
        result = async {
            match folder_entries {
                Some(entries) => folder::receive(&mut stream, &path, entries, size, &progress).await,
                None => receive_file(&mut stream, &path, size, &progress).await,
            }
        } => result,
    };
    let reply = if result.is_ok() { SAVED } else { FAILED };
    publish(&progress, result);
    let _ = timeout(SETUP_TIMEOUT, async {
        stream.write_u8(reply).await?;
        stream.flush().await
    })
    .await;
    Ok(())
}

async fn receive_file(
    stream: &mut (impl AsyncRead + AsyncWrite + Unpin),
    path: &Path,
    size: u64,
    progress: &watch::Sender<State>,
) -> Result<State> {
    let parent = path.parent().context("Missing destination folder")?;
    let temp = tempfile::Builder::new()
        .prefix(".strata-transfer-")
        .tempfile_in(parent)?;
    let mut file = File::from_std(temp.reopen()?);
    stream.write_u8(ACCEPT).await?;
    stream.flush().await?;
    let mut transfer = TransferProgress::new(size, progress);
    receive_verified_file(stream, &mut file, size, &mut transfer).await?;
    drop(file);
    temp.persist_noclobber(path)
        .context("Could not save the file at the chosen destination")?;
    Ok(State::Finished(Some(path.to_owned())))
}

struct Sender {
    expected: Option<String>,
    own: Option<String>,
    actual: Arc<Mutex<Option<String>>>,
}

impl client::Handler for Sender {
    type Error = anyhow::Error;

    async fn check_server_key(&mut self, key: &keys::PublicKeyOrCertificate) -> Result<bool> {
        let fingerprint = fingerprint(&key.public_key());
        ensure!(
            self.own.as_ref() != Some(&fingerprint),
            "Choose another computer as the target"
        );
        ensure!(
            self.expected
                .as_ref()
                .is_none_or(|expected| *expected == fingerprint),
            "The target's identity changed. Refresh the device list and select it again."
        );
        *self.actual.lock().unwrap() = Some(fingerprint);
        Ok(true)
    }
}

async fn send(
    target: Target,
    path: PathBuf,
    port: u16,
    own: Option<String>,
    progress: &watch::Sender<State>,
) -> Result<State> {
    let metadata = tokio::fs::metadata(&path)
        .await
        .context("Could not open the selected item")?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("Choose a named file or folder")?
        .to_owned();
    folder::validate_component(&name)?;
    let source = if metadata.is_dir() {
        Source::Folder(folder::Folder::scan(path).await?)
    } else {
        let file = File::open(&path)
            .await
            .context("Could not open the selected file")?;
        let metadata = file.metadata().await?;
        ensure!(metadata.is_file(), "Choose a regular file or folder");
        Source::File(file, metadata.len())
    };
    let offer = Offer {
        name,
        size: source.size(),
        sender: computer_name(),
        folder_entries: source.folder_entries(),
    };
    let actual = Arc::new(Mutex::new(None));
    let handler = Sender {
        expected: target.fingerprint,
        own,
        actual: actual.clone(),
    };
    let mut session = timeout(SETUP_TIMEOUT, client::connect(Arc::new(client::Config::default()), (target.address, port), handler))
        .await.context("The target did not respond. Check the Ethernet connection and whether 101 Downloader is open there.")??;
    let result = async {
        let folder = offer.folder_entries.is_some();
        let user = if folder { FOLDER_USER } else { USER };
        let auth = timeout(SETUP_TIMEOUT, session.authenticate_none(user)).await??;
        ensure!(
            auth.success(),
            "{}",
            if folder {
                "The target rejected folder sharing. Use the updated app on both computers."
            } else {
                "The target rejected the transfer connection"
            }
        );
        let channel = timeout(SETUP_TIMEOUT, session.channel_open_session()).await??;
        let fingerprint = actual
            .lock()
            .unwrap()
            .take()
            .context("Missing target identity")?;
        let mut stream = channel.into_stream();
        write_metadata(&mut stream, &offer, MAX_OFFER).await?;
        progress.send_replace(State::Waiting(code(&fingerprint)));
        match timeout(APPROVAL_TIMEOUT + SETUP_TIMEOUT, stream.read_u8())
            .await
            .context("The target did not answer the transfer request")??
        {
            DECLINE => return Ok(State::Declined),
            ACCEPT => {}
            _ => anyhow::bail!("The target could not prepare the destination"),
        }
        let mut transfer = TransferProgress::new(offer.size, progress);
        match source {
            Source::File(file, size) => {
                send_verified_file(&mut stream, file, size, &mut transfer).await?
            }
            Source::Folder(folder) => folder.send(&mut stream, &mut transfer).await?,
        }
        timeout(IO_TIMEOUT, stream.flush()).await??;
        ensure!(
            timeout(IO_TIMEOUT, stream.read_u8()).await?? == SAVED,
            "The target could not verify or save the transfer"
        );
        Ok(State::Finished(None))
    }
    .await;
    let _ = timeout(
        SETUP_TIMEOUT,
        session.disconnect(Disconnect::ByApplication, "Transfer closed", ""),
    )
    .await;
    result
}

enum Source {
    File(File, u64),
    Folder(folder::Folder),
}

impl Source {
    fn size(&self) -> u64 {
        match self {
            Self::File(_, size) => *size,
            Self::Folder(folder) => folder.size(),
        }
    }

    fn folder_entries(&self) -> Option<u32> {
        match self {
            Self::File(..) => None,
            Self::Folder(folder) => Some(folder.entry_count()),
        }
    }
}

async fn read_metadata<T: DeserializeOwned>(
    stream: &mut (impl AsyncRead + Unpin),
    limit: usize,
) -> Result<T> {
    let size = stream.read_u32().await? as usize;
    ensure!(
        (1..=limit).contains(&size),
        "Invalid transfer metadata size"
    );
    let mut data = vec![0; size];
    stream.read_exact(&mut data).await?;
    Ok(serde_json::from_slice(&data)?)
}

async fn write_metadata(
    stream: &mut (impl AsyncWrite + Unpin),
    value: &impl Serialize,
    limit: usize,
) -> Result<()> {
    let data = serde_json::to_vec(value)?;
    ensure!(data.len() <= limit, "The transfer metadata is too large");
    timeout(IO_TIMEOUT, async {
        stream.write_u32(data.len() as u32).await?;
        stream.write_all(&data).await?;
        stream.flush().await
    })
    .await??;
    Ok(())
}

async fn send_verified_file(
    stream: &mut (impl AsyncWrite + Unpin),
    file: File,
    size: u64,
    progress: &mut TransferProgress<'_>,
) -> Result<()> {
    let digest = copy_file(&mut file.take(size), stream, size, progress).await?;
    timeout(IO_TIMEOUT, stream.write_all(&digest)).await??;
    Ok(())
}

async fn receive_verified_file(
    stream: &mut (impl AsyncRead + Unpin),
    file: &mut File,
    size: u64,
    progress: &mut TransferProgress<'_>,
) -> Result<()> {
    let digest = copy_file(&mut (&mut *stream).take(size), file, size, progress).await?;
    let mut expected = [0; 32];
    timeout(IO_TIMEOUT, stream.read_exact(&mut expected)).await??;
    ensure!(
        digest == expected,
        "The received file failed its integrity check"
    );
    file.sync_all().await?;
    Ok(())
}

struct TransferProgress<'a> {
    bytes: u64,
    total: u64,
    status: &'a watch::Sender<State>,
    buffer: Vec<u8>,
}

impl<'a> TransferProgress<'a> {
    fn new(total: u64, status: &'a watch::Sender<State>) -> Self {
        status.send_replace(State::Transferring { bytes: 0, total });
        Self {
            bytes: 0,
            total,
            status,
            buffer: vec![0; 128 * 1024],
        }
    }
}

async fn copy_file(
    reader: &mut (impl AsyncRead + Unpin),
    writer: &mut (impl AsyncWrite + Unpin),
    size: u64,
    progress: &mut TransferProgress<'_>,
) -> Result<[u8; 32]> {
    let mut sha = Sha256::new();
    let mut transferred = 0;
    loop {
        let count = timeout(IO_TIMEOUT, reader.read(&mut progress.buffer)).await??;
        if count == 0 {
            break;
        }
        sha.update(&progress.buffer[..count]);
        timeout(IO_TIMEOUT, writer.write_all(&progress.buffer[..count])).await??;
        transferred += count as u64;
        progress.bytes += count as u64;
        progress.status.send_replace(State::Transferring {
            bytes: progress.bytes,
            total: progress.total,
        });
    }
    ensure!(
        transferred == size,
        "The file ended before the advertised size"
    );
    Ok(sha.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn loopback() -> (u16, mpsc::Receiver<Incoming>, JoinHandle<()>) {
        let key = keys::PrivateKey::random(&mut rand::rng(), keys::Algorithm::Ed25519).unwrap();
        let code = code(&fingerprint(key.public_key()));
        let config = Arc::new(server::Config {
            keys: vec![key],
            ..Default::default()
        });
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (offers, incoming) = mpsc::channel(1);
        let listener = tokio::spawn(listen(listener, config, code, offers));
        (port, incoming, listener)
    }

    #[tokio::test]
    async fn transfer_waits_for_approval_then_saves_the_selected_name() {
        let (port, mut incoming, listener) = loopback().await;
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let original = source.path().join("sample.bin");
        let payload = (0..300_000).map(|n| (n % 251) as u8).collect::<Vec<_>>();
        std::fs::write(&original, &payload).unwrap();
        let (progress, _, _) = operation(State::Connecting);
        let target = Target::manual("127.0.0.1").unwrap();
        let sender =
            tokio::spawn(async move { send(target, original, port, None, &progress).await });
        let mut offer = timeout(SETUP_TIMEOUT, incoming.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(offer.offer.name, "sample.bin");
        assert_eq!(offer.offer.size, payload.len() as u64);
        assert!(matches!(offer.operation.state(), State::Waiting(_)));
        assert!(!sender.is_finished());
        assert!(
            std::fs::read_dir(destination.path())
                .unwrap()
                .next()
                .is_none()
        );
        assert!(
            offer
                .accept(destination.path().to_owned(), "../escape")
                .is_err()
        );
        let existing = destination.path().join("existing.bin");
        std::fs::write(&existing, b"keep").unwrap();
        assert!(
            offer
                .accept(destination.path().to_owned(), "existing.bin")
                .is_err()
        );
        offer
            .accept(destination.path().to_owned(), "received.bin")
            .unwrap();
        let result = timeout(SETUP_TIMEOUT, sender)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(matches!(result, State::Finished(None)));
        assert_eq!(
            std::fs::read(destination.path().join("received.bin")).unwrap(),
            payload
        );
        assert_eq!(std::fs::read(existing).unwrap(), b"keep");
        listener.abort();
    }

    #[tokio::test]
    async fn folder_transfer_preserves_nested_and_empty_items() {
        let (port, mut incoming, listener) = loopback().await;
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let folder = source.path().join("shared.docs");
        std::fs::create_dir_all(folder.join("sub")).unwrap();
        std::fs::create_dir(folder.join("empty")).unwrap();
        let nested = (0..300_000).map(|n| (n % 251) as u8).collect::<Vec<_>>();
        std::fs::write(folder.join("sub").join("café.bin"), &nested).unwrap();
        std::fs::write(folder.join("sub").join("empty.bin"), []).unwrap();
        std::fs::write(folder.join("header.txt"), b"header").unwrap();

        let (progress, _, _) = operation(State::Connecting);
        let target = Target::manual("127.0.0.1").unwrap();
        let sender = tokio::spawn(async move { send(target, folder, port, None, &progress).await });
        let mut offer = timeout(SETUP_TIMEOUT, incoming.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(offer.offer.name, "shared.docs");
        assert_eq!(offer.offer.folder_entries, Some(5));
        assert_eq!(offer.offer.size, nested.len() as u64 + 6);
        assert!(matches!(offer.operation.state(), State::Waiting(_)));
        assert!(
            std::fs::read_dir(destination.path())
                .unwrap()
                .next()
                .is_none()
        );

        offer.accept(destination.path().into(), "received").unwrap();
        let result = timeout(SETUP_TIMEOUT, sender)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(matches!(result, State::Finished(None)));
        let received = destination.path().join("received");
        assert_eq!(
            std::fs::read(received.join("sub").join("café.bin")).unwrap(),
            nested
        );
        assert_eq!(
            std::fs::read(received.join("sub").join("empty.bin")).unwrap(),
            b""
        );
        assert_eq!(
            std::fs::read(received.join("header.txt")).unwrap(),
            b"header"
        );
        assert!(
            std::fs::read_dir(received.join("empty"))
                .unwrap()
                .next()
                .is_none()
        );
        assert_eq!(std::fs::read_dir(destination.path()).unwrap().count(), 1);
        listener.abort();
    }

    #[test]
    fn legacy_file_offers_keep_their_wire_format() {
        let data = r#"{"name":"test.txt","size":100,"sender":"Test"}"#;
        let offer: Offer = serde_json::from_str(data).unwrap();
        assert!(offer.folder_entries.is_none());
        assert_eq!(serde_json::to_string(&offer).unwrap(), data);
    }

    #[tokio::test]
    async fn declined_transfer_sends_no_file() {
        let (port, mut incoming, listener) = loopback().await;
        let source = tempfile::tempdir().unwrap();
        let file = source.path().join("sample.bin");
        std::fs::write(&file, b"sample").unwrap();
        let (progress, _, _) = operation(State::Connecting);
        let target = Target::manual("127.0.0.1").unwrap();
        let sender = tokio::spawn(async move { send(target, file, port, None, &progress).await });
        let mut offer = timeout(SETUP_TIMEOUT, incoming.recv())
            .await
            .unwrap()
            .unwrap();
        offer.decline();
        let result = timeout(SETUP_TIMEOUT, sender)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(matches!(result, State::Declined));
        listener.abort();
    }

    #[tokio::test]
    async fn cancelled_sender_releases_the_receiver_dialog() {
        let (port, mut incoming, listener) = loopback().await;
        let source = tempfile::tempdir().unwrap();
        let file = source.path().join("sample.bin");
        std::fs::write(&file, b"sample").unwrap();
        let (progress, _, _) = operation(State::Connecting);
        let target = Target::manual("127.0.0.1").unwrap();
        let sender = tokio::spawn(async move { send(target, file, port, None, &progress).await });
        let offer = timeout(SETUP_TIMEOUT, incoming.recv())
            .await
            .unwrap()
            .unwrap();
        sender.abort();
        timeout(Duration::from_secs(3), async {
            while matches!(offer.operation.state(), State::Waiting(_)) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(matches!(offer.operation.state(), State::Failed(_)));
        listener.abort();
    }

    #[tokio::test]
    async fn simultaneous_offers_can_be_approved_in_both_directions() {
        let (left_port, mut left_incoming, left_listener) = loopback().await;
        let (right_port, mut right_incoming, right_listener) = loopback().await;
        let left = tempfile::tempdir().unwrap();
        let right = tempfile::tempdir().unwrap();
        let left_source = left.path().join("left.bin");
        let right_source = right.path().join("right.bin");
        std::fs::write(&left_source, b"left").unwrap();
        std::fs::write(&right_source, b"right").unwrap();
        let target = Target::manual("127.0.0.1").unwrap();
        let (left_progress, _, _) = operation(State::Connecting);
        let (right_progress, _, _) = operation(State::Connecting);
        let left_target = target.clone();
        let left_send = tokio::spawn(async move {
            send(left_target, left_source, right_port, None, &left_progress).await
        });
        let right_send = tokio::spawn(async move {
            send(target, right_source, left_port, None, &right_progress).await
        });
        let (mut left_offer, mut right_offer) = timeout(SETUP_TIMEOUT, async {
            let (left, right) = tokio::join!(left_incoming.recv(), right_incoming.recv());
            (left.unwrap(), right.unwrap())
        })
        .await
        .unwrap();
        left_offer
            .accept(left.path().into(), "received.bin")
            .unwrap();
        right_offer
            .accept(right.path().into(), "received.bin")
            .unwrap();
        let (left_sent, right_sent) =
            timeout(SETUP_TIMEOUT, async { tokio::join!(left_send, right_send) })
                .await
                .unwrap();
        assert!(matches!(left_sent.unwrap().unwrap(), State::Finished(None)));
        assert!(matches!(
            right_sent.unwrap().unwrap(),
            State::Finished(None)
        ));
        assert_eq!(
            std::fs::read(left.path().join("received.bin")).unwrap(),
            b"right"
        );
        assert_eq!(
            std::fs::read(right.path().join("received.bin")).unwrap(),
            b"left"
        );
        left_listener.abort();
        right_listener.abort();
    }

    #[tokio::test]
    async fn invalid_payload_is_not_published() {
        let folder = tempfile::tempdir().unwrap();
        let destination = folder.path().join("broken.bin");
        let (mut sender, mut receiver) = tokio::io::duplex(1024);
        let (progress, _, _) = operation(State::Connecting);
        let (received, sent) = tokio::join!(
            receive_file(&mut receiver, &destination, 3, &progress),
            async {
                assert_eq!(sender.read_u8().await.unwrap(), ACCEPT);
                sender.write_all(b"abc").await.unwrap();
                sender.write_all(&[0; 32]).await
            }
        );
        sent.unwrap();
        assert!(received.unwrap_err().to_string().contains("integrity"));
        assert!(std::fs::read_dir(folder.path()).unwrap().next().is_none());
    }

    #[tokio::test]
    async fn discovery_returns_the_target_identity_and_ignores_self() {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = (Ipv4Addr::LOCALHOST, socket.local_addr().unwrap().port());
        let key = keys::PrivateKey::random(&mut rand::rng(), keys::Algorithm::Ed25519).unwrap();
        let fingerprint = fingerprint(key.public_key());
        let info = serde_json::to_vec(&Announcement {
            name: "Test device".into(),
            fingerprint: fingerprint.clone(),
        })
        .unwrap();
        let responder = tokio::spawn(announce(socket, info));
        let (discovered, self_only) = tokio::join!(
            discover(address, None),
            discover(address, Some(&fingerprint))
        );
        let peers = discovered.unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].name, "Test device");
        assert_eq!(peers[0].fingerprint.as_ref(), Some(&fingerprint));
        assert!(self_only.unwrap().is_empty());
        responder.abort();
    }
}
