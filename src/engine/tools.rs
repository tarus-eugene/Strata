use super::*;
use std::ffi::{OsStr, OsString};
use tokio::{io::AsyncReadExt, process::Command};

#[cfg(windows)]
pub(super) struct ProcessTree(windows::Win32::Foundation::HANDLE);
// Kernel handles are owned here and can be moved and closed on any thread.
#[cfg(windows)]
unsafe impl Send for ProcessTree {}
#[cfg(windows)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.0);
        }
    }
}
#[cfg(not(windows))]
pub(super) struct ProcessTree;

pub(super) fn spawn(command: &mut Command) -> Result<(tokio::process::Child, ProcessTree)> {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::{Diagnostics::ToolHelp::*, JobObjects::*, Threading::*};
        let tree = ProcessTree(CreateJobObjectW(None, None)?);
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            tree.0,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as _,
            std::mem::size_of_val(&limits) as u32,
        )?;
        command.creation_flags(CREATE_NO_WINDOW.0 | CREATE_SUSPENDED.0);
        let child = command.spawn()?;
        let pid = child.id().context("Transfer process has already exited")?;
        let process = ProcessTree(OpenProcess(
            PROCESS_SET_QUOTA | PROCESS_TERMINATE,
            false,
            pid,
        )?);
        AssignProcessToJobObject(tree.0, process.0)
            .context("Cannot isolate the transfer process tree")?;
        let snapshot = ProcessTree(CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0)?);
        let mut entry = THREADENTRY32 {
            dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        Thread32First(snapshot.0, &mut entry)?;
        loop {
            if entry.th32OwnerProcessID == pid {
                let thread = ProcessTree(OpenThread(
                    THREAD_SUSPEND_RESUME,
                    false,
                    entry.th32ThreadID,
                )?);
                ensure!(
                    ResumeThread(thread.0) != u32::MAX,
                    "Cannot resume the transfer process"
                );
                return Ok((child, tree));
            }
            Thread32Next(snapshot.0, &mut entry)
                .context("Transfer process has no initial thread")?;
        }
    }
    #[cfg(not(windows))]
    {
        Ok((command.spawn()?, ProcessTree))
    }
}

pub(super) fn executable(name: &str) -> Result<PathBuf> {
    let exe_name = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    };
    #[cfg(debug_assertions)]
    {
        let cached = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(".tools/media-tools")
            .join(&exe_name);
        if cached.is_file() {
            return Ok(cached);
        }
    }
    if let Some(parent) = std::env::current_exe()?.parent() {
        for candidate in [parent.join("tools").join(&exe_name), parent.join(&exe_name)] {
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    if name == "curl"
        && let Some(root) = std::env::var_os("SystemRoot")
    {
        let candidate = PathBuf::from(root).join("System32").join(&exe_name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path) {
            let candidate = directory.join(&exe_name);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    bail!(
        "{exe_name} is missing. Install 101 Downloader's media tools or put the tool in the app's tools folder or PATH."
    )
}

pub(super) async fn run(
    job: &Arc<Job>,
    executable: &Path,
    args: &[OsString],
    input: &[u8],
    progress: Option<&Path>,
) -> Result<Vec<u8>> {
    use std::process::Stdio;
    let mut command = Command::new(executable);
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let (mut child, tree) = spawn(&mut command).context("Cannot start the transfer tool")?;
    let mut stdin = child.stdin.take().unwrap();
    tokio::select! { r = stdin.write_all(input) => r?, _ = stopped(job) => bail!("Transfer stopped") }
    drop(stdin);
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let result = {
        let execution = async {
            let (status, out, err) = tokio::try_join!(
                async { Ok::<_, anyhow::Error>(child.wait().await?) },
                limited(stdout),
                limited(stderr)
            )?;
            if !status.success() {
                let mut message = String::from_utf8_lossy(&err).to_string();
                let options = &job.data.lock().unwrap().options;
                for secret in [
                    options.password.as_ref(),
                    options.key_passphrase.as_ref(),
                    options.proxy.as_ref().and_then(|p| p.password.as_ref()),
                ]
                .into_iter()
                .flatten()
                {
                    secret.redact(&mut message)?;
                }
                message = message.chars().take(8192).collect();
                bail!(
                    "Transfer tool failed ({}): {}",
                    status.code().unwrap_or(-1),
                    message.trim()
                );
            }
            Ok(out)
        };
        tokio::pin!(execution);
        let mut interval = tokio::time::interval(Duration::from_millis(200));
        loop {
            tokio::select! {
                r = &mut execution => break r,
                _ = stopped(job) => break Err(anyhow::anyhow!("Transfer stopped")),
                _ = interval.tick() => if let Some(path) = progress { update_file_progress(job, fs::metadata(path).map_or(0, |m| m.len())); },
            }
        }
    };
    drop(tree);
    if result.is_err() {
        let _ = child.kill().await;
    }
    result
}

async fn limited(reader: impl tokio::io::AsyncRead + Unpin) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(1024 * 1024 + 1).read_to_end(&mut bytes).await?;
    ensure!(
        bytes.len() <= 1024 * 1024,
        "Transfer tool returned too much diagnostic output"
    );
    Ok(bytes)
}

pub(super) fn config_value(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\r', "\\r")
            .replace('\n', "\\n")
            .replace('\t', "\\t")
    )
}

pub(super) fn begin_file(
    job: &Arc<Job>,
    total: Option<u64>,
    validator: Option<String>,
) -> Result<PathBuf> {
    let mut d = job.data.lock().unwrap();
    fs::create_dir_all(d.parts_dir())?;
    let path = d.parts_dir().join("payload.part");
    let old_len = fs::metadata(&path).map_or(0, |m| m.len());
    let reuse = total.is_some()
        && total == d.total
        && validator == d.validator
        && old_len <= total.unwrap();
    if !reuse {
        fs::File::create(&path)?;
    }
    let offset = if reuse { old_len } else { 0 };
    d.total = total;
    d.validator = validator;
    d.ranged = total.is_some();
    d.downloaded = offset;
    d.parts = vec![Part {
        start: 0,
        end: total.unwrap_or(1).saturating_sub(1),
        downloaded: offset,
    }];
    d.status = Status::Downloading;
    Ok(path)
}

pub(super) fn update_file_progress(job: &Arc<Job>, bytes: u64) {
    let mut data = job.data.lock().unwrap();
    data.downloaded = bytes;
    if let Some(part) = data.parts.first_mut() {
        part.downloaded = bytes;
    }
}

pub(super) async fn assemble_files(
    parts: Vec<PathBuf>,
    destination: PathBuf,
) -> Result<IntegrityFile> {
    // Batch disk I/O on one worker instead of scheduling each small read and write separately.
    tokio::task::spawn_blocking(move || -> Result<IntegrityFile> {
        use sha2::{Digest, Sha256};
        use std::io::{Read, Write};
        let directory = destination.parent().context("Missing download folder")?;
        let mut output = tempfile::NamedTempFile::new_in(directory)?;
        let mut hash = Sha256::new();
        let mut buffer = vec![0; 1024 * 1024];
        let mut size = 0;
        for path in parts {
            let mut reader = fs::File::open(path)?;
            let expected = reader.metadata()?.len();
            let mut copied = 0;
            loop {
                let count = reader.read(&mut buffer)?;
                if count == 0 { break; }
                output.as_file_mut().write_all(&buffer[..count])?;
                hash.update(&buffer[..count]);
                copied += count as u64;
            }
            ensure!(copied == expected, "A download part changed during assembly");
            size += copied;
        }
        output.as_file().sync_all()?;
        let fingerprint = IntegrityFile { path: PathBuf::new(), size, sha256: format!("{:x}", hash.finalize()) };
        integrity::verify(output.path(), std::slice::from_ref(&fingerprint))?;
        output.persist_noclobber(destination).context(
            "Cannot save the finished file. A file with this name may already exist; move it and retry.",
        )?;
        Ok(fingerprint)
    })
    .await?
}

pub(super) async fn finish_file(job: &Arc<Job>, path: &Path) -> Result<()> {
    check_running(job)?;
    let size = fs::metadata(path)?.len();
    let d = job.data.lock().unwrap().clone();
    ensure!(
        d.total.is_none_or(|total| total == size),
        "The server's file size does not match the downloaded file"
    );
    job.data.lock().unwrap().status = Status::Assembling;
    let integrity = assemble_files(vec![path.to_owned()], d.path()).await?;
    let _ = fs::remove_file(path);
    let _ = fs::remove_dir(d.parts_dir());
    {
        let mut data = job.data.lock().unwrap();
        data.total = Some(size);
        data.downloaded = size;
        data.status = Status::Complete;
        data.completed_at = Some(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs());
        data.owns_output = true;
        data.integrity = vec![integrity];
        data.integrity_baseline_only = false;
        data.error = None;
    }
    Ok(())
}

pub(super) async fn publish_directory(job: &Arc<Job>, source: &Path) -> Result<()> {
    check_running(job)?;
    let d = job.data.lock().unwrap().clone();
    job.data.lock().unwrap().status = Status::Verifying;
    let root = source.to_owned();
    let integrity = tokio::task::spawn_blocking(move || integrity::capture(&root)).await??;
    let size = integrity
        .iter()
        .try_fold(0_u64, |sum, file| sum.checked_add(file.size))
        .context("Download size is too large")?;
    ensure!(
        d.total.is_none_or(|total| total == size),
        "Downloaded files do not match the expected total size"
    );
    check_running(job)?;
    job.data.lock().unwrap().status = Status::Assembling;
    #[cfg(windows)]
    unsafe {
        use std::os::windows::ffi::OsStrExt;
        use windows::{
            Win32::Storage::FileSystem::{MOVE_FILE_FLAGS, MoveFileExW},
            core::PCWSTR,
        };
        let from: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<u16> = d.path().as_os_str().encode_wide().chain(Some(0)).collect();
        MoveFileExW(
            PCWSTR(from.as_ptr()),
            PCWSTR(to.as_ptr()),
            MOVE_FILE_FLAGS(0),
        )
        .context(
            "Cannot publish the download folder; a destination with this name may already exist",
        )?;
    }
    #[cfg(not(windows))]
    {
        ensure!(!d.path().exists(), "The destination already exists");
        fs::rename(source, d.path())?;
    }
    let _ = fs::remove_dir_all(d.parts_dir());
    let mut data = job.data.lock().unwrap();
    data.total = Some(size);
    data.downloaded = size;
    data.status = Status::Complete;
    data.completed_at = Some(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs());
    data.owns_output = true;
    data.integrity = integrity;
    data.integrity_baseline_only = d.protocol == Protocol::Video;
    data.error = None;
    Ok(())
}

impl Manager {
    pub(super) async fn descriptor(&self, job: &Arc<Job>) -> Result<Vec<u8>> {
        const LIMIT: usize = 8 * 1024 * 1024;
        let d = job.data.lock().unwrap().clone();
        let url = Url::parse(&d.url)?;
        if url.scheme() == "file" {
            let path = url
                .to_file_path()
                .map_err(|_| anyhow::anyhow!("Invalid descriptor path"))?;
            ensure!(
                fs::metadata(&path)?.len() <= LIMIT as u64,
                "Descriptor exceeds the 8 MiB limit"
            );
            return Ok(tokio::fs::read(path).await?);
        }
        let client = self.client_for(&d.options)?;
        let response = tokio::select! { r = download_request(&client, &d)?.send() => r?, _ = stopped(job) => bail!("Transfer stopped") };
        response.error_for_status_ref()?;
        ensure_identity(&response)?;
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        loop {
            let chunk = tokio::select! { r = stream.next() => r, _ = stopped(job) => bail!("Transfer stopped") };
            let Some(chunk) = chunk else {
                break;
            };
            let chunk = chunk?;
            ensure!(
                bytes.len() + chunk.len() <= LIMIT,
                "Descriptor exceeds the 8 MiB limit"
            );
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
}

pub(super) fn args(values: &[&str]) -> Vec<OsString> {
    values
        .iter()
        .map(OsStr::new)
        .map(OsStr::to_os_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "manual disk benchmark: copies 256 MiB twice"]
    async fn assembly_throughput() {
        use std::{io::Write, time::Instant};
        let directory = tempfile::tempdir().unwrap();
        let mut parts = Vec::new();
        let buffer = vec![0x5a; 1024 * 1024];
        for i in 0..4 {
            let path = directory.path().join(format!("{i}.part"));
            let mut file = fs::File::create(&path).unwrap();
            for _ in 0..64 {
                file.write_all(&buffer).unwrap();
            }
            file.sync_all().unwrap();
            parts.push(path);
        }
        let started = Instant::now();
        let mut old = tokio::fs::File::create(directory.path().join("old.bin"))
            .await
            .unwrap();
        for path in &parts {
            let mut source = tokio::fs::File::open(path).await.unwrap();
            tokio::io::copy(&mut source, &mut old).await.unwrap();
        }
        old.flush().await.unwrap();
        old.sync_all().await.unwrap();
        let old_elapsed = started.elapsed();
        let started = Instant::now();
        let destination = directory.path().join("new.bin");
        assemble_files(parts, destination.clone()).await.unwrap();
        let new_elapsed = started.elapsed();
        assert_eq!(fs::metadata(destination).unwrap().len(), 256 * 1024 * 1024);
        eprintln!("256 MiB assembly: old={old_elapsed:?}, new={new_elapsed:?}");
    }

    #[tokio::test]
    async fn assembly_preserves_order_across_buffers_and_handles_empty_files() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("1.part");
        let second = directory.path().join("0.part");
        let empty = directory.path().join("empty.part");
        let destination = directory.path().join("finished.bin");
        let mut expected = (0..2 * 1024 * 1024 + 17)
            .map(|i| (i % 251) as u8)
            .collect::<Vec<_>>();
        fs::write(&first, &expected).unwrap();
        fs::write(&second, b"last bytes").unwrap();
        fs::write(&empty, []).unwrap();
        assemble_files(vec![first, empty.clone(), second], destination.clone())
            .await
            .unwrap();
        expected.extend_from_slice(b"last bytes");
        assert_eq!(fs::read(destination).unwrap(), expected);

        let destination = directory.path().join("empty.bin");
        assemble_files(vec![empty], destination.clone())
            .await
            .unwrap();
        assert_eq!(fs::metadata(destination).unwrap().len(), 0);
    }

    #[tokio::test]
    async fn assembly_failure_keeps_parts_and_never_overwrites_destination() {
        let directory = tempfile::tempdir().unwrap();
        let part = directory.path().join("0.part");
        let missing = directory.path().join("missing.part");
        let destination = directory.path().join("finished.bin");
        fs::write(&part, b"downloaded bytes").unwrap();
        assert!(
            assemble_files(vec![part.clone(), missing], destination.clone())
                .await
                .is_err()
        );
        assert!(!destination.exists());
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);

        fs::write(&destination, b"keep me").unwrap();
        assert!(
            assemble_files(vec![part.clone()], destination.clone())
                .await
                .is_err()
        );
        assert_eq!(fs::read(&destination).unwrap(), b"keep me");
        assert_eq!(fs::read(&part).unwrap(), b"downloaded bytes");
        fs::remove_file(&destination).unwrap();
        assemble_files(vec![part], destination.clone())
            .await
            .unwrap();
        assert_eq!(fs::read(destination).unwrap(), b"downloaded bytes");
    }

    #[test]
    fn curl_config_escapes_quotes_and_line_breaks() {
        assert_eq!(super::config_value("a\"\nb\\c"), "\"a\\\"\\nb\\\\c\"");
    }
}
