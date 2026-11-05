use super::*;

fn remove_owned_path(path: &Path, allow_directory: bool) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    ensure!(
        !metadata.file_type().is_symlink(),
        "The download path is now a link. Remove that link manually instead."
    );
    if metadata.is_dir() {
        ensure!(
            allow_directory,
            "The downloaded file was replaced by a directory. Remove it manually instead."
        );
        // The standard library does not follow links inside this owned output directory.
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn delete_data(download: &Download) -> Result<()> {
    let mut components = Path::new(&download.name).components();
    ensure!(
        matches!(components.next(), Some(std::path::Component::Normal(_)))
            && components.next().is_none()
            && sanitize_filename(&download.name) == download.name,
        "The recorded filename is invalid; no files were deleted."
    );
    let directory = match fs::canonicalize(&download.directory) {
        Ok(directory) => directory,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("Cannot access the download folder"),
    };
    if download.owns_output {
        remove_owned_path(
            &directory.join(&download.name),
            download.protocol.folder_output(),
        )?;
    }
    remove_owned_path(
        &directory.join(format!(".strata-{}.parts", download.id)),
        true,
    )?;
    Ok(())
}

impl Manager {
    pub fn delete(self: &Arc<Self>, id: u64) {
        let Some(job) = self.find(id) else {
            return;
        };
        {
            let mut data = job.data.lock().unwrap();
            if job.editing.load(Ordering::SeqCst) {
                return;
            }
            if job.deleting.swap(true, Ordering::SeqCst) {
                return;
            }
            job.control.store(CANCEL, Ordering::SeqCst);
            data.status = Status::Deleting;
        }
        self.persist();
        let manager = self.clone();
        self.runtime.spawn(async move {
            while job.running.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            let download = job.data.lock().unwrap().clone();
            let cache = manager.torrent_metadata_path(id);
            let result = tokio::task::spawn_blocking(move || {
                delete_data(&download)?;
                if download.protocol == Protocol::Torrent {
                    match fs::remove_file(cache) {
                        Ok(()) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => {
                            return Err(error).context("Cannot remove saved torrent metadata");
                        }
                    }
                }
                Ok(())
            })
            .await;
            match result
                .context("File deletion worker stopped")
                .and_then(|result| result)
            {
                Ok(()) => manager
                    .jobs
                    .lock()
                    .unwrap()
                    .retain(|other| !Arc::ptr_eq(other, &job)),
                Err(error) => {
                    let mut data = job.data.lock().unwrap();
                    job.deleting.store(false, Ordering::SeqCst);
                    data.status = Status::Failed;
                    data.error = Some(format!(
                        "Could not delete download: {error:#}. Choose Delete from device to retry."
                    ));
                }
            }
            job.deleting.store(false, Ordering::SeqCst);
            manager.persist();
        });
    }
}
