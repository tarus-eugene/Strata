use crate::protocol::{Protocol, Source, TransferOptions};
use anyhow::{Context, Result, bail, ensure};
use futures_util::StreamExt;
use reqwest::{Client, StatusCode, header};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::AsyncWriteExt,
    runtime::{Handle, Runtime},
    sync::Semaphore,
    task::JoinSet,
};
use url::Url;

mod ftp;
mod metalink;
mod sftp;
mod tools;
#[cfg(feature = "torrent-native")]
mod torrent;
mod video;

const RUN: u8 = 0;
const PAUSE: u8 = 1;
const CANCEL: u8 = 2;
const MIN_PART: u64 = 256 * 1024;
static RUNTIME: OnceLock<Runtime> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    Queued,
    Connecting,
    Downloading,
    Pausing,
    Paused,
    Assembling,
    Complete,
    Failed,
    Cancelled,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Self::Queued => "Queued",
            Self::Connecting => "Connecting",
            Self::Downloading => "Downloading",
            Self::Pausing => "Pausing",
            Self::Paused => "Paused",
            Self::Assembling => "Finishing",
            Self::Complete => "Completed",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
        }
    }
    pub fn active(self) -> bool {
        matches!(
            self,
            Self::Connecting | Self::Downloading | Self::Pausing | Self::Assembling
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Part {
    pub start: u64,
    pub end: u64,
    pub downloaded: u64,
}

impl Part {
    pub fn len(&self) -> u64 {
        self.end - self.start + 1
    }
    pub fn is_empty(&self) -> bool {
        false
    }
    pub fn fraction(&self) -> f32 {
        (self.downloaded as f64 / self.len() as f64).clamp(0., 1.) as f32
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Download {
    pub id: u64,
    pub url: String,
    pub name: String,
    pub directory: PathBuf,
    pub total: Option<u64>,
    pub downloaded: u64,
    pub status: Status,
    pub parts: Vec<Part>,
    pub validator: Option<String>,
    pub ranged: bool,
    pub error: Option<String>,
    pub connections: usize,
    pub created: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub referrer: Option<String>,
    #[serde(default)]
    pub protocol: Protocol,
    #[serde(default)]
    pub options: TransferOptions,
}

impl Download {
    pub fn path(&self) -> PathBuf {
        self.directory.join(&self.name)
    }
    pub fn parts_dir(&self) -> PathBuf {
        self.directory.join(format!(".strata-{}.parts", self.id))
    }
    pub fn fraction(&self) -> f32 {
        if self.status == Status::Complete {
            return 1.;
        }
        self.total.filter(|n| *n > 0).map_or(0., |n| {
            (self.downloaded as f64 / n as f64).clamp(0., 1.) as f32
        })
    }
    pub fn host(&self) -> String {
        Url::parse(&self.url)
            .ok()
            .and_then(|u| match u.scheme() {
                "file" => Some("Local descriptor".into()),
                "magnet" => Some("BitTorrent network".into()),
                _ => u.host_str().map(str::to_owned),
            })
            .unwrap_or_default()
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Settings {
    pub directory: PathBuf,
    pub connections: usize,
    #[serde(default)]
    pub transport: TransferOptions,
    #[serde(default = "default_details_height")]
    pub details_height: f32,
}

fn default_details_height() -> f32 {
    240.
}

impl Default for Settings {
    fn default() -> Self {
        let home = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            directory: home.join("Downloads"),
            connections: 8,
            transport: TransferOptions::default(),
            details_height: default_details_height(),
        }
    }
}

struct Job {
    data: Mutex<Download>,
    control: Arc<AtomicU8>,
    running: AtomicBool,
}

#[derive(Serialize, Deserialize)]
struct History {
    settings: Settings,
    downloads: Vec<Download>,
}

pub struct Manager {
    jobs: Mutex<Vec<Arc<Job>>>,
    pub settings: Mutex<Settings>,
    runtime: Handle,
    client: Client,
    slots: Arc<Semaphore>,
    history: PathBuf,
    save_lock: Mutex<()>,
    pub persistence_error: Mutex<Option<String>>,
    pub browser_error: Mutex<Option<String>>,
    pub show_requested: AtomicBool,
    #[cfg(feature = "torrent-native")]
    torrent_session: tokio::sync::OnceCell<Arc<librqbit::Session>>,
}

impl Manager {
    pub fn new(history: PathBuf) -> Result<Arc<Self>> {
        let loaded = if history.exists() {
            Some(
                serde_json::from_slice::<History>(&fs::read(&history)?).context(
                    "Cannot read download history. The original file was left untouched.",
                )?,
            )
        } else {
            None
        };
        let (settings, downloads) = loaded
            .map(|h| (h.settings, h.downloads))
            .unwrap_or_default();
        let jobs = downloads
            .into_iter()
            .map(|mut data| {
                if data.status.active() || data.status == Status::Queued {
                    data.status = Status::Paused;
                }
                Arc::new(Job {
                    data: Mutex::new(data),
                    control: Arc::new(AtomicU8::new(PAUSE)),
                    running: AtomicBool::new(false),
                })
            })
            .collect();
        let runtime = RUNTIME.get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(4)
                .enable_all()
                .build()
                .expect("Cannot initialize download workers")
        });
        let client = Client::builder()
            .user_agent("Strata/0.1")
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()?;
        let manager = Arc::new(Self {
            jobs: Mutex::new(jobs),
            settings: Mutex::new(settings),
            runtime: runtime.handle().clone(),
            client,
            slots: Arc::new(Semaphore::new(3)),
            history,
            save_lock: Mutex::new(()),
            persistence_error: Mutex::new(None),
            browser_error: Mutex::new(None),
            show_requested: AtomicBool::new(false),
            #[cfg(feature = "torrent-native")]
            torrent_session: tokio::sync::OnceCell::new(),
        });
        let weak = Arc::downgrade(&manager);
        manager.runtime.spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let Some(manager) = weak.upgrade() else {
                    break;
                };
                manager.persist();
            }
        });
        Ok(manager)
    }

    pub fn snapshots(&self) -> Vec<Download> {
        self.jobs
            .lock()
            .unwrap()
            .iter()
            .map(|j| j.data.lock().unwrap().clone())
            .collect()
    }

    pub fn add(
        self: &Arc<Self>,
        input: &str,
        directory: PathBuf,
        connections: usize,
    ) -> Result<u64> {
        self.add_with_options(input, directory, connections, TransferOptions::default())
    }

    pub fn add_with_options(
        self: &Arc<Self>,
        input: &str,
        directory: PathBuf,
        connections: usize,
        options: TransferOptions,
    ) -> Result<u64> {
        self.add_with_metadata(input, directory, connections, None, None, options)
    }

    pub(crate) fn runtime_handle(&self) -> Handle {
        self.runtime.clone()
    }

    pub fn add_from_browser(
        self: &Arc<Self>,
        input: &str,
        name: Option<&str>,
        referrer: Option<&str>,
    ) -> Result<u64> {
        let browser_url = Url::parse(input)?;
        ensure!(
            matches!(
                browser_url.scheme(),
                "http" | "https" | "ftp" | "sftp" | "webdav" | "webdavs" | "magnet"
            ),
            "Browsers may send only remote download links, never local file paths"
        );
        let settings = self.settings.lock().unwrap().clone();
        let referrer = referrer
            .filter(|s| !s.is_empty())
            .map(validate_url)
            .transpose()?
            .map(|u| u.to_string());
        let name = name
            .filter(|s| !s.is_empty())
            .map(|s| sanitize_filename(s.rsplit(['/', '\\']).next().unwrap_or(s)));
        self.add_with_metadata(
            input,
            settings.directory,
            settings.connections,
            name,
            referrer,
            TransferOptions::default(),
        )
    }

    fn add_with_metadata(
        self: &Arc<Self>,
        input: &str,
        directory: PathBuf,
        connections: usize,
        suggested_name: Option<String>,
        referrer: Option<String>,
        options: TransferOptions,
    ) -> Result<u64> {
        let defaults = self.settings.lock().unwrap().transport.clone();
        let explicit_proxy = options.proxy.is_some();
        let mut options = options.with_defaults(&defaults);
        if options.protocol.is_none()
            && let Some(name) = &suggested_name
        {
            options.protocol = match Path::new(name)
                .extension()
                .and_then(|s| s.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref()
            {
                Some("torrent") => Some(Protocol::Torrent),
                Some("meta4" | "metalink") => Some(Protocol::Metalink),
                _ => None,
            };
        }
        let source = Source::parse(input, options)?;
        ensure!(
            !explicit_proxy
                || !matches!(
                    source.protocol,
                    Protocol::Ftp | Protocol::Sftp | Protocol::Torrent
                ),
            "This protocol does not support an explicit proxy. Proxy routing is available for HTTP(S), WebDAV, Metalink HTTP mirrors, and video transfers."
        );
        let url = &source.url;
        let directory = std::path::absolute(directory)?;
        ensure!(directory.is_dir(), "Choose an existing download folder.");
        let mut jobs = self.jobs.lock().unwrap();
        let id = SystemTime::now().duration_since(UNIX_EPOCH)?.as_micros() as u64;
        let base = if source.protocol.folder_output() {
            let name = if let Some(name) = &suggested_name {
                name.clone()
            } else if url.scheme() == "magnet" {
                url.query_pairs()
                    .find(|(k, _)| k == "dn")
                    .map(|(_, v)| sanitize_filename(&v))
                    .unwrap_or_else(|| "Torrent".into())
            } else {
                filename(url)
            };
            format!(
                "{}-files",
                Path::new(&name)
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
            )
        } else {
            suggested_name.unwrap_or_else(|| filename(url))
        };
        let name = available_name(
            &directory,
            &base,
            &jobs
                .iter()
                .map(|j| j.data.lock().unwrap().path())
                .collect::<Vec<_>>(),
        )?;
        let job = Arc::new(Job {
            data: Mutex::new(Download {
                id,
                url: url.to_string(),
                name,
                directory,
                total: None,
                downloaded: 0,
                status: Status::Queued,
                parts: vec![],
                validator: None,
                ranged: false,
                error: None,
                connections: connections.clamp(1, 16),
                created: id / 1_000_000,
                referrer,
                protocol: source.protocol,
                options: source.options,
            }),
            control: Arc::new(AtomicU8::new(RUN)),
            running: AtomicBool::new(false),
        });
        jobs.push(job.clone());
        drop(jobs);
        self.persist();
        if let Some(error) = self.persistence_error.lock().unwrap().clone() {
            self.jobs
                .lock()
                .unwrap()
                .retain(|j| j.data.lock().unwrap().id != id);
            bail!(error);
        }
        self.launch(job);
        Ok(id)
    }

    pub fn resume(self: &Arc<Self>, id: u64) {
        if let Some(job) = self.find(id) {
            if job.running.load(Ordering::SeqCst) {
                return;
            }
            if matches!(
                job.data.lock().unwrap().status,
                Status::Complete | Status::Cancelled
            ) {
                return;
            }
            job.control.store(RUN, Ordering::SeqCst);
            {
                let mut d = job.data.lock().unwrap();
                d.status = Status::Queued;
                d.error = None;
            }
            self.launch(job);
        }
    }

    pub fn pause(&self, id: u64) {
        if let Some(job) = self.find(id) {
            let mut d = job.data.lock().unwrap();
            if matches!(
                d.status,
                Status::Queued | Status::Connecting | Status::Downloading
            ) {
                job.control.store(PAUSE, Ordering::SeqCst);
                d.status = Status::Pausing;
            }
        }
    }

    pub fn cancel(&self, id: u64) {
        if let Some(job) = self.find(id) {
            let mut d = job.data.lock().unwrap();
            if d.status == Status::Complete || d.status == Status::Assembling {
                return;
            }
            job.control.store(CANCEL, Ordering::SeqCst);
            d.status = Status::Cancelled;
            // Partial files are retained; cancellation never deletes user data.
        }
        self.persist();
    }

    pub fn pause_all(&self) {
        for d in self.snapshots() {
            self.pause(d.id);
        }
    }

    pub fn remove(&self, id: u64) {
        self.jobs.lock().unwrap().retain(|job| {
            let mut data = job.data.lock().unwrap();
            if data.id != id {
                return true;
            }
            job.control.store(CANCEL, Ordering::SeqCst);
            data.status = Status::Cancelled;
            false
        });
        // Removing history never removes downloaded files or private partial data.
        self.persist();
    }

    pub fn shutdown(&self) {
        self.pause_all();
        // Wait for writes to settle before the process exits so part lengths remain resumable.
        for _ in 0..50 {
            if self
                .jobs
                .lock()
                .unwrap()
                .iter()
                .all(|j| !j.running.load(Ordering::SeqCst))
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        self.persist();
    }

    pub fn persist(&self) {
        let _guard = self.save_lock.lock().unwrap();
        let result = (|| -> Result<()> {
            let parent = self.history.parent().context("Missing history folder")?;
            fs::create_dir_all(parent)?;
            let history = History {
                settings: self.settings.lock().unwrap().clone(),
                downloads: self.snapshots(),
            };
            let mut temp = tempfile::NamedTempFile::new_in(parent)?;
            serde_json::to_writer_pretty(&mut temp, &history)?;
            temp.as_file().sync_all()?;
            temp.persist(&self.history)?;
            Ok(())
        })();
        *self.persistence_error.lock().unwrap() = result
            .err()
            .map(|e| format!("Could not save history: {e:#}"));
    }

    fn find(&self, id: u64) -> Option<Arc<Job>> {
        self.jobs
            .lock()
            .unwrap()
            .iter()
            .find(|j| j.data.lock().unwrap().id == id)
            .cloned()
    }

    fn launch(self: &Arc<Self>, job: Arc<Job>) {
        if job.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let manager = self.clone();
        self.runtime.spawn(async move {
            let result = manager.execute(&job).await;
            {
                let mut d = job.data.lock().unwrap();
                match job.control.load(Ordering::SeqCst) {
                    CANCEL if d.status != Status::Complete => d.status = Status::Cancelled,
                    PAUSE if d.status != Status::Complete => d.status = Status::Paused,
                    _ => {
                        if let Err(e) = result {
                            d.status = Status::Failed;
                            d.error = Some(format!("{e:#}"));
                        }
                    }
                }
            }
            job.running.store(false, Ordering::SeqCst);
            manager.persist();
        });
    }

    async fn execute(&self, job: &Arc<Job>) -> Result<()> {
        let _permit = tokio::select! {
            permit = self.slots.acquire() => permit?,
            _ = stopped(job) => bail!("Transfer stopped"),
        };
        check_running(job)?;
        job.data.lock().unwrap().status = Status::Connecting;
        let protocol = job.data.lock().unwrap().protocol;
        match protocol {
            Protocol::Http | Protocol::WebDav => self.execute_http(job).await,
            Protocol::Ftp => self.execute_ftp(job).await,
            Protocol::Sftp => self.execute_sftp(job).await,
            Protocol::Metalink => self.execute_metalink(job).await,
            Protocol::Video => self.execute_video(job).await,
            #[cfg(feature = "torrent-native")]
            Protocol::Torrent => self.execute_torrent(job).await,
            #[cfg(not(feature = "torrent-native"))]
            Protocol::Torrent => bail!("This build needs the torrent-native feature"),
        }
    }

    fn client_for(&self, options: &TransferOptions) -> Result<Client> {
        if let Some(proxy) = &options.proxy {
            Ok(Client::builder()
                .user_agent("Strata/0.1")
                .connect_timeout(Duration::from_secs(15))
                .read_timeout(Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::limited(10))
                .proxy(reqwest::Proxy::all(proxy.connection_url()?)?)
                .build()?)
        } else {
            Ok(self.client.clone())
        }
    }

    async fn execute_http(&self, job: &Arc<Job>) -> Result<()> {
        let initial = job.data.lock().unwrap().clone();
        let client = self.client_for(&initial.options)?;
        let probe = tokio::select! {
            response = download_request(&client, &initial)?.header(header::RANGE, "bytes=0-0").send() => response?,
            _ = stopped(job) => bail!("Transfer stopped"),
        };
        if probe.status() != StatusCode::RANGE_NOT_SATISFIABLE {
            probe.error_for_status_ref()?;
        }
        ensure_identity(&probe)?;
        let range = probe
            .headers()
            .get(header::CONTENT_RANGE)
            .and_then(|h| h.to_str().ok())
            .and_then(parse_content_range);
        let validator = response_validator(&probe);
        let ranged = probe.status() == StatusCode::PARTIAL_CONTENT
            && range.is_some_and(|(s, e, n)| s == 0 && e == 0 && n > 0)
            && validator.is_some();
        let total = if ranged {
            range.map(|(_, _, n)| n)
        } else if probe.status() == StatusCode::OK {
            probe.content_length()
        } else {
            None
        };
        drop(probe);
        check_running(job)?;
        let old = job.data.lock().unwrap().clone();
        let dir = old.parts_dir();
        let reuse = ranged
            && old.ranged
            && old.total == total
            && old.validator == validator
            && !old.parts.is_empty();
        fs::create_dir_all(&dir).context("Cannot create partial download folder")?;
        let parts = if ranged {
            let size = total.unwrap();
            let count = old.connections.min((size / MIN_PART).max(1) as usize);
            if reuse {
                old.parts.clone()
            } else {
                partition(size, count)
            }
        } else {
            vec![Part {
                start: 0,
                end: total.unwrap_or(1).saturating_sub(1),
                downloaded: 0,
            }]
        };
        {
            let mut d = job.data.lock().unwrap();
            d.total = total;
            d.ranged = ranged;
            d.validator = validator;
            d.parts = parts;
            d.downloaded = 0;
            for (i, part) in d.parts.iter_mut().enumerate() {
                let path = dir.join(format!("{i}.part"));
                if reuse {
                    part.downloaded = fs::metadata(&path).map_or(0, |m| m.len());
                    ensure!(
                        part.downloaded <= part.len(),
                        "A partial file is longer than its assigned range. Cancel and add the URL again."
                    );
                } else {
                    fs::File::create(path)?;
                    part.downloaded = 0;
                }
            }
            d.downloaded = d.parts.iter().map(|p| p.downloaded).sum();
            d.status = Status::Downloading;
        }
        let part_count = job.data.lock().unwrap().parts.len();
        let mut workers = JoinSet::new();
        for index in 0..part_count {
            let job = job.clone();
            let client = client.clone();
            workers.spawn(async move { download_part(client, job, index).await });
        }
        let mut failure = None;
        while let Some(result) = workers.join_next().await {
            if let Err(e) = result.context("Connection worker stopped").and_then(|v| v) {
                failure = Some(e);
            }
        }
        if let Some(e) = failure {
            return Err(e);
        }
        check_running(job)?;
        let d = job.data.lock().unwrap().clone();
        ensure!(
            d.total.is_none_or(|n| n == d.downloaded),
            "Downloaded size does not match the server's file size"
        );
        job.data.lock().unwrap().status = Status::Assembling;
        // persist_noclobber prevents replacing a file created after this transfer was added.
        let output = tempfile::NamedTempFile::new_in(&d.directory)?;
        let mut writer = tokio::fs::OpenOptions::new()
            .write(true)
            .open(output.path())
            .await?;
        for i in 0..part_count {
            let mut part = tokio::fs::File::open(dir.join(format!("{i}.part"))).await?;
            tokio::io::copy(&mut part, &mut writer).await?;
        }
        writer.flush().await?;
        writer.sync_all().await?;
        drop(writer);
        output.persist_noclobber(d.path()).context("Cannot save the finished file. A file with this name may already exist; move it and retry.")?;
        {
            let mut data = job.data.lock().unwrap();
            data.total = Some(data.downloaded);
            data.status = Status::Complete;
            data.error = None;
        }
        // This directory is exclusively owned by this transfer and contains only its parts.
        for i in 0..part_count {
            let _ = fs::remove_file(dir.join(format!("{i}.part")));
        }
        let _ = fs::remove_dir(dir);
        Ok(())
    }
}

async fn stopped(job: &Job) {
    while job.control.load(Ordering::SeqCst) == RUN {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn check_running(job: &Job) -> Result<()> {
    ensure!(
        job.control.load(Ordering::SeqCst) == RUN,
        "Transfer stopped"
    );
    Ok(())
}

async fn download_part(client: Client, job: Arc<Job>, index: usize) -> Result<()> {
    for attempt in 0..4 {
        check_running(&job)?;
        let result = transfer_part(&client, &job, index).await;
        if result.is_ok() {
            return result;
        }
        check_running(&job)?;
        if attempt == 3 {
            return result;
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(400 * (1 << attempt))) => {},
            _ = stopped(&job) => bail!("Transfer stopped"),
        }
    }
    unreachable!()
}

async fn transfer_part(client: &Client, job: &Arc<Job>, index: usize) -> Result<()> {
    let d = job.data.lock().unwrap().clone();
    let part = &d.parts[index];
    let path = d.parts_dir().join(format!("{index}.part"));
    let offset = if d.ranged {
        fs::metadata(&path)?.len()
    } else {
        0
    };
    if d.ranged && offset == part.len() {
        return Ok(());
    }
    let mut request = download_request(client, &d)?;
    if d.ranged {
        request = request.header(
            header::RANGE,
            format!("bytes={}-{}", part.start + offset, part.end),
        );
        if let Some(v) = &d.validator {
            request = request.header(header::IF_RANGE, v);
        }
    }
    let response = tokio::select! {
        response = request.send() => response?,
        _ = stopped(job) => bail!("Transfer stopped"),
    };
    response.error_for_status_ref()?;
    ensure_identity(&response)?;
    if d.ranged {
        ensure!(
            response.status() == StatusCode::PARTIAL_CONTENT,
            "The server stopped supporting this range, or the file changed. Cancel and add the URL again."
        );
        let range = response
            .headers()
            .get(header::CONTENT_RANGE)
            .and_then(|h| h.to_str().ok())
            .and_then(parse_content_range);
        ensure!(
            range == Some((part.start + offset, part.end, d.total.unwrap())),
            "Server returned an incorrect byte range"
        );
        if let Some(current) = response_validator(&response) {
            ensure!(
                Some(current) == d.validator,
                "The remote file changed during the transfer"
            );
        }
    } else {
        ensure!(
            response.status() == StatusCode::OK,
            "Expected a full file response from the server"
        );
        let mut data = job.data.lock().unwrap();
        data.total = response.content_length();
        data.downloaded = 0;
        data.parts[0].downloaded = 0;
        data.parts[0].end = data.total.unwrap_or(1).saturating_sub(1);
    }
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .append(d.ranged)
        .truncate(!d.ranged)
        .open(&path)
        .await?;
    let mut received = offset;
    let mut stream = response.bytes_stream();
    loop {
        let chunk = tokio::select! {
            chunk = stream.next() => chunk,
            _ = stopped(job) => { file.flush().await?; bail!("Transfer stopped"); },
        };
        let Some(chunk) = chunk else {
            break;
        };
        let chunk = chunk?;
        if d.ranged {
            ensure!(
                received + chunk.len() as u64 <= part.len(),
                "Server sent more bytes than requested"
            );
        }
        file.write_all(&chunk).await?;
        received += chunk.len() as u64;
        let mut data = job.data.lock().unwrap();
        data.parts[index].downloaded = received;
        data.downloaded = data.parts.iter().map(|p| p.downloaded).sum();
    }
    file.flush().await?;
    if d.ranged {
        ensure!(
            received == part.len(),
            "Connection ended before the whole range arrived"
        );
    } else if let Some(total) = job.data.lock().unwrap().total {
        ensure!(
            received == total,
            "Connection ended before the file was complete"
        );
    }
    Ok(())
}

fn ensure_identity(response: &reqwest::Response) -> Result<()> {
    ensure!(
        response
            .headers()
            .get(header::CONTENT_ENCODING)
            .is_none_or(|v| v == "identity"),
        "The server sent encoded content despite a request for original file bytes"
    );
    Ok(())
}

fn download_request(client: &Client, download: &Download) -> Result<reqwest::RequestBuilder> {
    let mut request = client
        .get(&download.url)
        .header(header::ACCEPT_ENCODING, "identity");
    if let Some(user) = &download.options.username {
        request = request.basic_auth(
            user,
            download
                .options
                .password
                .as_ref()
                .map(|s| s.expose())
                .transpose()?,
        );
    }
    Ok(match &download.referrer {
        Some(referrer) => request.header(header::REFERER, referrer),
        None => request,
    })
}

fn response_validator(response: &reqwest::Response) -> Option<String> {
    response
        .headers()
        .get(header::ETAG)
        .and_then(|h| h.to_str().ok())
        .filter(|v| !v.starts_with("W/"))
        .or_else(|| {
            response
                .headers()
                .get(header::LAST_MODIFIED)
                .and_then(|h| h.to_str().ok())
        })
        .map(str::to_owned)
}

pub fn partition(size: u64, count: usize) -> Vec<Part> {
    if size == 0 {
        return vec![];
    }
    let count = (count.clamp(1, 16) as u64).min(size);
    let base = size / count;
    let extra = size % count;
    let mut start = 0;
    (0..count)
        .map(|i| {
            let length = base + u64::from(i < extra);
            let part = Part {
                start,
                end: start + length - 1,
                downloaded: 0,
            };
            start += length;
            part
        })
        .collect()
}

pub fn parse_content_range(value: &str) -> Option<(u64, u64, u64)> {
    let (range, total) = value.strip_prefix("bytes ")?.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    let (start, end, total) = (start.parse().ok()?, end.parse().ok()?, total.parse().ok()?);
    (start <= end && end < total).then_some((start, end, total))
}

pub fn validate_url(input: &str) -> Result<Url> {
    let url = Url::parse(input.trim()).context("Enter a full http:// or https:// download URL")?;
    ensure!(
        matches!(url.scheme(), "http" | "https") && url.host_str().is_some(),
        "Only HTTP and HTTPS download links are supported"
    );
    ensure!(
        url.username().is_empty() && url.password().is_none(),
        "URLs containing passwords are not supported"
    );
    Ok(url)
}

pub fn filename(url: &Url) -> String {
    let last = url
        .path_segments()
        .and_then(|mut s| s.next_back())
        .filter(|s| !s.is_empty())
        .unwrap_or("download");
    let decoded = percent_encoding::percent_decode_str(last).decode_utf8_lossy();
    sanitize_filename(&decoded)
}

fn sanitize_filename(decoded: &str) -> String {
    let mut name: String = decoded
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .take(160)
        .collect();
    name = name.trim_matches([' ', '.']).to_string();
    let stem = name.split('.').next().unwrap_or("").to_uppercase();
    if name.is_empty()
        || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.ends_with(|c: char| c.is_ascii_digit()))
    {
        name = format!("download_{name}");
    }
    name
}

fn available_name(directory: &Path, base: &str, reserved: &[PathBuf]) -> Result<String> {
    let path = Path::new(base);
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let ext = path
        .extension()
        .map(|s| format!(".{}", s.to_string_lossy()))
        .unwrap_or_default();
    for i in 0..10000 {
        let name = if i == 0 {
            base.to_owned()
        } else {
            format!("{stem} ({i}){ext}")
        };
        let candidate = directory.join(&name);
        if !candidate.exists()
            && !reserved.iter().any(|p| {
                p.to_string_lossy()
                    .eq_ignore_ascii_case(&candidate.to_string_lossy())
            })
        {
            return Ok(name);
        }
    }
    bail!("Too many files with this name in the download folder")
}

pub fn history_path() -> PathBuf {
    if let Some(directory) = std::env::var_os("STRATA_DATA_DIR") {
        return PathBuf::from(directory).join("history.json");
    }
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Strata")
        .join("history.json")
}

#[cfg(test)]
mod settings_tests {
    use super::Settings;

    #[test]
    fn legacy_settings_default_panel_height_and_new_height_round_trips() {
        let mut settings: Settings = serde_json::from_value(
            serde_json::json!({ "directory": "Downloads", "connections": 8 }),
        )
        .unwrap();
        assert_eq!(settings.details_height, 240.);
        settings.details_height = 416.;
        let saved = serde_json::to_vec(&settings).unwrap();
        assert_eq!(
            serde_json::from_slice::<Settings>(&saved)
                .unwrap()
                .details_height,
            416.
        );
    }
}
