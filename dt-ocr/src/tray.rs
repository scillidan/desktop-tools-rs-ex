//! Native tray icon using resource 1 embedded by build.rs. The callback goes
//! to and is handled by the resident host window procedure.
//! Right-click offers Quit.

use anyhow::{ensure, Result};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::*;

pub const WM_TRAY: u32 = WM_APP + 9;

static TASKBAR_CREATED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The shell's "I restarted" broadcast; explorer restarts (or a crash) drop
/// every tray icon, so the host window has to add ours again.
pub fn taskbar_created() -> u32 {
    let cached = TASKBAR_CREATED.load(std::sync::atomic::Ordering::Relaxed);
    if cached != 0 {
        return cached;
    }
    let msg = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
    TASKBAR_CREATED.store(msg, std::sync::atomic::Ordering::Relaxed);
    msg
}

pub struct Tray {
    hwnd: HWND,
}

impl Tray {
    pub fn new(hwnd: HWND) -> Result<Self> {
        unsafe {
            ensure!(add(hwnd), "Shell_NotifyIconW(NIM_ADD) failed");
            Ok(Self { hwnd })
        }
    }
}

/// Adds (or re-adds) the icon; shared by startup and the TaskbarCreated path.
unsafe fn add(hwnd: HWND) -> bool {
    let Ok(instance) = GetModuleHandleW(None) else {
        return false;
    };
    let resource = PCWSTR::from_raw(std::ptr::without_provenance(1));
    let Ok(icon) = LoadIconW(Some(instance.into()), resource) else {
        return false;
    };
    let mut data = notify_data(hwnd, icon);
    let tip: Vec<u16> = "dt-ocr".encode_utf16().collect();
    data.szTip[..tip.len()].copy_from_slice(&tip);
    Shell_NotifyIconW(NIM_ADD, &data).as_bool()
}

/// Re-adds the icon after the shell restarted.
pub unsafe fn restore(hwnd: HWND) {
    if !add(hwnd) {
        crate::logline("warn: cannot re-add the tray icon");
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

/// Called by the resident host window procedure for the shell's WM_TRAY callback.
pub unsafe fn handle(hwnd: HWND, lparam: LPARAM) {
    if matches!(lparam.0 as u32 & 0xffff, WM_RBUTTONUP | WM_CONTEXTMENU) && crate::menu::show(hwnd)
    {
        PostQuitMessage(0);
    }
}
