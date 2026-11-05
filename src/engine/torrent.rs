use super::*;
use librqbit::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, DhtSessionConfig, Session, SessionOptions,
    api::TorrentIdOrHash,
};

fn save_metadata(path: &Path, bytes: &[u8]) -> Result<()> {
    if path.try_exists()? {
        return Ok(());
    }
    fs::create_dir_all(path.parent().context("Missing torrent metadata folder")?)?;
    let mut temporary = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    std::io::Write::write_all(&mut temporary, bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist_noclobber(path)?;
    Ok(())
}

impl Manager {
    pub(super) async fn execute_torrent(&self, job: &Arc<Job>) -> Result<()> {
        let d = job.data.lock().unwrap().clone();
        let content = d.parts_dir().join("content");
        fs::create_dir_all(d.parts_dir())?;
        let cache = self.torrent_metadata_path(d.id);
        let legacy_cache = d.parts_dir().join("metadata.torrent");
        let session = self
            .torrent_session
            .get_or_try_init(|| async {
                Session::new_with_opts(
                    self.history.parent().unwrap().join("torrent-session"),
                    SessionOptions {
                        dht: Some(DhtSessionConfig {
                            persistence: None,
                            ..Default::default()
                        }),
                        runtime_worker_threads: Some(4),
                        client_name_and_version: Some(
                            concat!("101Downloader/", env!("CARGO_PKG_VERSION")).into(),
                        ),
                        ..Default::default()
                    },
                )
                .await
            })
            .await?;
        let magnet = d.url.starts_with("magnet:");
        let add = if cache.is_file() {
            AddTorrent::from_bytes(tokio::fs::read(&cache).await?)
        } else if legacy_cache.is_file() {
            AddTorrent::from_bytes(tokio::fs::read(&legacy_cache).await?)
        } else if magnet {
            AddTorrent::from_url(&d.url)
        } else {
            AddTorrent::from_bytes(self.descriptor(job).await?)
        };
        let response = tokio::select! {
            result = session.add_torrent(add, Some(AddTorrentOptions { paused: true, overwrite: true,
                list_only: d.selected_files.is_none(), only_files: d.selected_files.clone(),
                output_folder: Some(content.to_string_lossy().into_owned()), peer_limit: Some(d.connections),
                ratelimits: librqbit::limits::LimitsConfig { download_bps: std::num::NonZeroU32::new(job.effective_speed_limit().min(u32::MAX as u64) as u32), ..Default::default() }, ..Default::default() })) => result?,
            _ = stopped(job) => bail!("Transfer stopped"),
        };
        let (id, handle) = match response {
            AddTorrentResponse::Added(id, handle) => (id, handle),
            AddTorrentResponse::AlreadyManaged(..) => {
                bail!("This torrent is already active in another transfer")
            }
            AddTorrentResponse::ListOnly(metadata) => {
                check_running(job)?;
                if magnet {
                    let expected = librqbit::Magnet::parse(&d.url)?
                        .as_id20()
                        .context("A BitTorrent v1 / hybrid info hash is required")?;
                    ensure!(
                        metadata.info_hash == expected,
                        "Cached torrent metadata does not match the magnet link"
                    );
                }
                let files = metadata
                    .info
                    .iter_file_details()
                    .map(|file| TorrentFile {
                        name: file.filename.to_string(),
                        size: file.len,
                    })
                    .collect::<Vec<_>>();
                ensure!(!files.is_empty(), "This torrent contains no files");
                save_metadata(&cache, &metadata.torrent_bytes)?;
                let mut data = job.data.lock().unwrap();
                data.torrent_files = files;
                data.status = Status::SelectingFiles;
                self.show_requested.store(true, Ordering::SeqCst);
                return Ok(());
            }
        };
        let selected = d.selected_files.as_deref().unwrap_or_default();
        let metadata = handle.metadata.load_full();
        let result = async {
            let metadata = metadata.as_ref().context("Torrent metadata is missing")?;
            let files = metadata.info.iter_file_details().map(|file| TorrentFile { name: file.filename.to_string(), size: file.len }).collect::<Vec<_>>();
            ensure!(files == d.torrent_files, "Torrent contents changed. Cancel this download and add it again to choose files.");
            ensure!(!selected.is_empty() && selected.iter().all(|&index| index < metadata.file_infos.len()), "Choose valid torrent files before starting");
            let total = selected.iter().map(|&index| metadata.file_infos[index].len).sum::<u64>();
            if magnet {
                let expected = librqbit::Magnet::parse(&d.url)?.as_id20().context("A BitTorrent v1 / hybrid info hash is required")?;
                ensure!(handle.info_hash() == expected, "Cached torrent metadata does not match the magnet link");
            }
            if let Some(metadata) = handle.metadata.load_full() { save_metadata(&cache, &metadata.torrent_bytes)?; }
            // Unpausing before the initial piece check finishes leaves the torrent paused.
            tokio::select! { result = handle.wait_until_initialized() => result?, _ = stopped(job) => bail!("Transfer stopped") }
            session.unpause(&handle).await?;
            loop {
                check_running(job)?;
                let stats = handle.stats();
                if let Some(error) = stats.error { bail!("Torrent failed: {error}"); }
                let downloaded = selected.iter().map(|&index| stats.file_progress.get(index).copied().unwrap_or(0).min(metadata.file_infos[index].len)).sum();
                { let mut data = job.data.lock().unwrap(); data.status = Status::Downloading; data.total = Some(total);
                    data.downloaded = downloaded; data.ranged = true;
                    data.parts = if total == 0 { vec![] } else { vec![Part { start: 0, end: total - 1, downloaded }] }; }
                if stats.finished { break; }
                tokio::select! { _ = tokio::time::sleep(Duration::from_millis(250)) => {}, _ = stopped(job) => bail!("Transfer stopped") }
            }
            Ok::<_, anyhow::Error>(())
        }.await;
        // Remove the live handle without deleting payloads; completed jobs must not keep seeding.
        session.delete(TorrentIdOrHash::Id(id), false).await?;
        drop(handle);
        result?;
        // librqbit creates every file for shared boundary pieces; publish only the user's selection.
        for (index, file) in metadata
            .context("Torrent metadata is missing")?
            .file_infos
            .iter()
            .enumerate()
        {
            if !selected.contains(&index) {
                ensure!(
                    file.relative_filename
                        .components()
                        .all(|part| matches!(part, std::path::Component::Normal(_))),
                    "Invalid torrent output path"
                );
                let path = content.join(&file.relative_filename);
                match fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(error).context("Cannot remove an unselected torrent file");
                    }
                }
                let mut parent = path.parent();
                while let Some(directory) = parent.filter(|directory| *directory != content) {
                    match fs::remove_dir(directory) {
                        Ok(()) => parent = directory.parent(),
                        Err(error)
                            if matches!(
                                error.kind(),
                                std::io::ErrorKind::DirectoryNotEmpty
                                    | std::io::ErrorKind::NotFound
                            ) =>
                        {
                            break;
                        }
                        Err(error) => {
                            return Err(error).context("Cannot remove an empty torrent folder");
                        }
                    }
                }
            }
        }
        tools::publish_directory(job, &content).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn native_torrent_and_magnet_download_from_a_local_seed_and_recheck_partial_data() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("seed");
        fs::create_dir(&source).unwrap();
        let payload = (0..2 * 1024 * 1024 + 97)
            .map(|n| (n % 251) as u8)
            .collect::<Vec<_>>();
        fs::write(source.join("payload.bin"), &payload).unwrap();
        fs::create_dir(source.join("excluded")).unwrap();
        fs::write(
            source.join("excluded/unselected.bin"),
            vec![7; 512 * 1024 + 123],
        )
        .unwrap();
        let seed = Session::new_with_opts(
            source.clone(),
            SessionOptions {
                dht: None,
                disable_trackers: true,
                disable_local_service_discovery: true,
                listen: Some(librqbit::ListenerOptions {
                    listen_addr: "127.0.0.1:0".parse().unwrap(),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let seed_port = seed.listen_addr().unwrap().port();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let tracker_url = format!("http://{}/announce", listener.local_addr().unwrap());
        let announces = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let tracker_announces = announces.clone();
        let tracker = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                tracker_announces.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    loop {
                        let mut byte = [0];
                        if stream.read_exact(&mut byte).await.is_err() {
                            return;
                        }
                        request.push(byte[0]);
                        if request.ends_with(b"\r\n\r\n") {
                            break;
                        }
                        if request.len() > 16384 {
                            return;
                        }
                    }
                    let mut body = b"d8:intervali1e5:peers6:".to_vec();
                    body.extend_from_slice(&[
                        127,
                        0,
                        0,
                        1,
                        (seed_port >> 8) as u8,
                        seed_port as u8,
                    ]);
                    body.push(b'e');
                    let headers = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(headers.as_bytes()).await;
                    let _ = stream.write_all(&body).await;
                });
            }
        });
        let (metadata, handle) = seed
            .create_and_serve_torrent(
                &source,
                librqbit::CreateTorrentOptions {
                    trackers: vec![tracker_url],
                    piece_length: Some(256 * 1024),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        handle.wait_until_completed().await.unwrap();
        let descriptor = directory.path().join("test.torrent");
        fs::write(&descriptor, metadata.as_bytes().unwrap()).unwrap();
        let manager = Manager::new(directory.path().join("history.json")).unwrap();
        manager
            .set_global_speed_limit(1024 * 1024)
            .await
            .unwrap()
            .unwrap();
        let client = Session::new_with_opts(
            directory.path().join("session"),
            SessionOptions {
                dht: None,
                disable_local_service_discovery: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(manager.torrent_session.set(client.clone()).is_ok());
        let permits = manager.slots.acquire_many(3).await.unwrap();
        let id = manager
            .add(&descriptor.to_string_lossy(), directory.path().into(), 4)
            .unwrap();
        let queued = manager
            .snapshots()
            .into_iter()
            .find(|d| d.id == id)
            .unwrap();
        let partial = queued.parts_dir().join("content");
        fs::create_dir_all(&partial).unwrap();
        fs::write(partial.join("payload.bin"), &payload[..256 * 1024]).unwrap();
        drop(permits);
        let choose_files = async |id| {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
            loop {
                let d = manager
                    .snapshots()
                    .into_iter()
                    .find(|d| d.id == id)
                    .unwrap();
                assert_ne!(d.status, Status::Failed, "{:?}", d.error);
                if d.status == Status::SelectingFiles
                    && !manager.find(id).unwrap().running.load(Ordering::SeqCst)
                {
                    assert_eq!(d.downloaded, 0);
                    assert_eq!(d.torrent_files.len(), 2);
                    assert!(!d.parts_dir().join("content/excluded").exists());
                    if partial == d.parts_dir().join("content") {
                        assert_eq!(
                            fs::metadata(partial.join("payload.bin")).unwrap().len(),
                            256 * 1024
                        );
                    } else {
                        assert!(!d.parts_dir().join("content").exists());
                    }
                    assert!(
                        client
                            .get(TorrentIdOrHash::Hash(metadata.info_hash()))
                            .is_none()
                    );
                    assert!(manager.select_torrent_files(id, vec![]).is_err());
                    assert!(
                        manager
                            .select_torrent_files(id, vec![d.torrent_files.len()])
                            .is_err()
                    );
                    let index = d
                        .torrent_files
                        .iter()
                        .position(|f| f.name == "payload.bin")
                        .unwrap();
                    manager.pause(id);
                    assert_eq!(
                        manager.find(id).unwrap().data.lock().unwrap().status,
                        Status::Paused
                    );
                    manager.resume(id);
                    assert_eq!(
                        manager.find(id).unwrap().data.lock().unwrap().status,
                        Status::SelectingFiles
                    );
                    let permits = manager
                        .slots
                        .try_acquire_many(3)
                        .expect("File selection must release queue slots");
                    manager
                        .select_torrent_files(id, vec![index, index])
                        .unwrap();
                    let saved: History =
                        serde_json::from_slice(&fs::read(&manager.history).unwrap()).unwrap();
                    let saved = saved.downloads.iter().find(|d| d.id == id).unwrap();
                    assert_eq!(saved.selected_files, Some(vec![index]));
                    assert_eq!(saved.status, Status::Paused);
                    assert_eq!(saved.total, Some(payload.len() as u64));
                    drop(permits);
                    break;
                }
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "Metadata timed out: {d:?}"
                );
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        };
        choose_files(id).await;
        for id in [id] {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
            loop {
                let d = manager
                    .snapshots()
                    .into_iter()
                    .find(|d| d.id == id)
                    .unwrap();
                if matches!(d.status, Status::Complete | Status::Failed) {
                    assert_eq!(d.status, Status::Complete, "{:?}", d.error);
                    assert_eq!(fs::read(d.path().join("payload.bin")).unwrap(), payload);
                    assert!(!d.path().join("excluded").exists());
                    assert_eq!(d.total, Some(payload.len() as u64));
                    assert_eq!(d.downloaded, payload.len() as u64);
                    break;
                }
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "Torrent timed out: {d:?}; announces={}; client={:?}; seed={:?}",
                    announces.load(Ordering::SeqCst),
                    client
                        .get(TorrentIdOrHash::Hash(metadata.info_hash()))
                        .map(|h| h.stats()),
                    handle.stats()
                );
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        }
        while manager.find(id).unwrap().running.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let completed = manager
            .snapshots()
            .into_iter()
            .find(|download| download.id == id)
            .unwrap();
        assert_eq!(completed.integrity.len(), 1);
        assert!(manager.torrent_metadata_path(id).is_file());
        fs::remove_file(&descriptor).unwrap();
        let additional = completed
            .torrent_files
            .iter()
            .position(|file| file.name.ends_with("unselected.bin"))
            .unwrap();
        assert!(manager.select_torrent_files(id, vec![additional]).is_err());
        let all_files = (0..completed.torrent_files.len()).collect();
        assert!(
            manager
                .select_torrent_files(id, vec![completed.torrent_files.len()])
                .is_err()
        );
        manager.select_torrent_files(id, all_files).unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            let download = manager
                .snapshots()
                .into_iter()
                .find(|download| download.id == id)
                .unwrap();
            if matches!(download.status, Status::Complete | Status::Failed) {
                assert_eq!(download.status, Status::Complete, "{:?}", download.error);
                assert_eq!(
                    fs::read(download.path().join("payload.bin")).unwrap(),
                    payload
                );
                assert_eq!(
                    fs::read(download.path().join("excluded/unselected.bin")).unwrap(),
                    vec![7; 512 * 1024 + 123]
                );
                assert_eq!(download.integrity.len(), 2);
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "Additional torrent selections timed out: {download:?}"
            );
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        let id = manager
            .add(
                &metadata.as_magnet().to_string(),
                directory.path().into(),
                4,
            )
            .unwrap();
        choose_files(id).await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            let d = manager
                .snapshots()
                .into_iter()
                .find(|d| d.id == id)
                .unwrap();
            if matches!(d.status, Status::Complete | Status::Failed) {
                assert_eq!(d.status, Status::Complete, "{:?}", d.error);
                assert_eq!(fs::read(d.path().join("payload.bin")).unwrap(), payload);
                assert!(!d.path().join("excluded").exists());
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "Magnet timed out: {d:?}"
            );
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        let cache = manager.torrent_metadata_path(id);
        manager.delete(id);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while manager.find(id).is_some() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "Torrent deletion timed out"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(!cache.exists());
        manager.shutdown();
        client.stop().await;
        seed.stop().await;
        tracker.abort();
    }
}
