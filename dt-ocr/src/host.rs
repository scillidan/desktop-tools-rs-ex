//! The resident host window. Hotkeys, the tray icon and the config timer hang
//! off it, never off a frame: a destroyed window silently takes its
//! RegisterHotKey and Shell_NotifyIcon entries with it ("hotkeys stopped
//! working"), and frames are disposable — closing one hides it and merging a
//! closed desktop's duplicates destroys one.

use anyhow::{ensure, Context, Result};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    GetLastError, ERROR_CLASS_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, CW_USEDEFAULT, WNDCLASSW,
    WS_EX_TOOLWINDOW, WS_POPUP,
};

pub struct Host(HWND);

impl Host {
    pub fn new() -> Result<Self> {
        unsafe {
            let instance: HINSTANCE = GetModuleHandleW(None)?.into();
            let class = windows::core::w!("dt-ocr.host");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: instance,
                lpszClassName: PCWSTR(class.as_ptr()),
                ..Default::default()
            };
            ensure!(
                RegisterClassW(&wc) != 0 || GetLastError() == ERROR_CLASS_ALREADY_EXISTS,
                "RegisterClassW(host) failed: {}",
                windows::core::Error::from_win32()
            );
            // A hidden top-level window, not HWND_MESSAGE: a message-only
            // window cannot be made foreground, so its TrackPopupMenu never
            // stays open. WS_EX_TOOLWINDOW keeps it out of the taskbar and
            // Alt-Tab.
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                class,
                windows::core::w!("dt-ocr.host"),
                WS_POPUP,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                None,
                None,
                Some(instance),
                None,
            )
            .context("cannot create the resident host window")?;
            Ok(Self(hwnd))
        }
    }

    pub fn hwnd(&self) -> HWND {
        self.0
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.0);
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // The shell *sends* the tray callback instead of posting it, so it never
    // shows up in the GetMessage loop — it has to be handled here, in the
    // window procedure, or right-clicking the icon does nothing.
    if msg == crate::tray::WM_TRAY {
        crate::tray::handle(hwnd, lparam);
        return LRESULT(0);
    }
    // Explorer restarted: every tray icon is gone, ours has to be added again.
    if msg == crate::tray::taskbar_created() {
        crate::tray::restore(hwnd);
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}
