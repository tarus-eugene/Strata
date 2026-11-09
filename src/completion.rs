use anyhow::{Context, Result, ensure};
use std::{
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    path::{Path, PathBuf},
    process::Command,
};
use strata::engine::CompletionAction;
use windows::Win32::{
    Foundation::{ERROR_NOT_ALL_ASSIGNED, GetLastError, HANDLE, LUID},
    Security::{
        AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED,
        SE_SHUTDOWN_NAME, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
    },
    System::{
        Power::SetSuspendState,
        Shutdown::{
            EWX_POWEROFF, EWX_REBOOT, ExitWindowsEx, SHTDN_REASON_FLAG_PLANNED,
            SHTDN_REASON_MAJOR_APPLICATION,
        },
        Threading::{GetCurrentProcess, OpenProcessToken},
    },
};

pub fn run(action: CompletionAction, path: &Path) -> Result<()> {
    if action == CompletionAction::Open {
        return open_download(path);
    }

    let mut token = HANDLE::default();
    let token = unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut token,
        )?;
        OwnedHandle::from_raw_handle(token.0)
    };
    let handle = HANDLE(token.as_raw_handle());
    let mut luid = LUID::default();
    let mut previous = TOKEN_PRIVILEGES::default();
    let mut returned = 0;
    unsafe {
        LookupPrivilegeValueW(None, SE_SHUTDOWN_NAME, &mut luid)?;
        let requested = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES {
                Luid: luid,
                Attributes: SE_PRIVILEGE_ENABLED,
            }],
        };
        AdjustTokenPrivileges(
            handle,
            false,
            Some(&requested),
            size_of::<TOKEN_PRIVILEGES>() as u32,
            Some(&mut previous),
            Some(&mut returned),
        )?;
        ensure!(
            GetLastError() != ERROR_NOT_ALL_ASSIGNED,
            "Windows did not grant permission for the power action"
        );
    }
    let reason = SHTDN_REASON_MAJOR_APPLICATION | SHTDN_REASON_FLAG_PLANNED;
    let result = unsafe {
        match action {
            CompletionAction::ShutDown => ExitWindowsEx(EWX_POWEROFF, reason).map_err(Into::into),
            CompletionAction::Restart => ExitWindowsEx(EWX_REBOOT, reason).map_err(Into::into),
            CompletionAction::Sleep if SetSuspendState(false, false, false) => Ok(()),
            CompletionAction::Sleep => Err(std::io::Error::last_os_error().into()),
            CompletionAction::Open => unreachable!(),
        }
    };
    if previous.PrivilegeCount != 0 {
        unsafe { AdjustTokenPrivileges(handle, false, Some(&previous), 0, None, None)? };
    }
    result
}

fn runnable(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("exe") || extension.eq_ignore_ascii_case("com")
    })
}

fn open_download(path: &Path) -> Result<()> {
    ensure!(path.is_absolute(), "The download location is invalid");
    let parent = path
        .parent()
        .context("The download folder is unavailable")?;
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.is_file()
        && runnable(path)
        && Command::new(path).current_dir(parent).spawn().is_ok()
    {
        return Ok(());
    }
    let folder = if metadata.is_dir() { path } else { parent };
    let explorer =
        PathBuf::from(std::env::var_os("SystemRoot").context("Windows folder not found")?)
            .join("explorer.exe");
    Command::new(explorer)
        .arg(folder)
        .spawn()
        .context("Could not open the download folder")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_direct_windows_programs_are_launch_candidates() {
        assert!(runnable(Path::new("setup.EXE")));
        assert!(runnable(Path::new("app.com")));
        assert!(!runnable(Path::new("setup.exe.zip")));
        assert!(!runnable(Path::new("script.cmd")));
        assert!(!runnable(Path::new("shortcut.lnk")));
    }
}
