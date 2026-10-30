use anyhow::{Result, ensure};
use gpui::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::cell::Cell;
use windows::{
    Win32::{
        Foundation::{
            ERROR_CLASS_ALREADY_EXISTS, GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, POINT,
            WPARAM,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{Shell::*, WindowsAndMessaging::*},
    },
    core::{PCWSTR, w},
};

const CALLBACK: u32 = WM_APP + 1;
const OPEN: u32 = 1;
const QUIT: u32 = 2;

pub struct Tray {
    window: HWND,
    state: Box<TrayState>,
}

struct TrayState {
    main: HWND,
    data: Cell<NOTIFYICONDATAW>,
    hidden: Cell<bool>,
    quitting: Cell<bool>,
    taskbar_created: u32,
}

impl Tray {
    pub fn new(window: &Window) -> Result<Self> {
        let handle = HasWindowHandle::window_handle(window)
            .map_err(|error| anyhow::anyhow!("Cannot access the app window: {error}"))?;
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            anyhow::bail!("The system tray requires a Windows window");
        };
        Self::from_handle(HWND(handle.hwnd.get() as *mut _))
    }

    fn from_handle(main: HWND) -> Result<Self> {
        unsafe {
            let instance = HINSTANCE(GetModuleHandleW(None)?.0);
            let class = WNDCLASSW {
                lpfnWndProc: Some(tray_proc),
                hInstance: instance,
                lpszClassName: w!("Strata.TrayWindow"),
                ..Default::default()
            };
            if RegisterClassW(&class) == 0 {
                ensure!(
                    GetLastError() == ERROR_CLASS_ALREADY_EXISTS,
                    "Cannot register the system tray window"
                );
            }
            let state = Box::new(TrayState {
                main,
                data: Cell::new(NOTIFYICONDATAW::default()),
                hidden: Cell::new(false),
                quitting: Cell::new(false),
                taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
            });
            let window = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                w!("Strata.TrayWindow"),
                w!("Strata"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance),
                Some((&*state as *const TrayState).cast()),
            )?;
            let result = (|| -> Result<()> {
                let icon = LoadImageW(
                    Some(instance),
                    PCWSTR(1 as _),
                    IMAGE_ICON,
                    0,
                    0,
                    LR_DEFAULTSIZE | LR_SHARED,
                )?;
                let mut tip = [0u16; 128];
                for (target, ch) in tip
                    .iter_mut()
                    .zip("Strata - Download manager".encode_utf16())
                {
                    *target = ch;
                }
                state.data.set(NOTIFYICONDATAW {
                    cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                    hWnd: window,
                    uID: 1,
                    uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP,
                    uCallbackMessage: CALLBACK,
                    hIcon: HICON(icon.0),
                    szTip: tip,
                    ..Default::default()
                });
                ensure!(
                    state.add_icon(),
                    "Windows could not add the Strata system tray icon"
                );
                Ok(())
            })();
            if let Err(error) = result {
                let _ = DestroyWindow(window);
                return Err(error);
            }
            Ok(Self { window, state })
        }
    }

    pub fn hide(&self) {
        self.state.hidden.set(true);
        unsafe {
            let _ = ShowWindowAsync(self.state.main, SW_HIDE);
        }
    }
    pub fn show(&self) {
        self.state.show();
    }
    pub fn is_hidden(&self) -> bool {
        self.state.hidden.get()
    }
    pub fn quitting(&self) -> bool {
        self.state.quitting.get()
    }
    pub fn remove_icon(&self) {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &self.state.data.get());
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        self.remove_icon();
        unsafe {
            let _ = DestroyWindow(self.window);
        }
    }
}

impl TrayState {
    fn add_icon(&self) -> bool {
        unsafe {
            let mut data = self.data.get();
            if !Shell_NotifyIconW(NIM_ADD, &data).as_bool() {
                return false;
            }
            data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            let _ = Shell_NotifyIconW(NIM_SETVERSION, &data);
            true
        }
    }

    fn show(&self) {
        self.hidden.set(false);
        unsafe {
            let _ = ShowWindowAsync(
                self.main,
                if IsIconic(self.main).as_bool() {
                    SW_RESTORE
                } else {
                    SW_SHOW
                },
            );
            let _ = SetForegroundWindow(self.main);
        }
    }

    fn menu(&self, owner: HWND) {
        unsafe {
            let Ok(menu) = CreatePopupMenu() else {
                return;
            };
            let result = (|| -> windows::core::Result<()> {
                AppendMenuW(menu, MF_STRING, OPEN as usize, w!("Open Strata"))?;
                AppendMenuW(menu, MF_SEPARATOR, 0, None)?;
                AppendMenuW(menu, MF_STRING, QUIT as usize, w!("Quit Strata"))?;
                let mut position = POINT::default();
                GetCursorPos(&mut position)?;
                let _ = SetForegroundWindow(owner);
                match TrackPopupMenu(
                    menu,
                    TPM_RETURNCMD | TPM_RIGHTBUTTON,
                    position.x,
                    position.y,
                    None,
                    owner,
                    None,
                )
                .0 as u32
                {
                    OPEN => self.show(),
                    QUIT => self.quitting.set(true),
                    _ => {}
                }
                let _ = PostMessageW(Some(owner), WM_NULL, WPARAM(0), LPARAM(0));
                Ok(())
            })();
            let _ = result;
            let _ = DestroyMenu(menu);
        }
    }
}

#[derive(Debug, PartialEq)]
enum Action {
    Open,
    Menu,
}

fn action(message: u32) -> Option<Action> {
    if matches!(message, NIN_SELECT | WM_LBUTTONUP | WM_LBUTTONDBLCLK)
        || message == (NIN_SELECT | NINF_KEY)
    {
        Some(Action::Open)
    } else if matches!(message, WM_CONTEXTMENU | WM_RBUTTONUP) {
        Some(Action::Menu)
    } else {
        None
    }
}

unsafe extern "system" fn tray_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        if message == WM_NCCREATE {
            let create = &*(lparam.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(window, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        let state = GetWindowLongPtrW(window, GWLP_USERDATA) as *const TrayState;
        if !state.is_null() {
            let state = &*state;
            if message == CALLBACK {
                match action(lparam.0 as u32 & 0xffff) {
                    Some(Action::Open) => state.show(),
                    Some(Action::Menu) => state.menu(window),
                    None => {}
                }
                return LRESULT(0);
            }
            if message == state.taskbar_created && state.taskbar_created != 0 {
                state.add_icon();
                return LRESULT(0);
            }
        }
        if message == WM_NCDESTROY {
            SetWindowLongPtrW(window, GWLP_USERDATA, 0);
        }
        DefWindowProcW(window, message, wparam, lparam)
    }
}

#[cfg(test)]
mod tests {
    use super::{Action, action};
    use windows::Win32::UI::{
        Shell::{NIN_SELECT, NINF_KEY},
        WindowsAndMessaging::*,
    };
    #[test]
    fn maps_mouse_and_keyboard_tray_actions() {
        assert_eq!(action(NIN_SELECT), Some(Action::Open));
        assert_eq!(action(NIN_SELECT | NINF_KEY), Some(Action::Open));
        assert_eq!(action(WM_LBUTTONDBLCLK), Some(Action::Open));
        assert_eq!(action(WM_CONTEXTMENU), Some(Action::Menu));
        assert_eq!(action(WM_RBUTTONUP), Some(Action::Menu));
        assert_eq!(action(WM_MOUSEMOVE), None);
    }

    #[test]
    #[ignore = "requires an interactive Windows Explorer notification area"]
    fn native_tray_hides_restores_and_cleans_up() {
        use super::Tray;
        use windows::{
            Win32::Foundation::{LPARAM, WPARAM},
            core::w,
        };
        unsafe {
            let main = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                w!("STATIC"),
                w!("Strata tray test"),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            let tray = Tray::from_handle(main)
                .expect("The test needs Windows Explorer and the embedded Strata icon");
            let pump = || {
                let mut message = MSG::default();
                while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            };
            SendMessageW(
                tray.window,
                super::CALLBACK,
                Some(WPARAM(0)),
                Some(LPARAM(NIN_SELECT as isize)),
            );
            pump();
            assert!(!tray.is_hidden());
            assert!(IsWindowVisible(main).as_bool());
            tray.hide();
            pump();
            assert!(tray.is_hidden());
            assert!(IsWindow(Some(main)).as_bool());
            assert!(!IsWindowVisible(main).as_bool());
            let message_window = tray.window;
            drop(tray);
            assert!(!IsWindow(Some(message_window)).as_bool());
            let _ = DestroyWindow(main);
        }
    }
}
