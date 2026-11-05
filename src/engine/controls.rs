use super::*;
use std::sync::atomic::AtomicU64;

pub(super) struct RateLimiter {
    limit: AtomicU64,
    gate: tokio::sync::Mutex<()>,
}

impl RateLimiter {
    pub(super) fn new(limit: u64) -> Self {
        Self {
            limit: AtomicU64::new(limit),
            gate: tokio::sync::Mutex::new(()),
        }
    }

    pub(super) async fn consume(&self, job: &Job, bytes: usize) -> Result<()> {
        if self.limit.load(Ordering::SeqCst) == 0 {
            return Ok(());
        }
        let _guard = tokio::select! {
            guard = self.gate.lock() => guard,
            _ = stopped(job) => bail!("Transfer stopped"),
        };
        let mut remaining = bytes as u64;
        while remaining > 0 {
            let limit = self.limit.load(Ordering::SeqCst);
            if limit == 0 {
                break;
            }
            let count = remaining.min((limit / 10).clamp(1, 64 * 1024));
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs_f64(count as f64 / limit as f64)) => {},
                _ = stopped(job) => bail!("Transfer stopped"),
            }
            remaining -= count;
        }
        Ok(())
    }
}

impl Job {
    pub(super) async fn throttle(&self, bytes: usize) -> Result<()> {
        self.limiter.consume(self, bytes).await?;
        self.global_limiter.consume(self, bytes).await
    }

    pub(super) fn effective_speed_limit(&self) -> u64 {
        let local = self.limiter.limit.load(Ordering::SeqCst);
        let global = self.global_limiter.limit.load(Ordering::SeqCst);
        match (local, global) {
            (0, limit) | (limit, 0) => limit,
            _ => local.min(global),
        }
    }
}

fn validate_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty() && sanitize_filename(name) == name,
        "Enter a valid filename without paths, reserved names, or trailing spaces or dots (up to 160 characters)."
    );
    Ok(())
}

pub(super) fn move_without_replacing(from: &Path, to: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::{
            Win32::Storage::FileSystem::{MOVE_FILE_FLAGS, MoveFileExW},
            core::PCWSTR,
        };
        let from = from
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let to = to
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        unsafe {
            MoveFileExW(
                PCWSTR(from.as_ptr()),
                PCWSTR(to.as_ptr()),
                MOVE_FILE_FLAGS(0),
            )?;
        }
    }
    #[cfg(not(windows))]
    {
        if from.is_dir() {
            ensure!(!to.try_exists()?, "The destination already exists");
            fs::rename(from, to)?;
        } else {
            fs::hard_link(from, to)?;
            fs::remove_file(from)?;
        }
    }
    Ok(())
}

impl Manager {
    pub fn set_global_speed_limit(
        self: &Arc<Self>,
        limit: u64,
    ) -> tokio::task::JoinHandle<Result<()>> {
        let manager = self.clone();
        self.runtime.spawn(async move {
            ensure!(
                limit == 0 || (16 * 1024..=u32::MAX as u64).contains(&limit),
                "The global limit must be at least 16 KiB/s, or 0 for unlimited."
            );
            let _guard = manager.controls_lock.lock().await;
            manager.global_editing.store(true, Ordering::SeqCst);
            let jobs = manager.jobs.lock().unwrap().clone();
            let mut restart = Vec::new();
            for job in &jobs {
                let data = job.data.lock().unwrap();
                if matches!(
                    data.status,
                    Status::Queued | Status::Connecting | Status::Downloading
                ) {
                    restart.push(data.id);
                }
                job.editing.store(true, Ordering::SeqCst);
                let id = data.id;
                drop(data);
                manager.pause(id);
            }
            for job in &jobs {
                while job.running.load(Ordering::SeqCst) {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }
            let original = manager.settings.lock().unwrap().speed_limit;
            manager.settings.lock().unwrap().speed_limit = limit;
            manager.global_limiter.limit.store(limit, Ordering::SeqCst);
            manager.persist();
            let error = manager.persistence_error.lock().unwrap().clone();
            if error.is_some() {
                manager.settings.lock().unwrap().speed_limit = original;
                manager
                    .global_limiter
                    .limit
                    .store(original, Ordering::SeqCst);
            }
            for job in &jobs {
                job.editing.store(false, Ordering::SeqCst);
            }
            manager.global_editing.store(false, Ordering::SeqCst);
            for id in restart {
                if manager
                    .find(id)
                    .is_some_and(|job| job.control.load(Ordering::SeqCst) == PAUSE)
                {
                    manager.resume(id);
                }
            }
            match error {
                Some(error) => Err(anyhow::anyhow!(error)),
                None => Ok(()),
            }
        })
    }

    pub fn set_priority(&self, ids: &[u64], priority: Priority) {
        for id in ids {
            if let Some(job) = self.find(*id) {
                job.data.lock().unwrap().priority = priority;
            }
        }
        self.persist();
    }

    pub fn change_download(
        self: &Arc<Self>,
        id: u64,
        change: DownloadChange,
    ) -> tokio::task::JoinHandle<Result<()>> {
        let manager = self.clone();
        self.runtime.spawn(async move {
            let _guard = manager.controls_lock.lock().await;
            let job = manager.find(id).context("This download no longer exists")?;
            match &change {
                DownloadChange::Rename(name) => validate_name(name)?,
                DownloadChange::SpeedLimit(limit) => {
                    ensure!(*limit <= u32::MAX as u64, "The speed limit is too large.");
                    ensure!(job.data.lock().unwrap().protocol != Protocol::Torrent || *limit == 0 || *limit >= 16 * 1024,
                        "Torrent downloads need a limit of at least 16 KiB/s.");
                }
            }
            let restart = {
                let data = job.data.lock().unwrap();
                ensure!(!job.deleting.load(Ordering::SeqCst), "Wait for deletion to finish.");
                ensure!(!job.editing.swap(true, Ordering::SeqCst), "This download is already being changed.");
                matches!(data.status, Status::Queued | Status::Connecting | Status::Downloading)
            };
            let result = async {
                manager.pause(id);
                while job.running.load(Ordering::SeqCst) { tokio::time::sleep(Duration::from_millis(20)).await; }
                let _save = manager.save_lock.lock().unwrap();
                let original = job.data.lock().unwrap().clone();
                let mut renamed = false;
                match change {
                    DownloadChange::Rename(name) => {
                        validate_name(&original.name)?;
                        if name != original.name {
                            let target = original.directory.join(&name);
                            let jobs = manager.jobs.lock().unwrap();
                            ensure!(!target.try_exists()?, "A file or folder with that name already exists.");
                            let reserved = jobs.iter().any(|other| { let other = other.data.lock().unwrap(); other.id != id && other.path().to_string_lossy().eq_ignore_ascii_case(&target.to_string_lossy()) });
                            ensure!(!reserved, "Another download already uses that name.");
                            if original.owns_output {
                                let metadata = fs::symlink_metadata(original.path()).context("Cannot access the downloaded file")?;
                                ensure!(!metadata.file_type().is_symlink(), "The downloaded path is a link; rename it manually.");
                                move_without_replacing(&original.path(), &target).context("Cannot rename the downloaded file")?;
                                renamed = true;
                            }
                            let mut data = job.data.lock().unwrap();
                            data.name = name;
                            data.preserve_name = true;
                        }
                    }
                    DownloadChange::SpeedLimit(limit) => {
                        job.data.lock().unwrap().speed_limit = limit;
                        job.limiter.limit.store(limit, Ordering::SeqCst);
                    }
                }
                manager.persist_locked();
                let save_error = manager.persistence_error.lock().unwrap().clone();
                if let Some(error) = save_error {
                    if renamed {
                        move_without_replacing(&job.data.lock().unwrap().path(), &original.path())
                            .context("The file was renamed, but history could not be saved or the rename rolled back")?;
                    }
                    job.limiter.limit.store(original.speed_limit, Ordering::SeqCst);
                    *job.data.lock().unwrap() = original;
                    bail!(error);
                }
                Ok(())
            }.await;
            job.editing.store(false, Ordering::SeqCst);
            if restart && job.control.load(Ordering::SeqCst) == PAUSE { manager.resume(id); }
            result
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn idle(job: &Job) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while job.running.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn rename_preserves_resume_data_and_moves_completed_outputs_without_clobbering() {
        let dir = tempfile::tempdir().unwrap();
        let history = dir.path().join("history.json");
        let manager = Manager::new(history.clone()).unwrap();
        let _blocked = manager.slots.acquire_many(3).await.unwrap();
        let id = manager
            .add("https://example.com/original.zip", dir.path().into(), 4)
            .unwrap();
        let job = manager.find(id).unwrap();
        manager.pause(id);
        idle(&job).await;
        let parts = job.data.lock().unwrap().parts_dir();
        fs::create_dir(&parts).unwrap();
        fs::write(parts.join("0.part"), b"partial").unwrap();
        manager
            .change_download(id, DownloadChange::Rename("new name.zip".into()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(job.data.lock().unwrap().name, "new name.zip");
        assert!(job.data.lock().unwrap().preserve_name);
        assert_eq!(fs::read(parts.join("0.part")).unwrap(), b"partial");
        assert_eq!(job.data.lock().unwrap().status, Status::Paused);
        fs::write(dir.path().join("new name.zip"), b"finished").unwrap();
        {
            let mut data = job.data.lock().unwrap();
            data.status = Status::Complete;
            data.owns_output = true;
        }
        fs::write(dir.path().join("existing.zip"), b"keep me").unwrap();
        for invalid in [
            "../escape.zip",
            "CON",
            "bad/name.zip",
            "trailing. ",
            "existing.zip",
            "",
        ] {
            assert!(
                manager
                    .change_download(id, DownloadChange::Rename(invalid.into()))
                    .await
                    .unwrap()
                    .is_err()
            );
        }
        manager
            .change_download(id, DownloadChange::Rename("renamed.zip".into()))
            .await
            .unwrap()
            .unwrap();
        assert!(!dir.path().join("new name.zip").exists());
        assert_eq!(
            fs::read(dir.path().join("renamed.zip")).unwrap(),
            b"finished"
        );
        assert_eq!(
            fs::read(dir.path().join("existing.zip")).unwrap(),
            b"keep me"
        );
        manager.set_priority(&[id], Priority::High);
        manager
            .change_download(id, DownloadChange::SpeedLimit(512 * 1024))
            .await
            .unwrap()
            .unwrap();
        let restored = Manager::new(history).unwrap();
        let saved = &restored.snapshots()[0];
        assert_eq!(saved.name, "renamed.zip");
        assert_eq!(saved.speed_limit, 512 * 1024);
        assert_eq!(saved.priority, Priority::High);
        let mut legacy = serde_json::to_value(saved).unwrap();
        legacy.as_object_mut().unwrap().remove("speed_limit");
        legacy.as_object_mut().unwrap().remove("priority");
        let legacy: Download = serde_json::from_value(legacy).unwrap();
        assert_eq!(legacy.speed_limit, 0);
        assert_eq!(legacy.priority, Priority::Normal);
        assert_eq!(legacy.completed_at, None);
        manager
            .set_global_speed_limit(1024 * 1024)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(job.effective_speed_limit(), 512 * 1024);
        let restored = Manager::new(dir.path().join("history.json")).unwrap();
        assert_eq!(restored.settings.lock().unwrap().speed_limit, 1024 * 1024);
        assert!(manager.set_global_speed_limit(1024).await.unwrap().is_err());
    }

    #[tokio::test]
    async fn limiter_is_shared_across_workers_and_pause_interrupts_waits() {
        let dir = tempfile::tempdir().unwrap();
        let manager = Manager::new(dir.path().join("history.json")).unwrap();
        let _blocked = manager.slots.acquire_many(3).await.unwrap();
        let id = manager
            .add("https://example.com/file", dir.path().into(), 4)
            .unwrap();
        manager.pause(id);
        let job = manager.find(id).unwrap();
        idle(&job).await;
        job.control.store(RUN, Ordering::SeqCst);
        job.limiter.limit.store(100_000, Ordering::SeqCst);
        let started = std::time::Instant::now();
        let (a, b) = tokio::join!(
            job.limiter.consume(&job, 10_000),
            job.limiter.consume(&job, 10_000)
        );
        a.unwrap();
        b.unwrap();
        assert!(started.elapsed() >= Duration::from_millis(190));
        job.limiter.limit.store(1024, Ordering::SeqCst);
        let result = tokio::time::timeout(Duration::from_millis(500), async {
            let (result, _) = tokio::join!(job.limiter.consume(&job, 64 * 1024), async {
                tokio::time::sleep(Duration::from_millis(20)).await;
                job.control.store(PAUSE, Ordering::SeqCst);
            });
            result
        })
        .await
        .unwrap();
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn priority_orders_the_queue_and_rename_restarts_a_queued_job() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let (sender, receiver) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                if stream.read(&mut byte).unwrap() == 0 {
                    return;
                }
                request.push(byte[0]);
            }
            sender.send(String::from_utf8(request).unwrap()).unwrap();
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        });
        let dir = tempfile::tempdir().unwrap();
        let manager = Manager::new(dir.path().join("history.json")).unwrap();
        let mut blocked = manager.slots.acquire_many(3).await.unwrap();
        let low = manager
            .add(&format!("{address}/low"), dir.path().into(), 1)
            .unwrap();
        let normal = manager
            .add(&format!("{address}/normal"), dir.path().into(), 1)
            .unwrap();
        let high = manager
            .add(&format!("{address}/high"), dir.path().into(), 1)
            .unwrap();
        manager.set_priority(&[low], Priority::Low);
        manager.set_priority(&[high], Priority::High);
        manager
            .change_download(high, DownloadChange::Rename("priority.bin".into()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            manager.find(high).unwrap().data.lock().unwrap().status,
            Status::Queued
        );
        let _other_slots = blocked.split(2).unwrap();
        drop(blocked);
        let request = tokio::task::spawn_blocking(move || {
            receiver.recv_timeout(Duration::from_secs(5)).unwrap()
        })
        .await
        .unwrap();
        assert!(request.starts_with("GET /high "));
        manager.pause(low);
        manager.pause(normal);
        manager.pause(high);
        server.join().unwrap();
        manager.shutdown();
    }
}
