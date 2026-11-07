//! Native tray icon using resource 1 embedded by build.rs. The callback goes
//! to and is handled by the resident host window procedure.
//! Right-click offers Quit.

use anyhow::{ensure, Result};
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIIF_NOSOUND, NIM_ADD,
    NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::*;

pub const WM_TRAY: u32 = WM_APP + 9;

static TASKBAR_CREATED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static HOST: AtomicIsize = AtomicIsize::new(0);
static NOTIFY: AtomicBool = AtomicBool::new(false);

/// Live config: show tray notifications after manual OCR and quick OCR.
pub fn set_notify(on: bool) {
    NOTIFY.store(on, Ordering::Relaxed);
}

pub fn notify_enabled() -> bool {
    NOTIFY.load(Ordering::Relaxed)
}

/// Balloon/toast on the existing tray icon; no-op when disabled or the icon
/// is not there. `NIIF_NOSOUND` keeps it quiet even when enabled.
pub fn notify(title: &str, body: &str) {
    if !notify_enabled() {
        return;
    }
    let host = HOST.load(Ordering::Relaxed);
    if host == 0 {
        return;
    }
    let mut data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: HWND(host as *mut core::ffi::c_void),
        uID: 1,
        uFlags: NIF_INFO,
        dwInfoFlags: NIIF_INFO | NIIF_NOSOUND,
        ..Default::default()
    };
    fill(&mut data.szInfoTitle, title);
    fill(&mut data.szInfo, body);
    unsafe {
        if !Shell_NotifyIconW(NIM_MODIFY, &data).as_bool() {
            crate::logline("warn: tray notification was refused by the shell");
        } else {
            crate::logline(&format!("notify: {title}"));
        }
    }
}

fn fill(dst: &mut [u16], text: &str) {
    let n = dst.len().saturating_sub(1);
    for slot in dst.iter_mut() {
        *slot = 0;
    }
    for (slot, unit) in dst.iter_mut().zip(text.encode_utf16().take(n)) {
        *slot = unit;
    }
}

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
            HOST.store(hwnd.0 as isize, Ordering::Relaxed);
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
    let tip: Vec<u16> = "Screen Tools".encode_utf16().collect();
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
            HOST.store(0, Ordering::Relaxed);
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

#[cfg(test)]
mod tests {
    use super::fill;

    #[test]
    fn info_text_is_nul_terminated_and_truncated() {
        let mut buf = [7u16; 8];
        fill(&mut buf, "abc");
        assert_eq!(&buf[..4], &[97, 98, 99, 0]);
        let mut small = [0u16; 4];
        fill(&mut small, "abcdef");
        assert_eq!(&small[..3], &[97, 98, 99]);
        assert_eq!(small[3], 0);
    }
}
