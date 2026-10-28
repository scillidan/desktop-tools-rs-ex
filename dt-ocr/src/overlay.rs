//! Draw-select overlay: a full-virtual-screen, topmost, hollow, crosshair
//! window. Left-drag paints a rubber-band rect; on release the rect either
//! places the frame (`Use::Frame`) or is handed to the caller to capture
//! (`Use::Snip`). Esc (or a too-small drag) cancels. The band is a plain
//! border — shortcut: no per-pixel alpha blending, a border is enough to
//! see the selection.

use crate::frame::{rect_of, COLOR_KEY};
use crate::logline;
use anyhow::Result;
use dt_common::layout::Rect;
use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, VIRTUAL_KEY, VK_ESCAPE,
};
use windows::Win32::UI::WindowsAndMessaging::*;

const BAND: u32 = 0x003C3CE6; // RGB(230, 60, 60) red — drawing in progress

struct DragState {
    start: Option<POINT>,
    cur: POINT,
}

static OVERLAY: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

fn overlay_hwnd() -> HWND {
    HWND(OVERLAY.load(std::sync::atomic::Ordering::Relaxed) as *mut core::ffi::c_void)
}

/// Rect of the entire virtual screen (all monitors).
fn virtual_screen() -> RECT {
    unsafe {
        RECT {
            left: GetSystemMetrics(SM_XVIRTUALSCREEN),
            top: GetSystemMetrics(SM_YVIRTUALSCREEN),
            right: GetSystemMetrics(SM_XVIRTUALSCREEN) + GetSystemMetrics(SM_CXVIRTUALSCREEN),
            bottom: GetSystemMetrics(SM_YVIRTUALSCREEN) + GetSystemMetrics(SM_CYVIRTUALSCREEN),
        }
    }
}

/// Creates (or recreates) the overlay and takes over the mouse until the
/// drag finishes; the selected rect goes to the frame window.
pub unsafe fn begin() -> Result<()> {
    if IsWindow(Some(overlay_hwnd())).as_bool() {
        let _ = DestroyWindow(overlay_hwnd());
    }
    let instance: HINSTANCE = GetModuleHandleW(None)?.into();
    let class = w!("dt-ocr.overlay");
    let wc = WNDCLASSW {
        style: WNDCLASS_STYLES(0),
        lpfnWndProc: Some(wndproc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: HICON::default(),
        hCursor: LoadCursorW(None, IDC_CROSS).unwrap_or_default(),
        hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as _),
        lpszMenuName: windows::core::PCWSTR::null(),
        lpszClassName: windows::core::PCWSTR(class.as_ptr()),
    };
    let atom = RegisterClassW(&wc);
    if atom == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
        anyhow::bail!(
            "RegisterClassW(overlay) failed: {}",
            windows::core::Error::from_win32()
        );
    }
    let vs = virtual_screen();
    let hwnd = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
        class,
        windows::core::PCWSTR::null(),
        WS_POPUP | WS_VISIBLE,
        vs.left,
        vs.top,
        vs.right - vs.left,
        vs.bottom - vs.top,
        None,
        None,
        Some(instance),
        None,
    )?;
    SetLayeredWindowAttributes(hwnd, COLOR_KEY, 0, LWA_COLORKEY)?;
    SetWindowLongPtrW(
        hwnd,
        GWLP_USERDATA,
        Box::into_raw(Box::new(DragState {
            start: None,
            cur: POINT::default(),
        })) as isize,
    );
    OVERLAY.store(hwnd.0 as isize, std::sync::atomic::Ordering::Relaxed);
    let _ = SetForegroundWindow(hwnd);
    let _ = SetCapture(hwnd);
    logline("draw: drag a rect (Esc cancels)");
    Ok(())
}

unsafe fn drag(hwnd: HWND) -> &'static mut DragState {
    &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut DragState)
}

/// Client-coords band rect from the drag state.
fn band(s: &DragState) -> Option<RECT> {
    let start = s.start?;
    Some(RECT {
        left: start.x.min(s.cur.x),
        top: start.y.min(s.cur.y),
        right: start.x.max(s.cur.x),
        bottom: start.y.max(s.cur.y),
    })
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_LBUTTONDOWN => {
            let s = drag(hwnd);
            let pt = POINT {
                x: (lparam.0 & 0xffff) as i32,
                y: ((lparam.0 >> 16) & 0xffff) as i32,
            };
            logline(&format!("draw: anchor at ({},{})", pt.x, pt.y));
            s.start = Some(pt);
            s.cur = pt;
            let _ = SetCapture(hwnd);
            let _ = InvalidateRect(Some(hwnd), None, false);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let s = drag(hwnd);
            if s.start.is_some() {
                let old = band(s);
                s.cur = POINT {
                    x: (lparam.0 & 0xffff) as i32,
                    y: ((lparam.0 >> 16) & 0xffff) as i32,
                };
                match (old, band(s)) {
                    (Some(a), Some(b)) => {
                        let union = RECT {
                            left: a.left.min(b.left),
                            top: a.top.min(b.top),
                            right: a.right.max(b.right),
                            bottom: a.bottom.max(b.bottom),
                        };
                        let _ = InvalidateRect(Some(hwnd), Some(&union as *const RECT), false);
                    }
                    _ => {
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                };
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let rect = band(drag(hwnd));
            let _ = ReleaseCapture();
            finish(hwnd, rect);
            LRESULT(0)
        }
        WM_KEYDOWN if VIRTUAL_KEY(wparam.0 as u16) == VK_ESCAPE => {
            logline("draw: cancelled");
            let _ = ReleaseCapture();
            finish(hwnd, None);
            LRESULT(0)
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);
            let key = CreateSolidBrush(COLOR_KEY);
            let _ = FillRect(hdc, &client, key);
            let _ = DeleteObject(key.into());
            if let Some(r) = band(drag(hwnd)) {
                let brush = CreateSolidBrush(COLORREF(BAND));
                let _ = FrameRect(hdc, &r, brush);
                let _ = DeleteObject(brush.into());
            }
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_DESTROY => {
            drop(Box::from_raw(
                GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut DragState
            ));
            OVERLAY.store(0, std::sync::atomic::Ordering::Relaxed);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Converts the client-coords band to a screen rect (before destroying the
/// window — its origin goes with it), places the frame, then cleans up.
unsafe fn finish(hwnd: HWND, band_rect: Option<RECT>) {
    let origin = rect_of({
        let mut wr = RECT::default();
        let _ = GetWindowRect(hwnd, &mut wr);
        wr
    });
    let _ = DestroyWindow(hwnd);
    let Some(r) = band_rect else {
        return;
    };
    let rect = Rect {
        l: origin.l + r.left,
        t: origin.t + r.top,
        r: origin.l + r.right,
        b: origin.t + r.bottom,
    };
    if rect.w() < 9 || rect.h() < 9 {
        logline("draw: selection too small, ignored");
        return;
    }
    logline(&format!("draw: selected {rect:?}"));
    crate::frame::place(crate::frame_hwnd(), rect);
}
