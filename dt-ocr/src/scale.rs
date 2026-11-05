//! Window zoom v1: a fullscreen topmost window presenting the focused window
//! through a DWM thumbnail. Hardware-composited and live, so it works on
//! windows that cannot be resized. Magpie-style D3D11 shader upscaling is the
//! later upgrade (`shortcut: DWM thumbnail instead of a capture-and-present
//! pipeline; upgrade when scaled text needs sharpening`).

use anyhow::{ensure, Context, Result};
use windows::core::w;
use windows::Win32::Foundation::{
    GetLastError, ERROR_CLASS_ALREADY_EXISTS, HWND, LPARAM, LRESULT, RECT, TRUE, WPARAM,
};
use windows::Win32::Graphics::Dwm::{
    DwmRegisterThumbnail, DwmUnregisterThumbnail, DwmUpdateThumbnailProperties,
    DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY, DWM_TNP_RECTDESTINATION,
    DWM_TNP_SOURCECLIENTAREAONLY, DWM_TNP_VISIBLE,
};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, GetClientRect, GetForegroundWindow, GetWindowRect,
    RegisterClassW, SetForegroundWindow, ShowWindow, CS_HREDRAW, CS_VREDRAW, SW_SHOW, WM_CLOSE,
    WM_DESTROY, WM_KEYDOWN, WM_RBUTTONUP, WNDCLASSW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use crate::logline;

const CLASS_NAME: windows::core::PCWSTR = w!("dt-ocr.zoom");

static ZOOM: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
static THUMB: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

pub unsafe fn toggle() {
    let hwnd = ZOOM.load(std::sync::atomic::Ordering::SeqCst);
    if hwnd != 0 {
        close(HWND(hwnd as *mut _));
        return;
    }
    if let Err(e) = open() {
        logline(&format!("scale failed: {e:#}"));
    }
}

/// Aspect-preserving rect of `sw x sh` centred in `dw x dh` (letterbox).
fn fit(sw: i32, sh: i32, dw: i32, dh: i32) -> RECT {
    if sw <= 0 || sh <= 0 || dw <= 0 || dh <= 0 {
        return RECT {
            left: 0,
            top: 0,
            right: dw.max(0),
            bottom: dh.max(0),
        };
    }
    let scale = (dw as f64 / sw as f64).min(dh as f64 / sh as f64);
    let w = ((sw as f64 * scale).round() as i32).clamp(1, dw);
    let h = ((sh as f64 * scale).round() as i32).clamp(1, dh);
    RECT {
        left: (dw - w) / 2,
        top: (dh - h) / 2,
        right: (dw - w) / 2 + w,
        bottom: (dh - h) / 2 + h,
    }
}

unsafe fn open() -> Result<()> {
    let source = crate::capture::target_window().context("no usable window to zoom")?;
    let mut src = RECT::default();
    if GetClientRect(source, &mut src).is_err() || src.right <= src.left || src.bottom <= src.top {
        let _ = GetWindowRect(source, &mut src);
    }
    let mon = MonitorFromWindow(source, MONITOR_DEFAULTTONEAREST);
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    ensure!(
        GetMonitorInfoW(mon, &mut info).as_bool(),
        "GetMonitorInfoW failed"
    );
    let m = info.rcMonitor;
    register()?;
    let instance: windows::Win32::Foundation::HINSTANCE = GetModuleHandleW(None)?.into();
    let hwnd = CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
        CLASS_NAME,
        w!("dt-ocr zoom"),
        WS_POPUP,
        m.left,
        m.top,
        m.right - m.left,
        m.bottom - m.top,
        None,
        None,
        Some(instance),
        None,
    )?;
    let dest = fit(
        src.right - src.left,
        src.bottom - src.top,
        m.right - m.left,
        m.bottom - m.top,
    );
    let thumb = DwmRegisterThumbnail(hwnd, source).context("DwmRegisterThumbnail failed")?;
    let props = DWM_THUMBNAIL_PROPERTIES {
        dwFlags: DWM_TNP_RECTDESTINATION
            | DWM_TNP_VISIBLE
            | DWM_TNP_OPACITY
            | DWM_TNP_SOURCECLIENTAREAONLY,
        rcDestination: dest,
        opacity: 255,
        fVisible: TRUE,
        fSourceClientAreaOnly: TRUE,
        ..Default::default()
    };
    if let Err(e) = DwmUpdateThumbnailProperties(thumb, &props) {
        let _ = DwmUnregisterThumbnail(thumb);
        let _ = DestroyWindow(hwnd);
        return Err(e).context("DwmUpdateThumbnailProperties failed");
    }
    ZOOM.store(hwnd.0 as isize, std::sync::atomic::Ordering::SeqCst);
    THUMB.store(thumb, std::sync::atomic::Ordering::SeqCst);
    let _ = ShowWindow(hwnd, SW_SHOW);
    let _ = SetForegroundWindow(hwnd);
    let _ = SetFocus(Some(hwnd));
    logline(&format!(
        "scale: zoom {}x{} into {}x{}",
        src.right - src.left,
        src.bottom - src.top,
        dest.right - dest.left,
        dest.bottom - dest.top
    ));
    Ok(())
}

unsafe fn close(hwnd: HWND) {
    let thumb = THUMB.swap(0, std::sync::atomic::Ordering::SeqCst);
    ZOOM.store(0, std::sync::atomic::Ordering::SeqCst);
    if thumb != 0 {
        let _ = DwmUnregisterThumbnail(thumb);
    }
    let _ = DestroyWindow(hwnd);
    logline("scale: exit");
}

unsafe fn register() -> Result<()> {
    let instance: windows::Win32::Foundation::HINSTANCE = GetModuleHandleW(None)?.into();
    let wc = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(wndproc),
        hInstance: instance,
        lpszClassName: CLASS_NAME,
        ..Default::default()
    };
    ensure!(
        RegisterClassW(&wc) != 0 || GetLastError() == ERROR_CLASS_ALREADY_EXISTS,
        "RegisterClassW(zoom) failed: {}",
        windows::core::Error::from_win32()
    );
    Ok(())
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_KEYDOWN => {
            // Esc leaves the zoom; the source window is untouched.
            if wparam.0 == 0x1b {
                close(hwnd);
            }
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            close(hwnd);
            LRESULT(0)
        }
        WM_CLOSE => {
            close(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            // close() already released the thumbnail; just stop here.
            let _ = GetForegroundWindow();
            LRESULT(0)
        }
        _ => windows::Win32::UI::WindowsAndMessaging::DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_letterboxes_and_centres() {
        // 4:3 source into 16:9 screen: full height, side bars.
        let r = fit(800, 600, 1920, 1080);
        assert_eq!(
            (r.left, r.top, r.right - r.left, r.bottom - r.top),
            (240, 0, 1440, 1080)
        );
        // 16:9 source into 4:3: full width, top/bottom bars.
        let r = fit(1920, 1080, 1024, 768);
        assert_eq!((r.left, r.right - r.left), (0, 1024));
        assert_eq!((r.top, r.bottom - r.top), (96, 576));
        // Same aspect fills exactly.
        let r = fit(1000, 500, 800, 400);
        assert_eq!((r.left, r.top, r.right, r.bottom), (0, 0, 800, 400));
        // Degenerate sizes do not panic.
        let r = fit(0, 0, 100, 50);
        assert_eq!((r.right, r.bottom), (100, 50));
    }
}
