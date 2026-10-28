//! Pinned screenshots: a borderless always-on-top layered window showing the
//! captured image, edged by translucent cyan hatch (or a solid line, or
//! nothing). Interior drag uses the system move loop; edge drag is our own
//! aspect-locked resize, so there is no non-client border to eat pixels or
//! leave ghosts. Hovering an edge shows a cyan grip and a resize cursor.
//! Esc/right-click closes.
//! shortcut: no virtual-desktop isolation — add per-desktop pinning with the
//! `desktops.rs` pattern if pins start leaking across desktops.

use crate::config::PinBorder;
use anyhow::{ensure, Context, Result};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    GetLastError, ERROR_CLASS_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE,
    WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetMonitorInfoW, MonitorFromRect,
    SelectObject, SetStretchBltMode, StretchBlt, AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HALFTONE, HDC, HGDIOBJ, MONITORINFO,
    MONITOR_DEFAULTTONEAREST, SRCCOPY,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VIRTUAL_KEY, VK_ESCAPE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetCursorPos, GetWindowLongPtrW,
    GetWindowRect, LoadCursorW, RegisterClassW, SetCursor, SetWindowLongPtrW, SetWindowPos,
    ShowWindow, UpdateLayeredWindow, CREATESTRUCTW, GWLP_USERDATA, HTBOTTOM, HTBOTTOMLEFT,
    HTBOTTOMRIGHT, HTCAPTION, HTCLIENT, HTLEFT, HTRIGHT, HTTOP, HTTOPLEFT, HTTOPRIGHT,
    HTTRANSPARENT, HWND_TOPMOST, IDC_SIZENESW, IDC_SIZENS, IDC_SIZENWSE, IDC_SIZEWE,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_SHOW, ULW_ALPHA, WMSZ_BOTTOM,
    WMSZ_BOTTOMLEFT, WMSZ_BOTTOMRIGHT, WMSZ_LEFT, WMSZ_RIGHT, WMSZ_TOP, WMSZ_TOPLEFT,
    WMSZ_TOPRIGHT, WM_CAPTURECHANGED, WM_CLOSE, WM_CREATE, WM_KEYDOWN, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCDESTROY, WM_NCHITTEST, WM_PAINT, WM_RBUTTONUP, WM_SETCURSOR,
    WM_SIZE, WNDCLASSW, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

const CLASS: PCWSTR = w!("dt-ocr.pin");
/// `WM_MOUSELEAVE` lives under UI::Controls in the windows crate.
const WM_MOUSELEAVE: u32 = 0x02A3;
/// Resize grip inside the image edge, in pixels.
const GRIP: i32 = 6;

/// Decoration margin: hatched shadow, one-pixel line, or none.
fn margin(border: PinBorder) -> i32 {
    match border {
        PinBorder::Hatch => 6,
        PinBorder::Line => 2,
        PinBorder::None => 0,
    }
}

struct Pin {
    mem: HDC,
    bmp: HGDIOBJ,
    old: HGDIOBJ,
    w: i32,
    h: i32,
    border: PinBorder,
    line_color: u32,
    margin: i32,
    drag: Option<ResizeDrag>,
    hover: u32,
    hover_pt: POINT,
    tracking: bool,
}

#[derive(Clone, Copy)]
struct ResizeDrag {
    side: u32,
    start: RECT,
    pointer: POINT,
}

/// Image size clamped into the available box, aspect preserved.
fn fit(src_w: i32, src_h: i32, max_w: i32, max_h: i32) -> (i32, i32) {
    if src_w <= max_w && src_h <= max_h {
        return (src_w, src_h);
    }
    let scale = (max_w as f64 / src_w as f64).min(max_h as f64 / src_h as f64);
    (
        ((src_w as f64 * scale) as i32).max(16).min(max_w.max(16)),
        ((src_h as f64 * scale) as i32).max(16).min(max_h.max(16)),
    )
}

/// Rewrites an image sizing rect so it keeps `ar` (width / height).
fn lock_aspect(side: u32, r: RECT, ar: f64) -> RECT {
    let mut out = r;
    let mut w = (r.right - r.left) as f64;
    let mut h = (r.bottom - r.top) as f64;
    if matches!(side, WMSZ_TOP | WMSZ_BOTTOM) {
        w = h * ar;
    } else {
        h = w / ar;
    }
    if w < 16.0 {
        w = 16.0;
        h = w / ar;
    }
    if h < 16.0 {
        h = 16.0;
        w = h * ar;
    }
    let (w, h) = (w.round() as i32, h.round() as i32);
    match side {
        WMSZ_LEFT | WMSZ_TOPLEFT | WMSZ_BOTTOMLEFT => out.left = out.right - w,
        _ => out.right = out.left + w,
    }
    match side {
        WMSZ_TOP | WMSZ_TOPLEFT | WMSZ_TOPRIGHT => out.top = out.bottom - h,
        _ => out.bottom = out.top + h,
    }
    out
}

/// Which resize edge the cursor is on, if any.
fn edge_at(r: RECT, pt: POINT) -> Option<u32> {
    let near = |a: i32, b: i32| (a - b).abs() <= GRIP;
    let left = near(pt.x, r.left);
    let right = near(pt.x, r.right);
    let top = near(pt.y, r.top);
    let bottom = near(pt.y, r.bottom);
    if r.right - r.left < 4 * GRIP || r.bottom - r.top < 4 * GRIP {
        return None;
    }
    Some(match (left, right, top, bottom) {
        (true, _, true, _) => HTTOPLEFT,
        (_, true, true, _) => HTTOPRIGHT,
        (true, _, _, true) => HTBOTTOMLEFT,
        (_, true, _, true) => HTBOTTOMRIGHT,
        (true, _, _, _) => HTLEFT,
        (_, true, _, _) => HTRIGHT,
        (_, _, true, _) => HTTOP,
        (_, _, _, true) => HTBOTTOM,
        _ => return None,
    })
}

fn sizing_side(hit: u32) -> u32 {
    match hit {
        HTLEFT => WMSZ_LEFT,
        HTRIGHT => WMSZ_RIGHT,
        HTTOP => WMSZ_TOP,
        HTBOTTOM => WMSZ_BOTTOM,
        HTTOPLEFT => WMSZ_TOPLEFT,
        HTTOPRIGHT => WMSZ_TOPRIGHT,
        HTBOTTOMLEFT => WMSZ_BOTTOMLEFT,
        HTBOTTOMRIGHT => WMSZ_BOTTOMRIGHT,
        _ => WMSZ_RIGHT,
    }
}

/// One margin pixel. `[0,0,0,0]` inside the image (the capture is painted
/// there later) and on empty hatch gaps. Hatch is cyan at ~43% alpha, with a
/// slightly stronger outline pixel next to the image so the edge stays legible.
fn band_pixel(
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    margin: i32,
    border: PinBorder,
    line_color: u32,
) -> [u8; 4] {
    let (ix0, iy0) = (margin, margin);
    let (ix1, iy1) = (w - margin - 1, h - margin - 1);
    if x >= ix0 && x <= ix1 && y >= iy0 && y <= iy1 {
        return [0, 0, 0, 0];
    }
    let dx = if x < ix0 {
        ix0 - x
    } else if x > ix1 {
        x - ix1
    } else {
        0
    };
    let dy = if y < iy0 {
        iy0 - y
    } else if y > iy1 {
        y - iy1
    } else {
        0
    };
    match border {
        PinBorder::None => [0, 0, 0, 0],
        PinBorder::Line if dx <= 1 && dy <= 1 => {
            let (r, g, b) = (
                (line_color >> 16) as u8,
                (line_color >> 8) as u8,
                line_color as u8,
            );
            [b, g, r, 255]
        }
        PinBorder::Line => [0, 0, 0, 0],
        PinBorder::Hatch if dx <= 1 && dy <= 1 => [0, 180, 180, 180],
        // 2 of every 5 diagonal pixels: visible at 6 px without turning solid.
        PinBorder::Hatch if (x + y).rem_euclid(5) < 2 => [0, 110, 110, 110],
        PinBorder::Hatch => [0, 0, 0, 0],
    }
}

/// Rounded cyan grip drawn over the image edge while the pointer is near it.
fn paint_grip(pixels: &mut [u8], w: i32, margin: i32, iw: i32, ih: i32, side: u32, local: POINT) {
    if iw < 24 || ih < 24 {
        return;
    }
    let clamp = |v: i32, lo: i32, hi: i32| v.clamp(lo, hi);
    let lx = clamp(local.x, 0, iw - 1);
    let ly = clamp(local.y, 0, ih - 1);
    let (cx, cy, rw, rh) = match side {
        HTLEFT => (3, clamp(ly, 9, ih - 10), 4, 18),
        HTRIGHT => (iw - 5, clamp(ly, 9, ih - 10), 4, 18),
        HTTOP => (clamp(lx, 9, iw - 10), 3, 18, 4),
        HTBOTTOM => (clamp(lx, 9, iw - 10), ih - 5, 18, 4),
        HTTOPLEFT => (4, 4, 8, 8),
        HTTOPRIGHT => (iw - 6, 4, 8, 8),
        HTBOTTOMLEFT => (4, ih - 6, 8, 8),
        HTBOTTOMRIGHT => (iw - 6, ih - 6, 8, 8),
        _ => return,
    };
    let (left, top) = (margin + cx - rw / 2, margin + cy - rh / 2);
    let (right, bottom) = (left + rw, top + rh);
    let radius = 2;
    for y in top..bottom {
        for x in left..right {
            let qx = if x < left + radius {
                left + radius - x
            } else if x > right - radius {
                x - (right - radius)
            } else {
                0
            };
            let qy = if y < top + radius {
                top + radius - y
            } else if y > bottom - radius {
                y - (bottom - radius)
            } else {
                0
            };
            if qx * qx + qy * qy <= radius * radius && x >= 0 && x < w {
                let i = ((y * w + x) * 4) as usize;
                pixels[i..i + 4].copy_from_slice(&[0, 190, 207, 230]);
            }
        }
    }
}

/// Shows `bgra` (w x h, top-down) at the capture position, clamped into the
/// work area. The window owns itself from here on.
pub unsafe fn show(
    rect: RECT,
    w: i32,
    h: i32,
    bgra: &[u8],
    border: PinBorder,
    line_color: u32,
) -> Result<()> {
    ensure!(w > 0 && h > 0, "nothing to pin");
    ensure!(
        bgra.len() >= (w as usize) * (h as usize) * 4,
        "short pixel buffer"
    );
    register()?;
    let instance: HINSTANCE = GetModuleHandleW(None)?.into();
    let mon = MonitorFromRect(&rect, MONITOR_DEFAULTTONEAREST);
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let work = if GetMonitorInfoW(mon, &mut info).as_bool() {
        info.rcWork
    } else {
        rect
    };
    let m = margin(border);
    let (win_w, win_h) = fit(
        w,
        h,
        work.right - work.left - 2 * m,
        work.bottom - work.top - 2 * m,
    );
    let x = rect
        .left
        .clamp(work.left, (work.right - win_w).max(work.left));
    let y = rect
        .top
        .clamp(work.top, (work.bottom - win_h).max(work.top));

    let mem = CreateCompatibleDC(None);
    ensure!(!mem.is_invalid(), "CreateCompatibleDC failed");
    let mut bi = BITMAPINFO::default();
    bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bi.bmiHeader.biWidth = w;
    bi.bmiHeader.biHeight = -h;
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 32;
    bi.bmiHeader.biCompression = BI_RGB.0;
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let dib = CreateDIBSection(Some(mem), &bi, DIB_RGB_COLORS, &mut bits, None, 0)
        .context("CreateDIBSection failed")?;
    if dib.is_invalid() || bits.is_null() {
        let _ = DeleteDC(mem);
        anyhow::bail!("CreateDIBSection returned null");
    }
    std::ptr::copy_nonoverlapping(bgra.as_ptr(), bits as *mut u8, bgra.len());
    let old = SelectObject(mem, dib.into());
    let pin = Box::into_raw(Box::new(Pin {
        mem,
        bmp: dib.into(),
        old,
        w,
        h,
        border,
        line_color,
        margin: m,
        drag: None,
        hover: 0,
        hover_pt: POINT::default(),
        tracking: false,
    }));
    let hwnd = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_LAYERED,
        CLASS,
        w!("dt-ocr.pin"),
        WS_POPUP,
        x - m,
        y - m,
        win_w + 2 * m,
        win_h + 2 * m,
        None,
        None,
        Some(instance),
        Some(pin as *const core::ffi::c_void),
    );
    let Ok(hwnd) = hwnd else {
        let pin = Box::from_raw(pin);
        let _ = SelectObject(pin.mem, pin.old);
        let _ = DeleteObject(pin.bmp);
        let _ = DeleteDC(pin.mem);
        return Err(windows::core::Error::from_win32()).context("cannot create the pinned window");
    };
    let _ = ShowWindow(hwnd, SW_SHOW);
    let _ = SetWindowPos(
        hwnd,
        Some(HWND_TOPMOST),
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
    );
    render(hwnd)?;
    crate::logline(&format!(
        "pin: {w}x{h} -> image {win_w}x{win_h} at ({x},{y})"
    ));
    Ok(())
}

/// Rebuilds the layered pixels after a resize or hover change. GDI scales the
/// image into the opaque centre; the margin carries the configured edge.
unsafe fn render(hwnd: HWND) -> Result<()> {
    let Some(raw) = pin_of(hwnd) else {
        return Ok(());
    };
    let pin = &*raw;
    let m = pin.margin;
    let mut client = RECT::default();
    GetClientRect(hwnd, &mut client)?;
    let (w, h) = (client.right, client.bottom);
    let (iw, ih) = (w - 2 * m, h - 2 * m);
    ensure!(iw > 0 && ih > 0, "pin window is too small");
    let mem = CreateCompatibleDC(None);
    ensure!(!mem.is_invalid(), "CreateCompatibleDC failed");
    let mut bi = BITMAPINFO::default();
    bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bi.bmiHeader.biWidth = w;
    bi.bmiHeader.biHeight = -h;
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 32;
    bi.bmiHeader.biCompression = BI_RGB.0;
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let dib = CreateDIBSection(Some(mem), &bi, DIB_RGB_COLORS, &mut bits, None, 0)?;
    ensure!(
        !dib.is_invalid() && !bits.is_null(),
        "CreateDIBSection returned null"
    );
    let pixels = std::slice::from_raw_parts_mut(bits as *mut u8, (w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let p = band_pixel(x, y, w, h, m, pin.border, pin.line_color);
            let i = ((y * w + x) * 4) as usize;
            pixels[i..i + 4].copy_from_slice(&p);
        }
    }
    let old = SelectObject(mem, dib.into());
    let _ = SetStretchBltMode(mem, HALFTONE);
    ensure!(
        StretchBlt(
            mem,
            m,
            m,
            iw,
            ih,
            Some(pin.mem),
            0,
            0,
            pin.w,
            pin.h,
            SRCCOPY,
        )
        .as_bool(),
        "StretchBlt failed"
    );
    for y in m..m + ih {
        for x in m..m + iw {
            pixels[((y * w + x) * 4 + 3) as usize] = 255;
        }
    }
    if pin.drag.is_none() && pin.hover != 0 {
        let mut wr = RECT::default();
        let _ = GetWindowRect(hwnd, &mut wr);
        let local = POINT {
            x: pin.hover_pt.x - wr.left - m,
            y: pin.hover_pt.y - wr.top - m,
        };
        paint_grip(&mut pixels[..], w, m, iw, ih, pin.hover, local);
    }
    let size = SIZE { cx: w, cy: h };
    let origin = POINT { x: 0, y: 0 };
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    UpdateLayeredWindow(
        hwnd,
        None,
        None,
        Some(&size),
        Some(mem),
        Some(&origin),
        windows::Win32::Foundation::COLORREF(0),
        Some(&blend),
        ULW_ALPHA,
    )?;
    let _ = SelectObject(mem, old);
    let _ = DeleteObject(dib.into());
    let _ = DeleteDC(mem);
    Ok(())
}

unsafe fn register() -> Result<()> {
    let instance: HINSTANCE = GetModuleHandleW(None)?.into();
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        hInstance: instance,
        lpszClassName: CLASS,
        ..Default::default()
    };
    ensure!(
        RegisterClassW(&wc) != 0 || GetLastError() == ERROR_CLASS_ALREADY_EXISTS,
        "RegisterClassW(pin) failed: {}",
        windows::core::Error::from_win32()
    );
    Ok(())
}

unsafe fn pin_of(hwnd: HWND) -> Option<*mut Pin> {
    let raw = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Pin;
    (!raw.is_null()).then_some(raw)
}

unsafe fn image_rect(hwnd: HWND) -> RECT {
    let mut r = RECT::default();
    let _ = GetWindowRect(hwnd, &mut r);
    let m = pin_of(hwnd).map(|p| (*p).margin).unwrap_or(0);
    r.left += m;
    r.top += m;
    r.right -= m;
    r.bottom -= m;
    r
}

fn resize_cursor(side: u32) -> PCWSTR {
    match side {
        HTLEFT | HTRIGHT => IDC_SIZEWE,
        HTTOP | HTBOTTOM => IDC_SIZENS,
        HTTOPLEFT | HTBOTTOMRIGHT => IDC_SIZENWSE,
        _ => IDC_SIZENESW,
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => {
            let cs = &*(lparam.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
            LRESULT(0)
        }
        // A layered window is painted by UpdateLayeredWindow, not WM_PAINT.
        WM_PAINT => LRESULT(0),
        WM_SIZE => {
            if let Err(e) = render(hwnd) {
                crate::logline(&format!("warn: pin render failed: {e:#}"));
            }
            LRESULT(0)
        }
        WM_NCHITTEST => {
            let x = (lparam.0 & 0xffff) as u16 as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xffff) as u16 as i16 as i32;
            let pt = POINT { x, y };
            let r = image_rect(hwnd);
            if edge_at(r, pt).is_some() {
                LRESULT(HTCLIENT as isize)
            } else if x >= r.left && x < r.right && y >= r.top && y < r.bottom {
                LRESULT(HTCAPTION as isize)
            } else {
                LRESULT(HTTRANSPARENT as isize)
            }
        }
        WM_LBUTTONDOWN => {
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            if let Some(hit) = edge_at(image_rect(hwnd), pt) {
                if let Some(raw) = pin_of(hwnd) {
                    (*raw).drag = Some(ResizeDrag {
                        side: sizing_side(hit),
                        start: image_rect(hwnd),
                        pointer: pt,
                    });
                    SetCapture(hwnd);
                    return LRESULT(0);
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_MOUSEMOVE => {
            let Some(raw) = pin_of(hwnd) else {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            };
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            if let Some(drag) = (*raw).drag {
                let (dx, dy) = (pt.x - drag.pointer.x, pt.y - drag.pointer.y);
                let mut r = drag.start;
                match drag.side {
                    WMSZ_LEFT | WMSZ_TOPLEFT | WMSZ_BOTTOMLEFT => r.left += dx,
                    _ => r.right += dx,
                }
                match drag.side {
                    WMSZ_TOP | WMSZ_TOPLEFT | WMSZ_TOPRIGHT => r.top += dy,
                    _ => r.bottom += dy,
                }
                let m = (*raw).margin;
                let out = lock_aspect(drag.side, r, (*raw).w as f64 / (*raw).h as f64);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    out.left - m,
                    out.top - m,
                    out.right - out.left + 2 * m,
                    out.bottom - out.top + 2 * m,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                return LRESULT(0);
            }
            let side = edge_at(image_rect(hwnd), pt).unwrap_or(0);
            let changed = side != (*raw).hover
                || (side != 0 && ((*raw).hover_pt.y - pt.y).abs() >= 2
                    || ((*raw).hover_pt.x - pt.x).abs() >= 2);
            if !(*raw).tracking && side != 0 {
                let mut track = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                (*raw).tracking = TrackMouseEvent(&mut track).is_ok();
            }
            if changed {
                (*raw).hover = side;
                (*raw).hover_pt = pt;
                if let Err(e) = render(hwnd) {
                    crate::logline(&format!("warn: pin hover render failed: {e:#}"));
                }
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            if let Some(raw) = pin_of(hwnd) {
                (*raw).tracking = false;
                if (*raw).hover != 0 {
                    (*raw).hover = 0;
                    if let Err(e) = render(hwnd) {
                        crate::logline(&format!("warn: pin hover render failed: {e:#}"));
                    }
                }
            }
            LRESULT(0)
        }
        WM_SETCURSOR => {
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            if let Some(side) = edge_at(image_rect(hwnd), pt) {
                if let Ok(cursor) = LoadCursorW(None, resize_cursor(side)) {
                    let _ = SetCursor(Some(cursor));
                    return LRESULT(1);
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_LBUTTONUP => {
            if let Some(raw) = pin_of(hwnd) {
                (*raw).drag = None;
                if let Err(e) = render(hwnd) {
                    crate::logline(&format!("warn: pin render failed: {e:#}"));
                }
            }
            let _ = ReleaseCapture();
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            if let Some(raw) = pin_of(hwnd) {
                (*raw).drag = None;
            }
            LRESULT(0)
        }
        WM_KEYDOWN if VIRTUAL_KEY(wparam.0 as u16) == VK_ESCAPE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_RBUTTONUP | WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_NCDESTROY => {
            if let Some(raw) = pin_of(hwnd) {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                let pin = Box::from_raw(raw);
                let _ = SelectObject(pin.mem, pin.old);
                let _ = DeleteObject(pin.bmp);
                let _ = DeleteDC(pin.mem);
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

#[cfg(test)]
mod tests {
    use super::{band_pixel, edge_at, fit, lock_aspect, margin, paint_grip, GRIP};
    use crate::config::PinBorder;
    use windows::Win32::Foundation::{POINT, RECT};
    use windows::Win32::UI::WindowsAndMessaging::{
        HTBOTTOMRIGHT, HTLEFT, HTRIGHT, WMSZ_BOTTOM, WMSZ_BOTTOMRIGHT, WMSZ_LEFT, WMSZ_RIGHT,
        WMSZ_TOP,
    };

    #[test]
    fn fit_keeps_small_images_untouched_and_shrinks_large_ones() {
        assert_eq!(fit(400, 260, 800, 600), (400, 260));
        assert_eq!(fit(1600, 800, 800, 800), (800, 400));
        assert_eq!(fit(600, 1200, 1000, 600), (300, 600));
        let (_, h) = fit(10000, 4, 800, 600);
        assert!(h >= 16);
    }

    #[test]
    fn lock_aspect_grows_and_shrinks_without_eating_the_image() {
        let r = RECT {
            left: 100,
            top: 100,
            right: 500,
            bottom: 300,
        }; // 2:1
        let out = lock_aspect(WMSZ_RIGHT, RECT { right: 600, ..r }, 2.0);
        assert_eq!((out.right - out.left, out.bottom - out.top), (500, 250));
        let out = lock_aspect(
            WMSZ_BOTTOMRIGHT,
            RECT {
                right: 400,
                bottom: 250,
                ..r
            },
            2.0,
        );
        assert_eq!((out.right - out.left, out.bottom - out.top), (300, 150));
        let out = lock_aspect(WMSZ_BOTTOM, r, 2.0);
        assert_eq!(out.right - out.left, 400);
        let out = lock_aspect(WMSZ_TOP, r, 2.0);
        assert_eq!(out.bottom, 300);
        let out = lock_aspect(
            WMSZ_LEFT,
            RECT {
                left: 300,
                top: 0,
                right: 500,
                bottom: 100,
            },
            2.0,
        );
        assert_eq!((out.left, out.right), (300, 500));
    }

    #[test]
    fn edge_hit_test_finds_edges_and_lets_small_windows_move() {
        let r = RECT {
            left: 0,
            top: 0,
            right: 400,
            bottom: 300,
        };
        assert_eq!(edge_at(r, POINT { x: 2, y: 150 }), Some(HTLEFT));
        assert_eq!(edge_at(r, POINT { x: 398, y: 298 }), Some(HTBOTTOMRIGHT));
        assert_eq!(edge_at(r, POINT { x: 200, y: 150 }), None);
        let tiny = RECT {
            left: 0,
            top: 0,
            right: 3 * GRIP,
            bottom: 3 * GRIP,
        };
        assert_eq!(edge_at(tiny, POINT { x: 1, y: 1 }), None);
    }

    #[test]
    fn pin_edge_modes_paint_hatch_line_or_nothing() {
        let m = margin(PinBorder::Hatch);
        let (w, h) = (2 * m + 100, 2 * m + 80);
        // The capture area is left clear for StretchBlt.
        assert_eq!(
            band_pixel(m + 10, m + 10, w, h, m, PinBorder::Hatch, 0),
            [0, 0, 0, 0]
        );
        // Strong outline next to the image, translucent cyan hatch outside it.
        assert_eq!(
            band_pixel(m - 1, m + 20, w, h, m, PinBorder::Hatch, 0),
            [0, 180, 180, 180]
        );
        assert_eq!(
            band_pixel(2, m + 19, w, h, m, PinBorder::Hatch, 0),
            [0, 0, 0, 0]
        );
        assert_eq!(
            band_pixel(1, m + 19, w, h, m, PinBorder::Hatch, 0),
            [0, 110, 110, 110]
        );
        // Line mode honours the configured colour at full opacity.
        let lm = margin(PinBorder::Line);
        assert_eq!(
            band_pixel(lm - 1, lm + 20, w, h, lm, PinBorder::Line, 0xff8800),
            [0x00, 0x88, 0xff, 255]
        );
        assert_eq!(
            band_pixel(0, lm + 20, w, h, lm, PinBorder::Line, 0),
            [0, 0, 0, 0]
        );
        assert_eq!(band_pixel(0, 0, w, h, 0, PinBorder::None, 0), [0, 0, 0, 0]);
    }

    #[test]
    fn hover_grip_paints_cyan_only_for_an_edge() {
        let (w, h, m, iw, ih) = (50, 50, 6, 38, 38);
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        paint_grip(&mut pixels, w, m, iw, ih, HTRIGHT, POINT { x: 37, y: 9 });
        let i = ((15 * w + 39) * 4) as usize;
        assert_eq!(&pixels[i..i + 4], &[0, 190, 207, 230]);
        let mut untouched = vec![0u8; (w * h * 4) as usize];
        paint_grip(&mut untouched, w, m, iw, ih, 0, POINT { x: 37, y: 9 });
        assert!(untouched.iter().all(|&b| b == 0));
    }
}
