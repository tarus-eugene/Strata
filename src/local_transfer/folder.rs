use super::{
    ACCEPT, IO_TIMEOUT, State, TransferProgress, read_metadata, receive_verified_file,
    send_verified_file, write_metadata,
};
use crate::engine::{move_without_replacing, validate_name};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, Metadata},
    path::{Path, PathBuf},
};
use tokio::{
    fs::{File, OpenOptions},
    io::{AsyncRead, AsyncWrite, AsyncWriteExt},
    sync::watch,
    time::timeout,
};

const MAX_ENTRIES: u32 = 1_000_000;
const MAX_ENTRY_BYTES: usize = 16 * 1024;
const MAX_PATH_TABLE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct Entry {
    relative: String,
    file_size: Option<u64>,
}

pub(super) struct Folder {
    root: PathBuf,
    entries: Vec<Entry>,
    size: u64,
}

impl Folder {
    pub(super) async fn scan(path: PathBuf) -> Result<Self> {
        tokio::task::spawn_blocking(move || Self::scan_blocking(&path)).await?
    }

    fn scan_blocking(path: &Path) -> Result<Self> {
        let metadata = fs::symlink_metadata(path)?;
        ensure!(
            metadata.is_dir() && !is_link(&metadata),
            "Choose a regular folder to share"
        );
        let root = fs::canonicalize(path)?;
        let mut folder = Self {
            root,
            entries: Vec::new(),
            size: 0,
        };
        let mut pending = vec![String::new()];
        let mut path_bytes = 0;
        while let Some(parent) = pending.pop() {
            let directory = if parent.is_empty() {
                folder.root.clone()
            } else {
                destination_path(&folder.root, &parent)?
            };
            ensure!(
                !is_link(&fs::symlink_metadata(&directory)?),
                "The selected folder contains a link or junction"
            );
            for child in fs::read_dir(directory)? {
                let child = child?;
                let name = child.file_name();
                let name = name
                    .to_str()
                    .context("The folder contains a name that cannot be transferred")?;
                validate_component(name)?;
                let relative = if parent.is_empty() {
                    name.to_owned()
                } else {
                    format!("{parent}/{name}")
                };
                path_bytes += relative.len();
                ensure!(
                    relative.len() + 64 <= MAX_ENTRY_BYTES
                        && path_bytes <= MAX_PATH_TABLE_BYTES
                        && folder.entries.len() < MAX_ENTRIES as usize,
                    "The selected folder has too many or overly long paths"
                );
                let metadata = fs::symlink_metadata(child.path())?;
                ensure!(
                    !is_link(&metadata),
                    "The selected folder contains a link or junction"
                );
                let file_size = if metadata.is_dir() {
                    pending.push(relative.clone());
                    None
                } else {
                    ensure!(
                        metadata.is_file(),
                        "The selected folder contains an unsupported item"
                    );
                    folder.size = folder
                        .size
                        .checked_add(metadata.len())
                        .context("The selected folder is too large")?;
                    Some(metadata.len())
                };
                folder.entries.push(Entry {
                    relative,
                    file_size,
                });
            }
        }
        Ok(folder)
    }

    pub(super) fn size(&self) -> u64 {
        self.size
    }

    pub(super) fn entry_count(&self) -> u32 {
        self.entries.len() as u32
    }

    pub(super) async fn send(
        self,
        stream: &mut (impl AsyncWrite + Unpin),
        progress: &mut TransferProgress<'_>,
    ) -> Result<()> {
        for entry in self.entries {
            write_metadata(stream, &entry, MAX_ENTRY_BYTES).await?;
            if let Some(size) = entry.file_size {
                let path = destination_path(&self.root, &entry.relative)?;
                let file = open_source_file(&path, &self.root, size).await?;
                send_verified_file(stream, file, size, progress).await?;
            }
        }
        Ok(())
    }
}

pub(super) fn validate_offer(count: u32, size: u64) -> Result<()> {
    ensure!(
        count <= MAX_ENTRIES && (count != 0 || size == 0),
        "Invalid folder size or item count"
    );
    Ok(())
}

pub(super) fn validate_component(name: &str) -> Result<()> {
    validate_name(name)?;
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_ascii_uppercase();
    let device_number = stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"));
    ensure!(
        !matches!(
            stem.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        ) && !matches!(
            device_number,
            Some("0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³")
        ),
        "The transfer contains a reserved Windows name"
    );
    Ok(())
}

fn destination_path(root: &Path, relative: &str) -> Result<PathBuf> {
    let mut path = root.to_owned();
    for component in relative.split('/') {
        validate_component(component).context("Invalid folder item path")?;
        path.push(component);
    }
    Ok(path)
}

fn is_link(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
    }
    #[cfg(not(windows))]
    metadata.file_type().is_symlink()
}

async fn open_source_file(path: &Path, root: &Path, size: u64) -> Result<File> {
    let metadata = tokio::fs::symlink_metadata(path).await?;
    ensure!(
        metadata.is_file() && !is_link(&metadata),
        "A file in the selected folder changed"
    );
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    options.custom_flags(windows::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT.0);
    let file = options.open(path).await?;
    let metadata = file.metadata().await?;
    ensure!(
        metadata.is_file() && !is_link(&metadata) && metadata.len() == size,
        "A file in the selected folder changed"
    );
    #[cfg(windows)]
    let resolved = opened_path(&file)?;
    #[cfg(not(windows))]
    let resolved = tokio::fs::canonicalize(path).await?;
    ensure!(
        resolved.starts_with(root),
        "A file now points outside the selected folder"
    );
    Ok(file)
}

#[cfg(windows)]
fn opened_path(file: &File) -> Result<PathBuf> {
    use std::{
        ffi::OsString,
        os::windows::{ffi::OsStringExt, io::AsRawHandle},
    };
    use windows::Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{FILE_NAME_NORMALIZED, GetFinalPathNameByHandleW},
    };
    let mut buffer = vec![0; 32_768];
    let length = unsafe {
        GetFinalPathNameByHandleW(
            HANDLE(file.as_raw_handle()),
            &mut buffer,
            FILE_NAME_NORMALIZED,
        )
    } as usize;
    ensure!(
        length > 0 && length < buffer.len(),
        "Could not resolve the opened file path"
    );
    Ok(PathBuf::from(OsString::from_wide(&buffer[..length])))
}

pub(super) async fn receive(
    stream: &mut (impl AsyncRead + AsyncWrite + Unpin),
    destination: &Path,
    entry_count: u32,
    size: u64,
    status: &watch::Sender<State>,
) -> Result<State> {
    let parent = destination.parent().context("Missing destination folder")?;
    let stage = tempfile::Builder::new()
        .prefix(".strata-transfer-")
        .tempdir_in(parent)?;
    stream.write_u8(ACCEPT).await?;
    stream.flush().await?;
    let mut progress = TransferProgress::new(size, status);
    for _ in 0..entry_count {
        let entry: Entry = timeout(IO_TIMEOUT, read_metadata(stream, MAX_ENTRY_BYTES)).await??;
        let path = destination_path(stage.path(), &entry.relative)?;
        if let Some(file_size) = entry.file_size {
            ensure!(
                file_size <= size - progress.bytes,
                "The folder exceeded its advertised size"
            );
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .await?;
            receive_verified_file(stream, &mut file, file_size, &mut progress).await?;
        } else {
            tokio::fs::create_dir(path).await?;
        }
    }
    ensure!(
        progress.bytes == size,
        "The folder did not match its advertised size"
    );
    move_without_replacing(stage.path(), destination)
        .context("Could not save the folder at the chosen destination")?;
    let _ = stage.keep();
    Ok(State::Finished(Some(destination.to_owned())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, duplex};

    #[test]
    fn rejects_unsafe_components() {
        let root = Path::new("stage");
        for path in [
            "../escape",
            "/absolute",
            "parent//child",
            "parent/..",
            "parent\\child",
            "C:/drive",
            "file:stream",
            "CONIN$",
            "AUX .txt",
            "sub/COM¹.txt",
        ] {
            assert!(destination_path(root, path).is_err(), "accepted {path}");
        }
        assert_eq!(
            destination_path(root, "sub/文档.txt").unwrap(),
            root.join("sub").join("文档.txt")
        );
    }

    #[tokio::test]
    async fn bad_folder_data_is_never_published() {
        let folder = tempfile::tempdir().unwrap();
        let destination = folder.path().join("shared");
        let (status, _) = watch::channel(State::Connecting);
        let (mut sender, mut receiver) = duplex(4096);
        let (result, ()) =
            tokio::join!(receive(&mut receiver, &destination, 1, 3, &status), async {
                assert_eq!(sender.read_u8().await.unwrap(), ACCEPT);
                write_metadata(
                    &mut sender,
                    &Entry {
                        relative: "file.bin".into(),
                        file_size: Some(3),
                    },
                    MAX_ENTRY_BYTES,
                )
                .await
                .unwrap();
                sender.write_all(b"abc").await.unwrap();
                sender.write_all(&[0; 32]).await.unwrap();
            });
        assert!(result.unwrap_err().to_string().contains("integrity"));
        assert!(fs::read_dir(folder.path()).unwrap().next().is_none());
    }

    #[tokio::test]
    async fn an_empty_folder_does_not_replace_a_destination_created_after_approval() {
        let folder = tempfile::tempdir().unwrap();
        let destination = folder.path().join("shared");
        let (status, _) = watch::channel(State::Connecting);
        let (mut sender, mut receiver) = duplex(4096);
        // An empty child keeps the receiver waiting while the destination appears.
        let (result, ()) =
            tokio::join!(receive(&mut receiver, &destination, 1, 0, &status), async {
                assert_eq!(sender.read_u8().await.unwrap(), ACCEPT);
                fs::create_dir(&destination).unwrap();
                fs::write(destination.join("existing.txt"), b"keep").unwrap();
                write_metadata(
                    &mut sender,
                    &Entry {
                        relative: "empty".into(),
                        file_size: None,
                    },
                    MAX_ENTRY_BYTES,
                )
                .await
                .unwrap();
            });
        assert!(result.is_err());
        assert_eq!(fs::read(destination.join("existing.txt")).unwrap(), b"keep");
        assert!(!destination.join("empty").exists());
        assert_eq!(fs::read_dir(folder.path()).unwrap().count(), 1);
    }
}
