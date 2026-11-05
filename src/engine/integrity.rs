use super::*;
use sha2::{Digest, Sha256};
use std::io::Read;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IntegrityFile {
    pub path: PathBuf,
    pub size: u64,
    pub sha256: String,
}

fn output_path(root: &Path, relative: &Path) -> Result<PathBuf> {
    ensure!(
        relative
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_))),
        "Invalid integrity file path"
    );
    Ok(if relative.as_os_str().is_empty() {
        root.to_owned()
    } else {
        root.join(relative)
    })
}

pub(super) fn fingerprint(path: &Path) -> Result<IntegrityFile> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "Integrity verification requires a regular file"
    );
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 1024 * 1024];
    let mut size = 0;
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
        size += count as u64;
    }
    ensure!(
        size == metadata.len() && file.metadata()?.modified()? == metadata.modified()?,
        "The file changed during integrity verification"
    );
    Ok(IntegrityFile {
        path: PathBuf::new(),
        size,
        sha256: format!("{:x}", hash.finalize()),
    })
}

pub(super) fn capture(root: &Path) -> Result<Vec<IntegrityFile>> {
    fn visit(root: &Path, path: &Path, files: &mut Vec<IntegrityFile>) -> Result<()> {
        let metadata = fs::symlink_metadata(path)?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "Download output contains a link"
        );
        if metadata.is_dir() {
            for entry in fs::read_dir(path)? {
                visit(root, &entry?.path(), files)?;
            }
        } else {
            let mut file = fingerprint(path)?;
            file.path = path.strip_prefix(root)?.to_owned();
            files.push(file);
        }
        Ok(())
    }
    let mut files = Vec::new();
    visit(root, root, &mut files)?;
    ensure!(
        !files.is_empty(),
        "The download contains no files to verify"
    );
    files.sort_unstable_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

pub(super) fn verify(root: &Path, files: &[IntegrityFile]) -> Result<()> {
    ensure!(
        !files.is_empty(),
        "This download has no saved SHA-256 fingerprints; its original integrity cannot be verified"
    );
    ensure!(
        !fs::symlink_metadata(root)?.file_type().is_symlink(),
        "The download output is now a link"
    );
    let canonical_root = fs::canonicalize(root)?;
    for expected in files {
        let path = output_path(root, &expected.path)?;
        ensure!(
            fs::canonicalize(&path)?.starts_with(&canonical_root),
            "Integrity file is outside the download output"
        );
        let actual =
            fingerprint(&path).with_context(|| format!("Cannot verify {}", path.display()))?;
        ensure!(
            actual.size == expected.size && actual.sha256 == expected.sha256,
            "Integrity verification failed for {}: size or SHA-256 checksum mismatch",
            path.display()
        );
    }
    Ok(())
}

fn missing(data: &Download) -> bool {
    let path = data.path();
    let missing_path = |path: &Path, folder: bool| match fs::metadata(path) {
        Ok(metadata) => {
            if folder {
                !metadata.is_dir()
            } else {
                !metadata.is_file()
            }
        }
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
    };
    missing_path(&path, data.protocol.folder_output())
        || data
            .integrity
            .iter()
            .any(|file| output_path(&path, &file.path).is_ok_and(|path| missing_path(&path, false)))
}

pub(super) fn refresh_missing(data: &mut Download) {
    if matches!(data.status, Status::Complete | Status::Missing) {
        if missing(data) {
            data.status = Status::Missing;
            data.error =
                Some("A downloaded file can no longer be found at its saved location.".into());
        } else if data.status == Status::Missing {
            data.status = Status::Complete;
            data.error = None;
        }
    }
}

impl Manager {
    pub(super) fn refresh_missing(&self) {
        let jobs = self.jobs.lock().unwrap().clone();
        for job in jobs {
            let mut snapshot = job.data.lock().unwrap().clone();
            if !matches!(snapshot.status, Status::Complete | Status::Missing) {
                continue;
            }
            let original_status = snapshot.status;
            let original_path = snapshot.path();
            refresh_missing(&mut snapshot);
            let mut data = job.data.lock().unwrap();
            if !job.running.load(Ordering::SeqCst)
                && !job.editing.load(Ordering::SeqCst)
                && !job.deleting.load(Ordering::SeqCst)
                && data.status == original_status
                && data.path() == original_path
            {
                data.status = snapshot.status;
                data.error = snapshot.error;
            }
        }
    }

    pub fn verify_integrity(self: &Arc<Self>, id: u64) {
        let Some(job) = self.find(id) else {
            return;
        };
        let snapshot = {
            let mut data = job.data.lock().unwrap();
            if !data.owns_output
                || job.running.load(Ordering::SeqCst)
                || job.editing.load(Ordering::SeqCst)
                || job.deleting.load(Ordering::SeqCst)
            {
                return;
            }
            job.running.store(true, Ordering::SeqCst);
            let snapshot = data.clone();
            data.status = Status::Verifying;
            data.error = None;
            snapshot
        };
        let manager = self.clone();
        self.runtime.spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                let is_missing = missing(&snapshot);
                (is_missing, verify(&snapshot.path(), &snapshot.integrity))
            })
            .await;
            {
                let mut data = job.data.lock().unwrap();
                if !job.deleting.load(Ordering::SeqCst) {
                    match result {
                        Ok((false, Ok(()))) => {
                            data.status = Status::Complete;
                            data.error = None;
                        }
                        Ok((true, _)) => {
                            data.status = Status::Missing;
                            data.error = Some(
                                "A downloaded file can no longer be found at its saved location."
                                    .into(),
                            );
                        }
                        Ok((false, Err(error))) => {
                            data.status = Status::Failed;
                            data.error = Some(format!("{error:#}"));
                        }
                        Err(error) => {
                            data.status = Status::Failed;
                            data.error = Some(format!("Integrity worker stopped: {error}"));
                        }
                    }
                }
            }
            job.running.store(false, Ordering::SeqCst);
            manager.persist();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_match_sha256_and_detect_same_size_corruption() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("file.bin");
        fs::write(&path, b"abc").unwrap();
        let files = capture(&path).unwrap();
        assert_eq!(
            files[0].sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        verify(&path, &files).unwrap();
        fs::write(&path, b"abd").unwrap();
        assert!(
            verify(&path, &files)
                .unwrap_err()
                .to_string()
                .contains("checksum mismatch")
        );
        fs::write(&path, b"ab").unwrap();
        assert!(verify(&path, &files).is_err());
    }

    #[test]
    fn folder_checks_cover_nested_and_empty_files_and_reject_unsafe_paths() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("nested")).unwrap();
        fs::write(directory.path().join("nested/file.bin"), b"payload").unwrap();
        fs::write(directory.path().join("empty.bin"), []).unwrap();
        let mut files = capture(directory.path()).unwrap();
        assert_eq!(files.len(), 2);
        verify(directory.path(), &files).unwrap();
        fs::remove_file(directory.path().join("nested/file.bin")).unwrap();
        assert!(verify(directory.path(), &files).is_err());
        files[0].path = PathBuf::from("../outside.bin");
        assert!(
            verify(directory.path(), &files)
                .unwrap_err()
                .to_string()
                .contains("Invalid integrity file path")
        );
        assert!(verify(directory.path(), &[]).is_err());
    }
}
