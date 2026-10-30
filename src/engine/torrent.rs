use super::*;
use librqbit::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, DhtSessionConfig, Session, SessionOptions,
    api::TorrentIdOrHash,
};

impl Manager {
    pub(super) async fn execute_torrent(&self, job: &Arc<Job>) -> Result<()> {
        let d = job.data.lock().unwrap().clone();
        let content = d.parts_dir().join("content");
        fs::create_dir_all(&content)?;
        let cache = d.parts_dir().join("metadata.torrent");
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
                        client_name_and_version: Some("Strata/0.1".into()),
                        ..Default::default()
                    },
                )
                .await
            })
            .await?;
        let magnet = d.url.starts_with("magnet:");
        let add = if magnet && !cache.is_file() {
            AddTorrent::from_url(&d.url)
        } else if magnet {
            AddTorrent::from_bytes(tokio::fs::read(&cache).await?)
        } else {
            AddTorrent::from_bytes(self.descriptor(job).await?)
        };
        let response = tokio::select! {
            result = session.add_torrent(add, Some(AddTorrentOptions { paused: true, overwrite: true,
                output_folder: Some(content.to_string_lossy().into_owned()), peer_limit: Some(d.connections), ..Default::default() })) => result?,
            _ = stopped(job) => bail!("Transfer stopped"),
        };
        let (id, handle) = match response {
            AddTorrentResponse::Added(id, handle) => (id, handle),
            AddTorrentResponse::AlreadyManaged(..) => {
                bail!("This torrent is already active in another transfer")
            }
            AddTorrentResponse::ListOnly(_) => bail!("The torrent was not started"),
        };
        let result = async {
            if magnet {
                let expected = librqbit::Magnet::parse(&d.url)?.as_id20().context("A BitTorrent v1 / hybrid info hash is required")?;
                ensure!(handle.info_hash() == expected, "Cached torrent metadata does not match the magnet link");
            }
            if let Some(metadata) = handle.metadata.load_full() { fs::write(&cache, &metadata.torrent_bytes)?; }
            // Unpausing before the initial piece check finishes leaves the torrent paused.
            tokio::select! { result = handle.wait_until_initialized() => result?, _ = stopped(job) => bail!("Transfer stopped") }
            session.unpause(&handle).await?;
            loop {
                check_running(job)?;
                let stats = handle.stats();
                if let Some(error) = stats.error { bail!("Torrent failed: {error}"); }
                { let mut data = job.data.lock().unwrap(); data.status = Status::Downloading; data.total = Some(stats.total_bytes);
                    data.downloaded = stats.progress_bytes; data.ranged = true;
                    data.parts = vec![Part { start: 0, end: stats.total_bytes.saturating_sub(1), downloaded: stats.progress_bytes }]; }
                if stats.finished { break; }
                tokio::select! { _ = tokio::time::sleep(Duration::from_millis(250)) => {}, _ = stopped(job) => bail!("Transfer stopped") }
            }
            Ok::<_, anyhow::Error>(())
        }.await;
        // Remove the live handle without deleting payloads; completed jobs must not keep seeding.
        session.delete(TorrentIdOrHash::Id(id), false).await?;
        drop(handle);
        result?;
        tools::publish_directory(job, &content)
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
        let payload = (0..2 * 1024 * 1024)
            .map(|n| (n % 251) as u8)
            .collect::<Vec<_>>();
        fs::write(source.join("payload.bin"), &payload).unwrap();
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
                &source.join("payload.bin"),
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
        let id = manager
            .add(
                &metadata.as_magnet().to_string(),
                directory.path().into(),
                4,
            )
            .unwrap();
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
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "Magnet timed out: {d:?}"
            );
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        manager.shutdown();
        client.stop().await;
        seed.stop().await;
        tracker.abort();
    }
}
