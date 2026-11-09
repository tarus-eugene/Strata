use anyhow::{Context, Result, ensure};
use std::{
    fs::{File, OpenOptions},
    os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    path::{Path, PathBuf},
};
use windows::{
    Win32::{
        Foundation::{
            DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, HANDLE, S_OK,
        },
        Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO,
            FILE_NAME_NORMALIZED, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
            FILE_SHARE_WRITE, FileIdInfo, GetFileInformationByHandleEx, GetFinalPathNameByHandleW,
        },
        System::{
            Com::IDataObject,
            Ole::{DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_MOVE, IDropSource, IDropSource_Impl},
            SystemServices::{MK_LBUTTON, MK_MBUTTON, MK_RBUTTON, MODIFIERKEYS_FLAGS},
        },
        UI::{
            Input::KeyboardAndMouse::{
                GetAsyncKeyState, ReleaseCapture, VIRTUAL_KEY, VK_LBUTTON, VK_MBUTTON,
            },
            Shell::{BHID_DataObject, IShellItem, SHCreateItemFromParsingName, SHDoDragDrop},
        },
    },
    core::HSTRING,
};
use windows_core::{BOOL, HRESULT, implement};

pub fn move_download(path: &Path) -> Result<Option<PathBuf>> {
    if !held(VK_LBUTTON) {
        return Ok(None);
    }
    let file = open_dragged_item(path)?;
    let original = current_path(&file)?;
    shell_drag(path, VK_LBUTTON, DROPEFFECT_MOVE, None)?;
    moved_path(&file, &original)
}

pub fn copy_download(path: &Path) -> Result<Option<PathBuf>> {
    let source: IDropSource = MiddleDropSource.into();
    shell_drag(path, VK_MBUTTON, DROPEFFECT_COPY, Some(&source))?;
    Ok(None)
}

fn held(button: VIRTUAL_KEY) -> bool {
    (unsafe { GetAsyncKeyState(i32::from(button.0)) }) < 0
}

fn shell_drag(
    path: &Path,
    button: VIRTUAL_KEY,
    effect: DROPEFFECT,
    source: Option<&IDropSource>,
) -> Result<()> {
    if !held(button) {
        return Ok(());
    }
    // GPUI initializes OLE on this thread.
    unsafe {
        let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(path.as_os_str()), None)?;
        let data: IDataObject = item.BindToHandler(None, &BHID_DataObject)?;
        if held(button) {
            ReleaseCapture()?;
            SHDoDragDrop(None, &data, source, effect)?;
        }
    }
    Ok(())
}

#[implement(IDropSource)]
struct MiddleDropSource;

impl IDropSource_Impl for MiddleDropSource_Impl {
    fn QueryContinueDrag(&self, escape: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
        if escape.as_bool() || (keys & (MK_LBUTTON | MK_RBUTTON)) != MODIFIERKEYS_FLAGS(0) {
            DRAGDROP_S_CANCEL
        } else if keys.contains(MK_MBUTTON) {
            S_OK
        } else {
            DRAGDROP_S_DROP
        }
    }

    fn GiveFeedback(&self, _: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

fn open_dragged_item(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES.0)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
        .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0)
        .open(path)?;
    ensure!(
        !file.metadata()?.file_type().is_symlink(),
        "Cannot track a move of a link"
    );
    Ok(file)
}

fn current_path(file: &File) -> Result<PathBuf> {
    let mut buffer = vec![0; 32768];
    let len = unsafe {
        GetFinalPathNameByHandleW(
            HANDLE(file.as_raw_handle()),
            &mut buffer,
            FILE_NAME_NORMALIZED,
        )
    } as usize;
    if len == 0 {
        return Err(std::io::Error::last_os_error()).context("Cannot resolve the dragged file");
    }
    ensure!(len < buffer.len(), "The dragged file's path is too long");
    let path = String::from_utf16(&buffer[..len])?;
    let path = if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{unc}"))
    } else {
        PathBuf::from(
            path.strip_prefix(r"\\?\")
                .context("Expected a Windows filesystem path")?,
        )
    };
    ensure!(
        path.is_absolute(),
        "Expected an absolute path for the dragged file"
    );
    Ok(path)
}

fn identity(file: &File) -> Result<FILE_ID_INFO> {
    let mut id = FILE_ID_INFO::default();
    unsafe {
        GetFileInformationByHandleEx(
            HANDLE(file.as_raw_handle()),
            FileIdInfo,
            (&mut id as *mut FILE_ID_INFO).cast(),
            size_of::<FILE_ID_INFO>() as u32,
        )?;
    }
    Ok(id)
}

fn moved_path(file: &File, original: &Path) -> Result<Option<PathBuf>> {
    // shortcut: an open handle follows same-volume moves. Track across volumes when the drop target supplies a verified destination.
    let destination = current_path(file)?;
    if destination == original
        || destination
            .components()
            .nth(2)
            .is_some_and(|part| part.as_os_str().eq_ignore_ascii_case("$Recycle.Bin"))
    {
        return Ok(None);
    }
    let found = open_dragged_item(&destination)?;
    ensure!(
        identity(file)? == identity(&found)?,
        "The moved file could not be identified at its new location"
    );
    Ok(Some(destination))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn middle_drag_drops_on_middle_release_and_cancels_on_escape() {
        let source: IDropSource = MiddleDropSource.into();
        unsafe {
            assert_eq!(source.QueryContinueDrag(false, MK_MBUTTON), S_OK);
            assert_eq!(
                source.QueryContinueDrag(false, MODIFIERKEYS_FLAGS(0)),
                DRAGDROP_S_DROP
            );
            assert_eq!(
                source.QueryContinueDrag(true, MK_MBUTTON),
                DRAGDROP_S_CANCEL
            );
            assert_eq!(
                source.QueryContinueDrag(false, MK_MBUTTON | MK_LBUTTON),
                DRAGDROP_S_CANCEL
            );
        }
    }

    #[test]
    fn follows_files_and_folders_through_a_move_with_rename() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("destination");
        std::fs::create_dir(&destination).unwrap();
        for folder in [false, true] {
            let path = root.path().join(if folder { "Folder" } else { "File.txt" });
            if folder {
                std::fs::create_dir(&path).unwrap();
            } else {
                std::fs::write(&path, b"fixture").unwrap();
            }
            let file = open_dragged_item(&path).unwrap();
            let original = current_path(&file).unwrap();
            assert_eq!(moved_path(&file, &original).unwrap(), None);

            let moved = destination.join(if folder {
                "Moved folder"
            } else {
                "Moved file.txt"
            });
            std::fs::rename(&path, &moved).unwrap();
            assert_eq!(moved_path(&file, &original).unwrap().unwrap(), moved);
        }
    }
}
