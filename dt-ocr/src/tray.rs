//! Native tray icon using resource 1 embedded by build.rs. The frame receives
//! its callback directly, so no extra message window or tray dependency is
//! needed. Right-click offers Quit.

use anyhow::{ensure, Result};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::*;

pub const WM_TRAY: u32 = WM_APP + 9;
const QUIT_ID: usize = 1;

pub struct Tray {
    hwnd: HWND,
}

impl Tray {
    pub fn new(hwnd: HWND) -> Result<Self> {
        unsafe {
            let instance: HINSTANCE = GetModuleHandleW(None)?.into();
            let resource = PCWSTR::from_raw(std::ptr::without_provenance(1));
            let icon = LoadIconW(Some(instance), resource)?;
            let mut data = notify_data(hwnd, icon);
            let tip: Vec<u16> = "dt-ocr".encode_utf16().collect();
            data.szTip[..tip.len()].copy_from_slice(&tip);
            ensure!(
                Shell_NotifyIconW(NIM_ADD, &data).as_bool(),
                "Shell_NotifyIconW(NIM_ADD) failed"
            );
            Ok(Self { hwnd })
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &notify_data(self.hwnd, Default::default()));
        }
    }
}

fn notify_data(
    hwnd: HWND,
    icon: windows::Win32::UI::WindowsAndMessaging::HICON,
) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: WM_TRAY,
        hIcon: icon,
        ..Default::default()
    }
}

/// Called directly by the frame window procedure for the shell callback.
pub unsafe fn handle(hwnd: HWND, lparam: LPARAM) {
    if !matches!(lparam.0 as u32 & 0xffff, WM_RBUTTONUP | WM_CONTEXTMENU) {
        return;
    }
    let Ok(menu) = CreatePopupMenu() else {
        return;
    };
    if AppendMenuW(menu, MF_STRING, QUIT_ID, w!("Quit")).is_err() {
        let _ = DestroyMenu(menu);
        return;
    }
    let mut pt = POINT::default();
    if GetCursorPos(&mut pt).is_err() {
        let _ = DestroyMenu(menu);
        return;
    }
    // These two messages are required for a tray popup to dismiss cleanly.
    let _ = SetForegroundWindow(hwnd);
    let cmd = TrackPopupMenu(
        menu,
        TPM_LEFTALIGN | TPM_TOPALIGN | TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
        pt.x,
        pt.y,
        None,
        hwnd,
        None,
    );
    let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
    let _ = DestroyMenu(menu);
    if cmd.0 == QUIT_ID as i32 {
        PostQuitMessage(0);
    }
}
