use crate::engine::Manager;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::Path,
    sync::Arc,
};

pub const HOST_NAME: &str = "com.strata.download_manager";
pub const EXTENSION_ID: &str = include_str!("../extension/id.txt");
pub const MAX_MESSAGE: usize = 64 * 1024;
pub const BROWSER_REGISTRY_ROOTS: &str = include_str!("../resources/browser-registry.txt");

pub fn extension_directory() -> std::path::PathBuf {
    let installed = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|parent| parent.join("extension")));
    installed
        .filter(|path| path.join("manifest.json").is_file())
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("extension"))
}

#[cfg(windows)]
pub fn register_browsers() -> Result<()> {
    let bridge = std::env::current_exe()?.with_file_name("strata-bridge.exe");
    ensure!(
        bridge.is_file(),
        "Build strata-bridge.exe beside the Strata app first"
    );
    let directory = bridge.parent().unwrap().join("browser");
    std::fs::create_dir_all(&directory)?;
    let path = directory.join(format!("{HOST_NAME}.json"));
    let manifest = serde_json::json!({ "name": HOST_NAME, "description": "Strata local download handoff", "path": bridge,
        "type": "stdio", "allowed_origins": [format!("chrome-extension://{}/", EXTENSION_ID.trim())] });
    std::fs::write(&path, serde_json::to_vec_pretty(&manifest)?)?;
    for root in BROWSER_REGISTRY_ROOTS
        .lines()
        .filter(|line| !line.is_empty())
    {
        windows_registry::CURRENT_USER
            .create(format!(r"{root}\{HOST_NAME}"))?
            .set_string("", path.to_string_lossy())?;
    }
    Ok(())
}

#[cfg(windows)]
pub fn unregister_browsers() -> Result<()> {
    let manifest = std::env::current_exe()?
        .parent()
        .unwrap()
        .join("browser")
        .join(format!("{HOST_NAME}.json"));
    for root in BROWSER_REGISTRY_ROOTS
        .lines()
        .filter(|line| !line.is_empty())
    {
        let path = format!(r"{root}\{HOST_NAME}");
        let owned = windows_registry::CURRENT_USER
            .open(&path)
            .and_then(|key| key.get_string(""))
            .is_ok_and(|value| value.eq_ignore_ascii_case(&manifest.to_string_lossy()));
        if owned {
            windows_registry::CURRENT_USER.remove_tree(&path)?;
        }
    }
    Ok(())
}

#[cfg(windows)]
pub fn browsers_registered() -> bool {
    BROWSER_REGISTRY_ROOTS
        .lines()
        .filter(|line| !line.is_empty())
        .all(|root| {
            windows_registry::CURRENT_USER
                .open(format!(r"{root}\{HOST_NAME}"))
                .and_then(|key| key.get_string(""))
                .is_ok_and(|path| Path::new(&path).is_file())
        })
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Ping,
    Open,
    Download {
        url: String,
        #[serde(default)]
        filename: Option<String>,
        #[serde(default)]
        referrer: Option<String>,
    },
    Transfer {
        source: String,
        directory: Option<std::path::PathBuf>,
        connections: Option<usize>,
        options: Box<crate::protocol::TransferOptions>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Reply {
    pub ok: bool,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Reply {
    pub fn error(error: impl std::fmt::Display) -> Self {
        Self {
            ok: false,
            version: env!("CARGO_PKG_VERSION").into(),
            id: None,
            error: Some(error.to_string()),
        }
    }
}

pub fn read_message(reader: &mut impl Read) -> Result<Request> {
    let mut length = [0; 4];
    reader
        .read_exact(&mut length)
        .context("Missing native-message length")?;
    let length = u32::from_le_bytes(length) as usize;
    ensure!(
        (1..=MAX_MESSAGE).contains(&length),
        "Invalid native-message size"
    );
    let mut bytes = vec![0; length];
    reader
        .read_exact(&mut bytes)
        .context("Truncated native message")?;
    serde_json::from_slice(&bytes).context("Invalid Strata request")
}

pub fn write_message(writer: &mut impl Write, reply: &Reply) -> Result<()> {
    let bytes = serde_json::to_vec(reply)?;
    ensure!(bytes.len() <= MAX_MESSAGE, "Reply is too large");
    writer.write_all(&(bytes.len() as u32).to_le_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()?;
    Ok(())
}

fn dispatch(manager: &Arc<Manager>, request: Request) -> Reply {
    let result = match request {
        Request::Ping => Ok(None),
        Request::Open => {
            manager
                .show_requested
                .store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(None)
        }
        Request::Download {
            url,
            filename,
            referrer,
        } => manager
            .add_from_browser(&url, filename.as_deref(), referrer.as_deref())
            .map(Some),
        Request::Transfer {
            source,
            directory,
            connections,
            options,
        } => {
            let settings = manager.settings.lock().unwrap().clone();
            let result = manager
                .add_with_options(
                    &source,
                    directory.unwrap_or(settings.directory),
                    connections.unwrap_or(settings.connections),
                    *options,
                )
                .map(Some);
            if result.is_ok() {
                manager
                    .show_requested
                    .store(true, std::sync::atomic::Ordering::SeqCst);
            }
            result
        }
    };
    match result {
        Ok(id) => Reply {
            ok: true,
            version: env!("CARGO_PKG_VERSION").into(),
            id,
            error: None,
        },
        Err(error) => Reply::error(format!("{error:#}")),
    }
}

#[cfg(windows)]
pub use platform::{BrowserListener, send, start};

#[cfg(windows)]
mod platform {
    use super::*;
    use std::{
        hash::{DefaultHasher, Hash, Hasher},
        process::{Command, Stdio},
        time::Duration,
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions},
    };
    use windows::{
        Win32::{
            Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree},
            Security::{
                Authorization::{
                    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
                },
                GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
                TOKEN_USER, TokenUser,
            },
            System::Threading::{GetCurrentProcess, OpenProcessToken},
        },
        core::{HSTRING, PWSTR},
    };

    pub struct BrowserListener(tokio::task::JoinHandle<()>);
    impl Drop for BrowserListener {
        fn drop(&mut self) {
            self.0.abort();
        }
    }

    fn current_sid() -> Result<String> {
        unsafe {
            let mut token = HANDLE::default();
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)?;
            let result = (|| -> Result<String> {
                let mut length = 0;
                let _ = GetTokenInformation(token, TokenUser, None, 0, &mut length);
                ensure!(length > 0, "Cannot read Windows user identity");
                // Word alignment is sufficient for TOKEN_USER and its embedded SID.
                let mut buffer =
                    vec![0usize; (length as usize).div_ceil(std::mem::size_of::<usize>())];
                GetTokenInformation(
                    token,
                    TokenUser,
                    Some(buffer.as_mut_ptr().cast()),
                    length,
                    &mut length,
                )?;
                let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
                let mut sid = PWSTR::null();
                ConvertSidToStringSidW(user.User.Sid, &mut sid)?;
                let result = sid.to_string().context("Invalid Windows user SID");
                LocalFree(Some(HLOCAL(sid.0.cast())));
                result
            })();
            let _ = CloseHandle(token);
            result
        }
    }

    fn pipe_name(history: &Path) -> Result<(String, String)> {
        let sid = current_sid()?;
        let mut hash = DefaultHasher::new();
        let parent = history.parent().context("Missing history folder")?;
        std::fs::create_dir_all(parent)?;
        let normalized = parent
            .canonicalize()?
            .join(history.file_name().context("Missing history filename")?);
        normalized.to_string_lossy().to_lowercase().hash(&mut hash);
        Ok((
            format!(r"\\.\pipe\Strata-{sid}-{:016x}", hash.finish()),
            sid,
        ))
    }

    fn create_server(name: &str, sid: &str, first: bool) -> Result<NamedPipeServer> {
        unsafe {
            let mut descriptor = PSECURITY_DESCRIPTOR::default();
            let sddl = HSTRING::from(format!("D:P(A;;GA;;;SY)(A;;GA;;;{sid})"));
            ConvertStringSecurityDescriptorToSecurityDescriptorW(&sddl, 1, &mut descriptor, None)?;
            let mut attributes = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor.0,
                bInheritHandle: false.into(),
            };
            // Windows copies the descriptor during pipe creation; it never crosses an await.
            let result = ServerOptions::new()
                .first_pipe_instance(first)
                .reject_remote_clients(true)
                .create_with_security_attributes_raw(
                    name,
                    (&mut attributes as *mut SECURITY_ATTRIBUTES).cast(),
                );
            LocalFree(Some(HLOCAL(descriptor.0)));
            result.context("Cannot open the Strata browser connection")
        }
    }

    pub fn start(manager: Arc<Manager>, history: &Path) -> Result<BrowserListener> {
        let (name, sid) = pipe_name(history)?;
        let runtime = manager.runtime_handle();
        let server = {
            let _entered = runtime.enter();
            create_server(&name, &sid, true)?
        };
        Ok(BrowserListener(runtime.spawn(async move {
            let mut server = server;
            loop {
                if let Err(error) = server.connect().await {
                    *manager.browser_error.lock().unwrap() = Some(error.to_string());
                    break;
                }
                let next = match create_server(&name, &sid, false) {
                    Ok(next) => next,
                    Err(error) => {
                        *manager.browser_error.lock().unwrap() = Some(error.to_string());
                        break;
                    }
                };
                let connected = std::mem::replace(&mut server, next);
                let manager = manager.clone();
                tokio::spawn(async move {
                    let _ =
                        tokio::time::timeout(Duration::from_secs(10), handle(connected, manager))
                            .await;
                });
            }
        })))
    }

    async fn handle(mut pipe: NamedPipeServer, manager: Arc<Manager>) -> Result<()> {
        let length = pipe.read_u32_le().await? as usize;
        ensure!(
            (1..=MAX_MESSAGE).contains(&length),
            "Invalid bridge-message size"
        );
        let mut bytes = vec![0; length];
        pipe.read_exact(&mut bytes).await?;
        let reply = match serde_json::from_slice(&bytes) {
            Ok(request) => dispatch(&manager, request),
            Err(error) => Reply::error(error),
        };
        let bytes = serde_json::to_vec(&reply)?;
        pipe.write_u32_le(bytes.len() as u32).await?;
        pipe.write_all(&bytes).await?;
        pipe.flush().await?;
        Ok(())
    }

    pub async fn send(request: &Request, history: &Path, launch: bool) -> Result<Reply> {
        let (name, _) = pipe_name(history)?;
        let mut launched = false;
        let start = std::time::Instant::now();
        let mut pipe = loop {
            match ClientOptions::new().open(&name) {
                Ok(pipe) => break pipe,
                Err(error) if matches!(error.raw_os_error(), Some(2 | 231)) => {
                    if !launch {
                        anyhow::bail!("Open Strata to connect the extension");
                    }
                    if error.raw_os_error() == Some(2) && !launched {
                        let app = std::env::current_exe()?.with_file_name("strata.exe");
                        ensure!(
                            app.is_file(),
                            "Strata app is missing beside its browser bridge"
                        );
                        Command::new(app)
                            .arg("--from-browser")
                            .stdin(Stdio::null())
                            .stdout(Stdio::null())
                            .stderr(Stdio::null())
                            .spawn()?;
                        launched = true;
                    }
                    ensure!(
                        start.elapsed() < Duration::from_secs(15),
                        "Strata did not open in time; the download will stay in Chrome"
                    );
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(error) => return Err(error.into()),
            }
        };
        let bytes = serde_json::to_vec(request)?;
        ensure!(bytes.len() <= MAX_MESSAGE, "Request is too large");
        pipe.write_u32_le(bytes.len() as u32).await?;
        pipe.write_all(&bytes).await?;
        let reply = tokio::time::timeout(Duration::from_secs(20), async {
            let length = pipe.read_u32_le().await? as usize;
            ensure!(
                (1..=MAX_MESSAGE).contains(&length),
                "Invalid app reply size"
            );
            let mut bytes = vec![0; length];
            pipe.read_exact(&mut bytes).await?;
            serde_json::from_slice(&bytes).context("Invalid app reply")
        })
        .await
        .context("Strata did not acknowledge the transfer")??;
        Ok(reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_native_message_framing() {
        let body = br#"{"action":"ping"}"#;
        let mut frame = (body.len() as u32).to_le_bytes().to_vec();
        frame.extend(body);
        assert!(matches!(
            read_message(&mut frame.as_slice()).unwrap(),
            Request::Ping
        ));
        assert!(read_message(&mut (MAX_MESSAGE as u32 + 1).to_le_bytes().as_slice()).is_err());
        assert!(read_message(&mut &frame[..6]).is_err());
        assert!(
            serde_json::from_str::<Request>(r#"{"action":"execute","command":"bad"}"#).is_err()
        );
        let mut reply = vec![];
        write_message(&mut reply, &Reply::error("test")).unwrap();
        assert_eq!(
            u32::from_le_bytes(reply[..4].try_into().unwrap()) as usize,
            reply.len() - 4
        );
    }

    #[cfg(windows)]
    #[test]
    fn first_connection_prepares_a_new_data_folder() {
        let directory = tempfile::tempdir().unwrap();
        let history = directory.path().join("new-user-data/history.json");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let error = runtime
            .block_on(send(&Request::Ping, &history, false))
            .unwrap_err();
        assert!(error.to_string().contains("Open Strata"));
        assert!(history.parent().unwrap().is_dir());
    }

    #[cfg(windows)]
    #[test]
    fn named_pipe_accepts_only_valid_download_requests() {
        let dir = tempfile::tempdir().unwrap();
        let history = dir.path().join("history.json");
        let manager = Manager::new(history.clone()).unwrap();
        manager.settings.lock().unwrap().directory = dir.path().into();
        let listener = start(manager.clone(), &history).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let reply = rt.block_on(send(&Request::Ping, &history, false)).unwrap();
        assert!(reply.ok);
        let reply = rt.block_on(send(&Request::Open, &history, false)).unwrap();
        assert!(reply.ok);
        assert!(
            manager
                .show_requested
                .swap(false, std::sync::atomic::Ordering::SeqCst)
        );
        let alternate = std::path::PathBuf::from(history.to_string_lossy().replace('\\', "/"));
        assert!(
            rt.block_on(send(&Request::Ping, &alternate, false))
                .unwrap()
                .ok
        );
        let reply = rt
            .block_on(send(
                &Request::Download {
                    url: "file:///private".into(),
                    filename: None,
                    referrer: None,
                },
                &history,
                false,
            ))
            .unwrap();
        assert!(!reply.ok);
        assert!(manager.snapshots().is_empty());
        let reply = rt
            .block_on(send(
                &Request::Transfer {
                    source: "http://127.0.0.1:9/cli-file.bin".into(),
                    directory: Some(dir.path().into()),
                    connections: Some(4),
                    options: Box::default(),
                },
                &history,
                false,
            ))
            .unwrap();
        assert!(reply.ok, "{:?}", reply.error);
        let id = reply.id.unwrap();
        assert_eq!(
            manager
                .snapshots()
                .into_iter()
                .find(|d| d.id == id)
                .unwrap()
                .connections,
            4
        );
        assert!(
            manager
                .show_requested
                .swap(false, std::sync::atomic::Ordering::SeqCst)
        );
        manager.cancel(id);
        drop(listener);
        manager.shutdown();
    }
}
