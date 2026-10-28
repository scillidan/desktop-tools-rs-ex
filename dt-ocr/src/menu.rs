//! Custom tray menu. Win32's `TrackPopupMenu` kept a native Win11 menu on
//! screen after the tray click and blocks the frame; this is a small owner-drawn
//! popup with one Quit row that closes on any outside click, Esc or focus loss.

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    GetLastError, COLORREF, ERROR_CLASS_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, POINT,
    RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateRoundRectRgn, CreateSolidBrush, DeleteObject, DrawTextW,
    EndPaint, FillRect, FrameRect, GetMonitorInfoW, InvalidateRect, MonitorFromPoint, SelectObject,
    SetBkMode, SetTextColor, SetWindowRgn, ANTIALIASED_QUALITY, CLIP_DEFAULT_PRECIS,
    DEFAULT_CHARSET, DEFAULT_PITCH, DT_CENTER, DT_SINGLELINE, DT_VCENTER, FW_NORMAL, HGDIOBJ,
    MONITORINFO, MONITOR_DEFAULTTONEAREST, OUT_DEFAULT_PRECIS, PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VIRTUAL_KEY,
    VK_ESCAPE,
};
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS: PCWSTR = w!("dt-ocr.tray-menu");
const WM_MOUSELEAVE: u32 = 0x02A3;
const TEXT: &str = "Quit dt-ocr";

static MENU_HWND: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
static MENU_FONT: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
static HOVER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static TRACKING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn scale(value: i32, dpi: u32) -> i32 {
    (value * dpi as i32 + 48) / 96
}

/// Shows the menu above the tray icon, clamped into the monitor work area.
pub unsafe fn show(owner: HWND) {
    let existing = MENU_HWND.load(std::sync::atomic::Ordering::SeqCst);
    if existing != 0 && IsWindow(Some(HWND(existing as *mut _))).as_bool() {
        dismiss(HWND(existing as *mut _));
        return;
    }
    let mut pt = POINT::default();
    if GetCursorPos(&mut pt).is_err() {
        return;
    }
    let dpi = match GetDpiForWindow(owner) {
        0 => 96,
        dpi => dpi,
    };
    let (w, h) = (scale(150, dpi), scale(34, dpi));
    let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let work = if GetMonitorInfoW(mon, &mut info).as_bool() {
        info.rcWork
    } else {
        RECT {
            left: 0,
            top: 0,
            right: GetSystemMetrics(SM_CXSCREEN),
            bottom: GetSystemMetrics(SM_CYSCREEN),
        }
    };
    let x = (pt.x - w + scale(4, dpi)).clamp(work.left, (work.right - w).max(work.left));
    let y = (pt.y - h - scale(6, dpi)).clamp(work.top, (work.bottom - h).max(work.top));
    if register().is_err() {
        return;
    }
    let Ok(instance) = GetModuleHandleW(None) else {
        return;
    };
    let Ok(hwnd) = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
        CLASS,
        w!("dt-ocr menu"),
        WS_POPUP,
        x,
        y,
        w,
        h,
        Some(owner),
        None,
        Some(instance.into()),
        None,
    ) else {
        return;
    };
    MENU_HWND.store(hwnd.0 as isize, std::sync::atomic::Ordering::SeqCst);
    HOVER.store(false, std::sync::atomic::Ordering::SeqCst);
    let radius = scale(8, dpi);
    let region = CreateRoundRectRgn(0, 0, w + 1, h + 1, radius, radius);
    if SetWindowRgn(hwnd, Some(region), true) == 0 {
        let _ = DeleteObject(HGDIOBJ(region.0));
    }
    let face: Vec<u16> = "Segoe UI".encode_utf16().chain([0]).collect();
    let font = CreateFontW(
        -scale(13, dpi),
        0,
        0,
        0,
        FW_NORMAL.0 as i32,
        0,
        0,
        0,
        DEFAULT_CHARSET,
        OUT_DEFAULT_PRECIS,
        CLIP_DEFAULT_PRECIS,
        ANTIALIASED_QUALITY,
        DEFAULT_PITCH.0 as u32,
        windows::core::PCWSTR(face.as_ptr()),
    );
    MENU_FONT.store(font.0 as isize, std::sync::atomic::Ordering::SeqCst);
    let _ = ShowWindow(hwnd, SW_SHOW);
    let _ = SetForegroundWindow(hwnd);
    let _ = SetFocus(Some(hwnd));
    SetCapture(hwnd);
    let _ = InvalidateRect(Some(hwnd), None, true);
}

unsafe fn register() -> windows::core::Result<()> {
    let instance: HINSTANCE = GetModuleHandleW(None)?.into();
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        hInstance: instance,
        lpszClassName: CLASS,
        ..Default::default()
    };
    if RegisterClassW(&wc) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
        return Err(windows::core::Error::from_win32());
    }
    Ok(())
}

unsafe fn dismiss(hwnd: HWND) {
    let _ = ReleaseCapture();
    MENU_HWND.store(0, std::sync::atomic::Ordering::SeqCst);
    let _ = ShowWindow(hwnd, SW_HIDE);
    let _ = DestroyWindow(hwnd);
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => LRESULT(0),
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            let bg = CreateSolidBrush(COLORREF(0x00202020)); // RGB(32,32,32)
            FillRect(hdc, &rc, bg);
            let _ = DeleteObject(HGDIOBJ(bg.0));
            let mut item = RECT {
                left: rc.left + 3,
                top: rc.top + 3,
                right: rc.right - 3,
                bottom: rc.bottom - 3,
            };
            if HOVER.load(std::sync::atomic::Ordering::SeqCst) {
                let hover = CreateSolidBrush(COLORREF(0x0000D4FF)); // RGB(255,212,0)
                FillRect(hdc, &item, hover);
                let _ = DeleteObject(HGDIOBJ(hover.0));
                let _ = SetTextColor(hdc, COLORREF(0x00000000));
            } else {
                let border = CreateSolidBrush(COLORREF(0x0000D4FF));
                FrameRect(hdc, &rc, border);
                let _ = DeleteObject(HGDIOBJ(border.0));
                let _ = SetTextColor(hdc, COLORREF(0x00FFFFFF));
            }
            let font = MENU_FONT.load(std::sync::atomic::Ordering::SeqCst);
            if font != 0 {
                let _ = SelectObject(hdc, HGDIOBJ(font as *mut _));
            }
            let _ = SetBkMode(hdc, TRANSPARENT);
            let mut text: Vec<u16> = TEXT.encode_utf16().collect();
            let _ = DrawTextW(
                hdc,
                &mut text,
                &mut item,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE,
            );
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let x = (lparam.0 & 0xffff) as u16 as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xffff) as u16 as i16 as i32;
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            let inside = x >= rc.left && x < rc.right && y >= rc.top && y < rc.bottom;
            if HOVER.swap(inside, std::sync::atomic::Ordering::SeqCst) != inside {
                let _ = InvalidateRect(Some(hwnd), None, true);
            }
            if !TRACKING.swap(true, std::sync::atomic::Ordering::SeqCst) {
                let mut track = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                let _ = TrackMouseEvent(&mut track);
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            TRACKING.store(false, std::sync::atomic::Ordering::SeqCst);
            if HOVER.swap(false, std::sync::atomic::Ordering::SeqCst) {
                let _ = InvalidateRect(Some(hwnd), None, true);
            }
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_ACTIVATE as isize),
        WM_LBUTTONDOWN | WM_RBUTTONDOWN => {
            let x = (lparam.0 & 0xffff) as u16 as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xffff) as u16 as i16 as i32;
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            let inside = x >= rc.left && x < rc.right && y >= rc.top && y < rc.bottom;
            if inside && msg == WM_LBUTTONDOWN {
                dismiss(hwnd);
                PostQuitMessage(0);
            } else {
                dismiss(hwnd);
            }
            LRESULT(0)
        }
        WM_KEYDOWN if VIRTUAL_KEY(wparam.0 as u16) == VK_ESCAPE => {
            dismiss(hwnd);
            LRESULT(0)
        }
        WM_KILLFOCUS => {
            let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            LRESULT(0)
        }
        WM_ACTIVATE if (wparam.0 & 0xffff) == WA_INACTIVE as usize => {
            let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            LRESULT(0)
        }
        WM_CLOSE => {
            dismiss(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            let font = MENU_FONT.swap(0, std::sync::atomic::Ordering::SeqCst);
            if font != 0 {
                let _ = DeleteObject(HGDIOBJ(font as *mut _));
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
