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

mod chunks;
mod controls;
mod deletion;
mod ftp;
mod integrity;
mod metalink;
mod naming;
mod sftp;
mod tools;
#[cfg(feature = "torrent-native")]
mod torrent;
mod video;

pub(crate) use controls::{move_without_replacing, validate_name};

pub use integrity::IntegrityFile;

const RUN: u8 = 0;
const PAUSE: u8 = 1;
const CANCEL: u8 = 2;
const MIN_PART: u64 = 256 * 1024;
pub const AUTO_CONNECTIONS: usize = 0;
const DEFAULT_NON_HTTP_CONNECTIONS: usize = 8;
const DOWNLOAD_RETRIES: u32 = 3;
static RUNTIME: OnceLock<Runtime> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    Queued,
    Connecting,
    Retrying,
    SelectingFiles,
    Downloading,
    Pausing,
    Paused,
    Assembling,
    Verifying,
    Complete,
    Missing,
    Failed,
    Cancelled,
    Deleting,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Self::Queued => "Queued",
            Self::Connecting => "Connecting",
            Self::Retrying => "Retrying",
            Self::SelectingFiles => "Choose files",
            Self::Downloading => "Downloading",
            Self::Pausing => "Pausing",
            Self::Paused => "Paused",
            Self::Assembling => "Finishing",
            Self::Verifying => "Verifying",
            Self::Complete => "Completed",
            Self::Missing => "Missing",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
            Self::Deleting => "Deleting",
        }
    }
    pub fn active(self) -> bool {
        matches!(
            self,
            Self::Connecting
                | Self::Retrying
                | Self::Downloading
                | Self::Pausing
                | Self::Assembling
                | Self::Verifying
        )
    }

    pub fn can_pause_transfer(self) -> bool {
        matches!(
            self,
            Self::Queued | Self::Connecting | Self::Retrying | Self::Downloading
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Priority {
    Low,
    #[default]
    Normal,
    High,
}

impl Priority {
    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Normal => "Normal",
            Self::High => "High",
        }
    }
}

pub enum DownloadChange {
    Rename(String),
    SpeedLimit(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionAction {
    ShutDown,
    Sleep,
    Restart,
    Open,
}

impl CompletionAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::ShutDown => "Shut down PC",
            Self::Sleep => "Sleep",
            Self::Restart => "Restart PC",
            Self::Open => "Open",
        }
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TorrentFile {
    pub name: String,
    pub size: u64,
}

pub struct PreparedDownload {
    source: Source,
    referrer: Option<String>,
    name: String,
    preserve_name: bool,
}

impl PreparedDownload {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn protocol(&self) -> Protocol {
        self.source.protocol
    }

    pub fn url(&self) -> &Url {
        &self.source.url
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
    #[serde(default)]
    pub completed_at: Option<u64>,
    #[serde(skip)]
    pub completion_action: Option<CompletionAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub referrer: Option<String>,
    #[serde(default)]
    pub protocol: Protocol,
    #[serde(default)]
    pub options: TransferOptions,
    #[serde(default)]
    pub preserve_name: bool,
    #[serde(default)]
    pub owns_output: bool,
    #[serde(default)]
    pub priority: Priority,
    #[serde(default)]
    pub speed_limit: u64,
    #[serde(default)]
    pub torrent_files: Vec<TorrentFile>,
    #[serde(default)]
    pub selected_files: Option<Vec<usize>>,
    #[serde(default)]
    pub integrity: Vec<IntegrityFile>,
    #[serde(default)]
    pub integrity_baseline_only: bool,
}

impl Download {
    pub fn can_set_completion_action(&self) -> bool {
        !self.owns_output
            && !matches!(
                self.status,
                Status::Complete | Status::Missing | Status::Cancelled | Status::Deleting
            )
    }

    pub fn can_verify_integrity(&self) -> bool {
        self.owns_output
            && !self.status.active()
            && !matches!(self.status, Status::Queued | Status::Deleting)
    }
    pub fn path(&self) -> PathBuf {
        self.directory.join(&self.name)
    }
    pub fn parts_dir(&self) -> PathBuf {
        self.directory.join(format!(".strata-{}.parts", self.id))
    }
    pub fn fraction(&self) -> f32 {
        if matches!(self.status, Status::Complete | Status::Missing) {
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
    #[serde(default)]
    pub speed_limit: u64,
    #[serde(default = "default_completion_chime")]
    pub completion_chime: bool,
}

fn default_details_height() -> f32 {
    240.
}

fn default_completion_chime() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        let home = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            directory: home.join("Downloads"),
            connections: AUTO_CONNECTIONS,
            transport: TransferOptions::default(),
            details_height: default_details_height(),
            speed_limit: 0,
            completion_chime: default_completion_chime(),
        }
    }
}

struct Job {
    data: Mutex<Download>,
    control: Arc<AtomicU8>,
    running: AtomicBool,
    name_hint: Mutex<Option<naming::FilenameHint>>,
    deleting: AtomicBool,
    editing: AtomicBool,
    limiter: Arc<controls::RateLimiter>,
    global_limiter: Arc<controls::RateLimiter>,
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
    completion_pending: AtomicBool,
    completion_actions: Mutex<Vec<(u64, CompletionAction)>>,
    global_limiter: Arc<controls::RateLimiter>,
    controls_lock: tokio::sync::Mutex<()>,
    global_editing: AtomicBool,
    quitting: AtomicBool,
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
        let global_limiter = Arc::new(controls::RateLimiter::new(settings.speed_limit));
        let jobs = downloads
            .into_iter()
            .map(|mut data| {
                if matches!(data.status, Status::Complete | Status::Missing) {
                    data.owns_output = true;
                    integrity::refresh_missing(&mut data);
                }
                if data.status == Status::Verifying && data.owns_output {
                    data.status = Status::Complete;
                    integrity::refresh_missing(&mut data);
                    if data.status == Status::Complete {
                        data.status = Status::Failed;
                        data.error = Some("Integrity verification was interrupted. Choose Verify file integrity to check again.".into());
                    }
                }
                if data.status == Status::Deleting {
                    data.status = Status::Failed;
                    data.error = Some(
                        "Deletion was interrupted. Choose Delete from device again to finish."
                            .into(),
                    );
                }
                if data.status.active() || data.status == Status::Queued {
                    data.status = Status::Paused;
                }
                Arc::new(Job {
                    limiter: Arc::new(controls::RateLimiter::new(data.speed_limit)),
                    global_limiter: global_limiter.clone(),
                    data: Mutex::new(data),
                    control: Arc::new(AtomicU8::new(PAUSE)),
                    running: AtomicBool::new(false),
                    name_hint: Mutex::new(None),
                    deleting: AtomicBool::new(false),
                    editing: AtomicBool::new(false),
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
            .pool_max_idle_per_host(16)
            .pool_idle_timeout(Duration::from_secs(90))
            .user_agent(concat!("101Downloader/", env!("CARGO_PKG_VERSION")))
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
            completion_pending: AtomicBool::new(false),
            completion_actions: Mutex::new(Vec::new()),
            global_limiter,
            controls_lock: tokio::sync::Mutex::new(()),
            global_editing: AtomicBool::new(false),
            quitting: AtomicBool::new(false),
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
                let _ = tokio::task::spawn_blocking(move || {
                    manager.refresh_missing();
                    let _guard = manager.save_lock.lock().unwrap();
                    if !manager.quitting.load(Ordering::SeqCst) {
                        manager.persist_locked();
                    }
                })
                .await;
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

    pub fn take_completion_chime(&self) -> bool {
        self.completion_pending.swap(false, Ordering::SeqCst)
            && self.settings.lock().unwrap().completion_chime
    }

    pub fn set_completion_action(&self, id: u64, action: Option<CompletionAction>) -> Result<()> {
        let job = self.find(id).context("This download no longer exists")?;
        let mut data = job.data.lock().unwrap();
        ensure!(
            data.can_set_completion_action() && !self.quitting.load(Ordering::SeqCst),
            "This download can no longer run a completion action"
        );
        data.completion_action = action;
        Ok(())
    }

    pub fn take_completion_actions(&self) -> Vec<(u64, CompletionAction)> {
        std::mem::take(&mut *self.completion_actions.lock().unwrap())
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
        let download = self.prepare_download(input, options)?;
        self.start_download(&download, directory, connections, None, None)
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
        let download = self.prepare_browser_download(input, name, referrer)?;
        let settings = self.settings.lock().unwrap().clone();
        self.start_download(
            &download,
            settings.directory,
            settings.connections,
            None,
            None,
        )
    }

    pub fn prepare_browser_download(
        &self,
        input: &str,
        name: Option<&str>,
        referrer: Option<&str>,
    ) -> Result<PreparedDownload> {
        let browser_url = Url::parse(input)?;
        ensure!(
            matches!(
                browser_url.scheme(),
                "http" | "https" | "ftp" | "sftp" | "webdav" | "webdavs" | "magnet"
            ),
            "Browsers may send only remote download links, never local file paths"
        );
        let referrer = referrer
            .filter(|s| !s.is_empty())
            .map(validate_url)
            .transpose()?
            .map(|u| u.to_string());
        let name = name
            .filter(|s| !s.is_empty())
            .map(|s| sanitize_filename(s.rsplit(['/', '\\']).next().unwrap_or(s)));
        self.prepare_with_metadata(input, name, referrer, TransferOptions::default())
    }

    pub fn prepare_download(
        &self,
        input: &str,
        options: TransferOptions,
    ) -> Result<PreparedDownload> {
        self.prepare_with_metadata(input, None, None, options)
    }

    fn prepare_with_metadata(
        &self,
        input: &str,
        suggested_name: Option<String>,
        referrer: Option<String>,
        options: TransferOptions,
    ) -> Result<PreparedDownload> {
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
        let preserve_name = suggested_name
            .as_ref()
            .is_some_and(|name| !naming::generated_name(name));
        let name = if source.protocol.folder_output() {
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
        Ok(PreparedDownload {
            source,
            referrer,
            name,
            preserve_name,
        })
    }

    pub fn start_download(
        self: &Arc<Self>,
        download: &PreparedDownload,
        directory: PathBuf,
        connections: usize,
        name: Option<&str>,
        completion_action: Option<CompletionAction>,
    ) -> Result<u64> {
        ensure!(
            !self.quitting.load(Ordering::SeqCst),
            "101 Downloader is quitting. Open it again and retry."
        );
        if let Some(name) = name {
            controls::validate_name(name)?;
        }
        ensure!(
            !directory.as_os_str().is_empty(),
            "Choose an existing download folder."
        );
        let directory = std::path::absolute(directory)?;
        ensure!(directory.is_dir(), "Choose an existing download folder.");
        let mut jobs = self.jobs.lock().unwrap();
        let id = SystemTime::now().duration_since(UNIX_EPOCH)?.as_micros() as u64;
        let preserve_name = name.is_some() || download.preserve_name;
        let name = available_name(
            &directory,
            name.unwrap_or(&download.name),
            &jobs
                .iter()
                .map(|j| j.data.lock().unwrap().path())
                .collect::<Vec<_>>(),
        )?;
        let job = Arc::new(Job {
            data: Mutex::new(Download {
                id,
                url: download.source.url.to_string(),
                name,
                directory,
                total: None,
                downloaded: 0,
                status: Status::Queued,
                parts: vec![],
                validator: None,
                ranged: false,
                error: None,
                connections: connections.min(16),
                created: id / 1_000_000,
                completed_at: None,
                completion_action,
                referrer: download.referrer.clone(),
                protocol: download.source.protocol,
                options: download.source.options.clone(),
                preserve_name,
                owns_output: false,
                priority: Priority::Normal,
                speed_limit: 0,
                torrent_files: Vec::new(),
                selected_files: None,
                integrity: Vec::new(),
                integrity_baseline_only: false,
            }),
            control: Arc::new(AtomicU8::new(RUN)),
            running: AtomicBool::new(false),
            name_hint: Mutex::new(None),
            deleting: AtomicBool::new(false),
            editing: AtomicBool::new(false),
            limiter: Arc::new(controls::RateLimiter::new(0)),
            global_limiter: self.global_limiter.clone(),
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
        self.show_requested.store(true, Ordering::SeqCst);
        Ok(id)
    }

    pub fn resume(self: &Arc<Self>, id: u64) {
        if self.quitting.load(Ordering::SeqCst) {
            return;
        }
        if let Some(job) = self.find(id) {
            let mut data = job.data.lock().unwrap();
            if job.running.load(Ordering::SeqCst)
                || job.deleting.load(Ordering::SeqCst)
                || job.editing.load(Ordering::SeqCst)
            {
                return;
            }
            if data.owns_output
                || matches!(
                    data.status,
                    Status::Complete | Status::Missing | Status::Cancelled
                )
            {
                return;
            }
            if data.protocol == Protocol::Torrent
                && data.selected_files.is_none()
                && !data.torrent_files.is_empty()
            {
                data.status = Status::SelectingFiles;
                data.error = None;
                self.show_requested.store(true, Ordering::SeqCst);
                return;
            }
            job.control.store(RUN, Ordering::SeqCst);
            data.status = Status::Queued;
            data.error = None;
            self.launch(job.clone());
        }
    }

    pub fn select_torrent_files(self: &Arc<Self>, id: u64, mut selected: Vec<usize>) -> Result<()> {
        let _guard = self
            .controls_lock
            .try_lock()
            .context("Another download setting is being saved. Try again in a moment.")?;
        let job = self.find(id).context("This download no longer exists")?;
        let (previous, staged) = {
            let mut data = job.data.lock().unwrap();
            ensure!(
                !self.quitting.load(Ordering::SeqCst)
                    && !job.running.load(Ordering::SeqCst)
                    && !job.deleting.load(Ordering::SeqCst)
                    && !job.editing.load(Ordering::SeqCst),
                "The download is busy. Try again in a moment."
            );
            ensure!(
                data.protocol == Protocol::Torrent
                    && matches!(
                        data.status,
                        Status::SelectingFiles
                            | Status::Paused
                            | Status::Complete
                            | Status::Missing
                            | Status::Failed
                            | Status::Cancelled
                    ),
                "This download is not waiting for file selection"
            );
            ensure!(
                !selected.is_empty(),
                "Select at least one file to download."
            );
            ensure!(
                selected
                    .iter()
                    .all(|&index| index < data.torrent_files.len()),
                "The file list has changed. Close this dialog and try again."
            );
            selected.sort_unstable();
            selected.dedup();
            if let Some(previous) = &data.selected_files {
                ensure!(
                    previous.iter().all(|index| selected.contains(index)),
                    "Previously selected files must remain selected."
                );
                if data.status == Status::Complete && previous == &selected {
                    return Ok(());
                }
            }
            let previous = data.clone();
            let staged = if data.owns_output && data.path().try_exists()? {
                let metadata = fs::symlink_metadata(data.path())?;
                ensure!(
                    metadata.is_dir() && !metadata.file_type().is_symlink(),
                    "The torrent output must be a regular folder"
                );
                fs::create_dir_all(data.parts_dir())?;
                let staged = data.parts_dir().join("content");
                controls::move_without_replacing(&data.path(), &staged)
                    .context("Cannot stage the existing torrent files")?;
                Some(staged)
            } else {
                None
            };
            job.editing.store(true, Ordering::SeqCst);
            data.total = Some(
                selected
                    .iter()
                    .map(|&index| data.torrent_files[index].size)
                    .sum(),
            );
            data.selected_files = Some(selected);
            data.owns_output = false;
            data.integrity.clear();
            data.integrity_baseline_only = false;
            data.completed_at = None;
            data.status = Status::Paused;
            (previous, staged)
        };
        self.persist();
        if let Some(error) = self.persistence_error.lock().unwrap().clone() {
            if let Some(staged) = staged {
                if let Err(rollback) = controls::move_without_replacing(&staged, &previous.path()) {
                    job.editing.store(false, Ordering::SeqCst);
                    bail!(
                        "{error}. Existing files remain in {} because restoring the output failed: {rollback:#}",
                        staged.display()
                    );
                }
            }
            *job.data.lock().unwrap() = previous;
            job.editing.store(false, Ordering::SeqCst);
            bail!(error);
        }
        job.editing.store(false, Ordering::SeqCst);
        self.resume(id);
        Ok(())
    }

    pub fn pause(&self, id: u64) {
        if let Some(job) = self.find(id) {
            let mut d = job.data.lock().unwrap();
            if d.status.can_pause_transfer() || d.status == Status::SelectingFiles {
                job.control.store(PAUSE, Ordering::SeqCst);
                d.status = if job.running.load(Ordering::SeqCst) {
                    Status::Pausing
                } else {
                    Status::Paused
                };
            }
        }
    }

    pub fn cancel(&self, id: u64) {
        if let Some(job) = self.find(id) {
            let mut d = job.data.lock().unwrap();
            if job.deleting.load(Ordering::SeqCst) {
                return;
            }
            if d.owns_output
                || matches!(
                    d.status,
                    Status::Complete | Status::Missing | Status::Assembling | Status::Verifying
                )
            {
                return;
            }
            job.control.store(CANCEL, Ordering::SeqCst);
            d.status = Status::Cancelled;
            d.completion_action = None;
            // Partial files are retained; cancellation never deletes user data.
        }
        self.persist();
    }

    pub fn pause_all(&self) {
        for d in self.snapshots() {
            self.pause(d.id);
        }
    }

    pub fn shutdown(&self) {
        self.quitting.store(true, Ordering::SeqCst);
        self.pause_all();
        // Wait for writes to settle before the process exits so part lengths remain resumable.
        for _ in 0..50 {
            if self.jobs.lock().unwrap().iter().all(|j| {
                !j.running.load(Ordering::SeqCst)
                    && !j.deleting.load(Ordering::SeqCst)
                    && !j.editing.load(Ordering::SeqCst)
            }) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        self.persist();
    }

    pub fn persist(&self) {
        let _guard = self.save_lock.lock().unwrap();
        self.persist_locked();
    }

    fn persist_locked(&self) {
        let result = (|| -> Result<()> {
            let parent = self.history.parent().context("Missing history folder")?;
            fs::create_dir_all(parent)?;
            let settings = self.settings.lock().unwrap().clone();
            let history = History {
                settings,
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

    fn torrent_metadata_path(&self, id: u64) -> PathBuf {
        self.history
            .with_file_name("torrent-metadata")
            .join(format!("{id}.torrent"))
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
            let result = manager.execute_with_retries(&job).await;
            let completion_action = {
                let mut d = job.data.lock().unwrap();
                if job.deleting.load(Ordering::SeqCst) {
                    d.status = Status::Deleting;
                } else {
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
                if d.status == Status::Complete && manager.settings.lock().unwrap().completion_chime
                {
                    manager.completion_pending.store(true, Ordering::SeqCst);
                }
                job.running.store(false, Ordering::SeqCst);
                if d.status == Status::Complete {
                    d.completion_action.take().map(|action| (d.id, action))
                } else {
                    None
                }
            };
            manager.persist();
            if let Some(action) = completion_action {
                manager.completion_actions.lock().unwrap().push(action);
            }
        });
    }

    async fn execute_with_retries(&self, job: &Arc<Job>) -> Result<()> {
        for attempt in 0.. {
            let result = self.execute(job).await;
            if attempt == DOWNLOAD_RETRIES || !result.as_ref().is_err_and(retryable_transfer_error)
            {
                return result;
            }
            {
                let mut data = job.data.lock().unwrap();
                check_running(job)?;
                data.status = Status::Retrying;
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(1 << attempt)) => {},
                _ = stopped(job) => bail!("Transfer stopped"),
            }
            let mut data = job.data.lock().unwrap();
            check_running(job)?;
            data.status = Status::Queued;
        }
        unreachable!()
    }

    async fn execute(&self, job: &Arc<Job>) -> Result<()> {
        let _permit = loop {
            check_running(job)?;
            let permit = {
                let jobs = self.jobs.lock().unwrap();
                let next = jobs
                    .iter()
                    .filter_map(|candidate| {
                        let d = candidate.data.lock().unwrap();
                        (d.status == Status::Queued
                            && candidate.control.load(Ordering::SeqCst) == RUN)
                            .then_some((std::cmp::Reverse(d.priority), d.id))
                    })
                    .min();
                let mut data = job.data.lock().unwrap();
                check_running(job)?;
                if !self.global_editing.load(Ordering::SeqCst)
                    && next.is_some_and(|(_, id)| id == data.id)
                {
                    // shortcut: global-capped external jobs run alone; use shared streaming when concurrency is needed.
                    let slots = if self.settings.lock().unwrap().speed_limit > 0
                        && !matches!(
                            data.protocol,
                            Protocol::Http | Protocol::WebDav | Protocol::Sftp
                        ) {
                        3
                    } else {
                        1
                    };
                    self.slots
                        .try_acquire_many(slots)
                        .ok()
                        .inspect(|_| data.status = Status::Connecting)
                } else {
                    None
                }
            };
            if let Some(permit) = permit {
                break permit;
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(50)) => {},
                _ = stopped(job) => bail!("Transfer stopped"),
            }
        };
        check_running(job)?;
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
                .pool_max_idle_per_host(16)
                .pool_idle_timeout(Duration::from_secs(90))
                .user_agent(concat!("101Downloader/", env!("CARGO_PKG_VERSION")))
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
        *job.name_hint.lock().unwrap() = naming::response_name(&probe, &initial);
        self.adopt_response_name(job)?;
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
        if ranged {
            let mut stream = probe.bytes_stream();
            let mut received = 0;
            while let Some(bytes) = tokio::select! { bytes = stream.next() => bytes, _ = stopped(job) => bail!("Transfer stopped") }
            {
                received += bytes?.len();
                ensure!(
                    received <= 1,
                    "The server returned too much data for the range probe"
                );
            }
            if received != 1 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "The range probe ended before its byte arrived",
                )
                .into());
            }
        } else {
            drop(probe);
        }
        check_running(job)?;
        let old = job.data.lock().unwrap().clone();
        let dir = old.parts_dir();
        let reuse = ranged
            && old.ranged
            && old.total == total
            && old.validator == validator
            && !old.parts.is_empty();
        if reuse {
            chunks::ordered_indices(&old.parts, total.unwrap())?;
        }
        fs::create_dir_all(&dir).context("Cannot create partial download folder")?;
        let parts = if ranged {
            let size = total.unwrap();
            if reuse {
                old.parts.clone()
            } else {
                chunks::plan(size, chunks::connections(Some(size), old.connections))
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
        chunks::download(job.clone(), client).await?;
        check_running(job)?;
        self.adopt_response_name(job)?;
        let d = job.data.lock().unwrap().clone();
        let ordered = if d.ranged {
            chunks::ordered_indices(&d.parts, d.total.unwrap())?
        } else {
            (0..d.parts.len()).collect()
        };
        ensure!(
            d.total.is_none_or(|n| n == d.downloaded),
            "Downloaded size does not match the server's file size"
        );
        for &index in &ordered {
            ensure!(
                fs::metadata(dir.join(format!("{index}.part")))?.len() == d.parts[index].downloaded,
                "A downloaded part no longer matches its recorded size"
            );
        }
        job.data.lock().unwrap().status = Status::Assembling;
        let integrity = tools::assemble_files(
            ordered
                .into_iter()
                .map(|i| dir.join(format!("{i}.part")))
                .collect(),
            d.path(),
        )
        .await?;
        {
            let mut data = job.data.lock().unwrap();
            data.total = Some(data.downloaded);
            data.status = Status::Complete;
            data.completed_at = Some(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs());
            data.owns_output = true;
            data.integrity = vec![integrity];
            data.integrity_baseline_only = false;
            data.error = None;
        }
        // This directory is exclusively owned by this transfer and contains only its parts.
        let _ = fs::remove_dir_all(dir);
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

fn retryable_transfer_error(error: &anyhow::Error) -> bool {
    use std::io::ErrorKind;
    error.chain().any(|cause| {
        cause.downcast_ref::<reqwest::Error>().is_some_and(|error| {
            error.is_timeout()
                || error.is_connect()
                || error.is_body()
                || error.is_decode()
                || matches!(
                    error.status().map(|status| status.as_u16()),
                    Some(408 | 429 | 500 | 502 | 503 | 504)
                )
        }) || cause.downcast_ref::<std::io::Error>().is_some_and(|error| {
            matches!(
                error.kind(),
                ErrorKind::ConnectionRefused
                    | ErrorKind::ConnectionReset
                    | ErrorKind::ConnectionAborted
                    | ErrorKind::TimedOut
                    | ErrorKind::UnexpectedEof
                    | ErrorKind::BrokenPipe
                    | ErrorKind::NotConnected
            )
        }) || cause.is::<tokio::time::error::Elapsed>()
            || cause.downcast_ref::<russh::Error>().is_some_and(|error| {
                matches!(
                    error,
                    russh::Error::ConnectionTimeout
                        | russh::Error::KeepaliveTimeout
                        | russh::Error::InactivityTimeout
                        | russh::Error::Disconnect
                )
            })
            || cause
                .downcast_ref::<russh_sftp::client::error::Error>()
                .is_some_and(|error| matches!(error, russh_sftp::client::error::Error::Timeout))
            || cause
                .downcast_ref::<tools::ToolExit>()
                .is_some_and(|error| matches!(error.code, 5 | 6 | 7 | 18 | 28 | 52 | 55 | 56))
    })
}

async fn download_part(
    client: Client,
    job: Arc<Job>,
    index: usize,
    split: &AtomicBool,
    abort: &AtomicBool,
) -> Result<chunks::Outcome> {
    for attempt in 0..4 {
        check_running(&job)?;
        ensure!(!abort.load(Ordering::SeqCst), "Another connection failed");
        let result = transfer_part(&client, &job, index, split, abort).await;
        if result.is_ok() {
            return result;
        }
        check_running(&job)?;
        if attempt == 3 || !result.as_ref().is_err_and(retryable_transfer_error) {
            return result;
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(400 * (1 << attempt))) => {},
            reason = chunks::interrupted(&job, split, abort) => match reason {
                chunks::Interrupt::Split => return Ok(chunks::Outcome::Split),
                chunks::Interrupt::Stop => bail!("Transfer stopped"),
            },
        }
    }
    unreachable!()
}

async fn transfer_part(
    client: &Client,
    job: &Arc<Job>,
    index: usize,
    split: &AtomicBool,
    abort: &AtomicBool,
) -> Result<chunks::Outcome> {
    let d = job.data.lock().unwrap().clone();
    let part = &d.parts[index];
    let path = d.parts_dir().join(format!("{index}.part"));
    let offset = if d.ranged {
        fs::metadata(&path)?.len()
    } else {
        0
    };
    if d.ranged && offset == part.len() {
        return Ok(chunks::Outcome::Complete);
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
        reason = chunks::interrupted(job, split, abort) => match reason {
            chunks::Interrupt::Split => return Ok(chunks::Outcome::Split),
            chunks::Interrupt::Stop => bail!("Transfer stopped"),
        },
    };
    response.error_for_status_ref()?;
    ensure_identity(&response)?;
    if let Some(candidate) = naming::response_name(&response, &d) {
        let mut hint = job.name_hint.lock().unwrap();
        if hint
            .as_ref()
            .is_none_or(|prior| candidate.priority > prior.priority)
        {
            *hint = Some(candidate);
        }
    }
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
            reason = chunks::interrupted(job, split, abort) => {
                file.flush().await?;
                match reason {
                    chunks::Interrupt::Split => return Ok(chunks::Outcome::Split),
                    chunks::Interrupt::Stop => bail!("Transfer stopped"),
                }
            },
        };
        let Some(chunk) = chunk else {
            break;
        };
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                file.flush().await?;
                return Err(error.into());
            }
        };
        if d.ranged && received + chunk.len() as u64 > part.len() {
            file.flush().await?;
            bail!("Server sent more bytes than requested");
        }
        if let Err(error) = job.throttle(chunk.len()).await {
            file.flush().await?;
            return Err(error);
        }
        file.write_all(&chunk).await?;
        received += chunk.len() as u64;
        let mut data = job.data.lock().unwrap();
        data.parts[index].downloaded = received;
        data.downloaded = data.parts.iter().map(|p| p.downloaded).sum();
    }
    file.flush().await?;
    let expected = if d.ranged {
        Some(part.len())
    } else {
        job.data.lock().unwrap().total
    };
    if expected.is_some_and(|size| received < size) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "Connection ended before all the expected bytes arrived",
        )
        .into());
    }
    ensure!(
        expected.is_none_or(|size| received == size),
        "Server sent more bytes than expected"
    );
    Ok(chunks::Outcome::Complete)
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
            if c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{200e}' | '\u{200f}') || "<>:\"/\\|?*".contains(c) {
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
    if let Some(directory) =
        std::env::var_os("DOWNLOADER101_DATA_DIR").or_else(|| std::env::var_os("STRATA_DATA_DIR"))
    {
        return PathBuf::from(directory).join("history.json");
    }
    let root = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    compatible_history(&root)
}

fn compatible_history(root: &Path) -> PathBuf {
    let current = root.join("101 Downloader").join("history.json");
    let legacy = root.join("Strata").join("history.json");
    if !current.exists() && legacy.exists() {
        legacy
    } else {
        current
    }
}

#[cfg(test)]
mod settings_tests {
    use super::Settings;

    #[test]
    fn rebrand_preserves_legacy_history_and_prefers_existing_new_history() {
        let directory = tempfile::tempdir().unwrap();
        let current = directory.path().join("101 Downloader/history.json");
        let legacy = directory.path().join("Strata/history.json");
        assert_eq!(super::compatible_history(directory.path()), current);
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, b"legacy history").unwrap();
        assert_eq!(super::compatible_history(directory.path()), legacy);
        std::fs::create_dir_all(current.parent().unwrap()).unwrap();
        std::fs::write(&current, b"new history").unwrap();
        assert_eq!(super::compatible_history(directory.path()), current);
        assert_eq!(std::fs::read(legacy).unwrap(), b"legacy history");
    }

    #[test]
    fn legacy_settings_default_panel_height_and_new_height_round_trips() {
        let mut settings: Settings = serde_json::from_value(
            serde_json::json!({ "directory": "Downloads", "connections": 8 }),
        )
        .unwrap();
        assert_eq!(settings.details_height, 240.);
        assert!(settings.completion_chime);
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
