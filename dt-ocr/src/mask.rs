//! P0.3 overlay mask: one topmost layered window over the frame region,
//! rendered per-pixel through `UpdateLayeredWindow` so the gaps show the live
//! screen while each OCR word gets an opaque black backing box that hides the
//! original glyphs. Click a word → clipboard gets that word; press-drag a
//! rectangle → clipboard gets the selected words joined into a paragraph.
//! Esc or right-click dismisses.
//!
//! Every pixel carries alpha ≥ 1 even where nothing is drawn: an alpha-0
//! layered pixel passes mouse input through, and the mask must own the whole
//! frame region (PLAN: click-through only *while* OCR runs).

use crate::capture::copy_text;
use crate::logline;
use crate::ocr::Word;
use crate::text::join_words;
use anyhow::{ensure, Context, Result};
use dt_common::layout::Rect;
use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, SetFocus, VIRTUAL_KEY, VK_ESCAPE,
};
use windows::Win32::UI::WindowsAndMessaging::*;

/// Black at alpha 1/255: invisible over any content, but hit-testable.
const BG_ALPHA: u32 = 0x0100_0000;
const BOX_ALPHA: u32 = 0xFF00_0000;
const TEXT_WHITE: u32 = 0x00FF_FFFF;
const SELECT_YELLOW: u32 = 0x0000D2FF; // RGB(255, 210, 0)
const TEXT_FACE: &str = "Microsoft YaHei"; // Latin + CJK in one face

struct MaskState {
    parent: HWND,
    /// Foreground before the mask took focus; restored on dismiss.
    prev_fg: HWND,
    words: Vec<Word>,
    /// Word rects are window-client coordinates == image pixels.
    rect: Rect,
    /// Baseline premultiplied BGRA (boxes + text) copied into `bits` before
    /// every selection repaint, so highlights never accumulate.
    base: Vec<u32>,
    /// 32 bpp top-down DIB owned by the mask.
    bits: *mut u32,
    memdc: HDC,
    bitmap: HBITMAP,
    old_bitmap: HGDIOBJ,
    /// Left-button drag origin (client coords) while selecting.
    drag: Option<POINT>,
    selected: Vec<usize>,
}

static MASK: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

fn mask_hwnd() -> HWND {
    HWND(MASK.load(std::sync::atomic::Ordering::Relaxed) as *mut core::ffi::c_void)
}

/// Closes the overlay (no-op when none is open) and returns the frame to its
/// normal interaction state.
pub unsafe fn dismiss() {
    let hwnd = mask_hwnd();
    if IsWindow(Some(hwnd)).as_bool() {
        let _ = DestroyWindow(hwnd);
    }
}

/// Builds and shows the mask for `words`, which are already in coordinates
/// relative to `rect` (the frame's screen rect).
pub unsafe fn show(parent: HWND, rect: Rect, words: Vec<Word>) -> Result<()> {
    dismiss();
    ensure!(rect.has_area(), "mask rect has no area");
    ensure!(!words.is_empty(), "mask has no words");

    let instance: HINSTANCE = GetModuleHandleW(None)?.into();
    let class = w!("dt-ocr.mask");
    let wc = WNDCLASSW {
        style: WNDCLASS_STYLES(0),
        lpfnWndProc: Some(wndproc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: HICON::default(),
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        hbrBackground: HBRUSH::default(),
        lpszMenuName: windows::core::PCWSTR::null(),
        lpszClassName: windows::core::PCWSTR(class.as_ptr()),
    };
    let atom = RegisterClassW(&wc);
    if atom == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
        anyhow::bail!(
            "RegisterClassW(mask) failed: {}",
            windows::core::Error::from_win32()
        );
    }

    let (w, h) = (rect.w(), rect.h());
    let screen = GetDC(None);
    ensure!(!screen.0.is_null(), "GetDC(screen) failed");
    let memdc = CreateCompatibleDC(Some(screen));
    let mut info = BITMAPINFO::default();
    info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    info.bmiHeader.biWidth = w;
    info.bmiHeader.biHeight = -h; // top-down: image row 0 is the window's top
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    info.bmiHeader.biCompression = BI_RGB.0;
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let bitmap = CreateDIBSection(Some(screen), &info, DIB_RGB_COLORS, &mut bits, None, 0)
        .context("CreateDIBSection failed")?;
    let _ = ReleaseDC(None, screen);
    ensure!(!bits.is_null(), "CreateDIBSection returned null bits");
    let old_bitmap = SelectObject(memdc, bitmap.into());
    let bits = bits as *mut u32;
    let len = (w * h) as usize;
    std::ptr::write_bytes(bits as *mut u8, 0, len * 4);
    draw_base(memdc, bits, w, h, &words);

    let prev_fg = GetForegroundWindow();
    let hwnd = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
        class,
        windows::core::PCWSTR::null(),
        WS_POPUP,
        rect.l,
        rect.t,
        w,
        h,
        Some(parent),
        None,
        Some(instance),
        None,
    )?;
    SetWindowLongPtrW(
        hwnd,
        GWLP_USERDATA,
        Box::into_raw(Box::new(MaskState {
            parent,
            prev_fg,
            words,
            rect,
            base: std::slice::from_raw_parts(bits, len).to_vec(),
            bits,
            memdc,
            bitmap,
            old_bitmap,
            drag: None,
            selected: Vec::new(),
        })) as isize,
    );
    MASK.store(hwnd.0 as isize, std::sync::atomic::Ordering::Relaxed);
    present(hwnd);
    let _ = ShowWindow(hwnd, SW_SHOW);
    let _ = SetForegroundWindow(hwnd);
    let _ = SetFocus(Some(hwnd));
    // The first word + rect is the cheapest way to see, from the log alone,
    // what the engine found and where a click test can aim.
    let s = state(hwnd);
    logline(&format!(
        "mask: {} words over {w}x{h} at ({},{}), first `{}` {:?}",
        s.words.len(),
        rect.l,
        rect.t,
        s.words[0].text,
        s.words[0].rect
    ));
    Ok(())
}

unsafe fn state(hwnd: HWND) -> &'static mut MaskState {
    &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut MaskState)
}

/// Draws every word box + white text once, then resolves the alpha channel:
/// boxes and glyph pixels opaque, everything else the invisible-but-solid
/// background alpha.
unsafe fn draw_base(hdc: HDC, bits: *mut u32, w: i32, h: i32, words: &[Word]) {
    let _ = SetBkMode(hdc, TRANSPARENT);
    let _ = SetTextColor(hdc, COLORREF(TEXT_WHITE));
    let black = CreateSolidBrush(COLORREF(0));
    let face = wide(TEXT_FACE);
    let mut fonts: Vec<(i32, HFONT)> = Vec::new();
    for word in words {
        let rc = clip_rect(word.rect, w, h);
        if rc.w() < 2 || rc.h() < 2 {
            continue;
        }
        let win = to_win(rc);
        let _ = FillRect(hdc, &win, black);
        // Opaque box first; the text pass may rewrite alpha on glyph pixels,
        // the RGB pass below restores those.
        for y in rc.t..rc.b {
            for x in rc.l..rc.r {
                *bits.add((y * w + x) as usize) |= BOX_ALPHA;
            }
        }
        let em = (rc.h() * 4 / 5).clamp(8, 64);
        let font = match fonts.iter().find(|(size, _)| *size == em) {
            Some((_, f)) => *f,
            None => {
                let f = CreateFontW(
                    -em,
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
                fonts.push((em, f));
                f
            }
        };
        let old = SelectObject(hdc, font.into());
        let mut text: Vec<u16> = word.text.encode_utf16().collect();
        let mut text_rc = to_win(rc);
        text_rc.left += 1;
        let _ = DrawTextW(
            hdc,
            &mut text,
            &mut text_rc,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
        );
        let _ = SelectObject(hdc, old);
    }
    let _ = DeleteObject(black.into());
    for (_, font) in fonts {
        let _ = DeleteObject(font.into());
    }
    for i in 0..(w * h) as usize {
        let p = *bits.add(i);
        *bits.add(i) = if p & 0x00FF_FFFF != 0 {
            p | BOX_ALPHA
        } else if p >> 24 == 0 {
            BG_ALPHA
        } else {
            p
        };
    }
}

/// Repaints the layered surface: baseline + selection outlines.
unsafe fn present(hwnd: HWND) {
    let s = state(hwnd);
    let len = (s.rect.w() * s.rect.h()) as usize;
    std::ptr::copy_nonoverlapping(s.base.as_ptr(), s.bits, len);
    let brush = CreateSolidBrush(COLORREF(SELECT_YELLOW));
    for &index in &s.selected {
        let rc = to_win(s.words[index].rect);
        let _ = FrameRect(s.memdc, &rc, brush);
    }
    let _ = DeleteObject(brush.into());
    // GDI writes the reserved byte as 0; a zero alpha hides the outline.
    for i in 0..len {
        let p = *s.bits.add(i);
        if p & 0x00FF_FFFF != 0 && p >> 24 == 0 {
            *s.bits.add(i) = p | BOX_ALPHA;
        }
    }
    let dst = POINT {
        x: s.rect.l,
        y: s.rect.t,
    };
    let size = SIZE {
        cx: s.rect.w(),
        cy: s.rect.h(),
    };
    let src = POINT::default();
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    let _ = UpdateLayeredWindow(
        hwnd,
        None,
        Some(&dst),
        Some(&size),
        Some(s.memdc),
        Some(&src),
        COLORREF(0),
        Some(&blend),
        ULW_ALPHA,
    );
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn to_win(r: Rect) -> RECT {
    RECT {
        left: r.l,
        top: r.t,
        right: r.r,
        bottom: r.b,
    }
}

fn clip_rect(r: Rect, w: i32, h: i32) -> Rect {
    Rect {
        l: r.l.clamp(0, w),
        t: r.t.clamp(0, h),
        r: r.r.clamp(0, w),
        b: r.b.clamp(0, h),
    }
}

fn intersects(a: Rect, b: Rect) -> bool {
    a.l < b.r && b.l < a.r && a.t < b.b && b.t < a.b
}

/// Smallest word containing the point (OCR boxes of adjacent lines may
/// overlap by a pixel).
fn word_at(words: &[Word], pt: POINT) -> Option<usize> {
    words
        .iter()
        .enumerate()
        .filter(|(_, w)| pt.x >= w.rect.l && pt.x < w.rect.r && pt.y >= w.rect.t && pt.y < w.rect.b)
        .min_by_key(|(_, w)| w.rect.w() * w.rect.h())
        .map(|(i, _)| i)
}

fn drag_rect(start: POINT, end: POINT) -> Rect {
    Rect {
        l: start.x.min(end.x),
        t: start.y.min(end.y),
        r: start.x.max(end.x),
        b: start.y.max(end.y),
    }
}

unsafe fn finish_selection(hwnd: HWND, start: POINT, end: POINT) {
    let s = state(hwnd);
    let rect = drag_rect(start, end);
    let picked: Vec<usize> = if rect.w() < 4 && rect.h() < 4 {
        word_at(&s.words, end).into_iter().collect()
    } else {
        s.words
            .iter()
            .enumerate()
            .filter(|(_, w)| intersects(w.rect, rect))
            .map(|(i, _)| i)
            .collect()
    };
    if picked.is_empty() {
        s.selected.clear();
        present(hwnd);
        return;
    }
    let text = if picked.len() == 1 {
        s.words[picked[0]].text.clone()
    } else {
        join_words(picked.iter().map(|&i| s.words[i].text.as_str()))
    };
    s.selected = picked;
    present(hwnd);
    match copy_text(hwnd, &text) {
        Ok(()) => logline(&format!(
            "mask: copied {} chars: {}",
            text.chars().count(),
            preview(&text)
        )),
        Err(e) => logline(&format!("mask: clipboard failed: {e:#}")),
    }
}

fn preview(text: &str) -> String {
    let flat = text.replace(['\r', '\n'], " ");
    if flat.chars().count() > 60 {
        format!("{}…", flat.chars().take(60).collect::<String>())
    } else {
        flat
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let point = || POINT {
        x: (lparam.0 & 0xFFFF) as u16 as i32,
        y: ((lparam.0 >> 16) & 0xFFFF) as u16 as i32,
    };
    match msg {
        WM_LBUTTONDOWN => {
            let s = state(hwnd);
            s.drag = Some(point());
            s.selected.clear();
            let _ = SetCapture(hwnd);
            present(hwnd);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let Some(start) = state(hwnd).drag else {
                return LRESULT(0);
            };
            let rect = drag_rect(start, point());
            let picked: Vec<usize> = state(hwnd)
                .words
                .iter()
                .enumerate()
                .filter(|(_, w)| intersects(w.rect, rect))
                .map(|(i, _)| i)
                .collect();
            state(hwnd).selected = picked;
            present(hwnd);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let start = state(hwnd).drag.take();
            let _ = ReleaseCapture();
            if let Some(start) = start {
                finish_selection(hwnd, start, point());
            }
            LRESULT(0)
        }
        // Right-click closes on button-up: destroying on button-down would
        // spill the release into the app underneath.
        WM_RBUTTONDOWN => {
            let _ = SetCapture(hwnd);
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            let _ = ReleaseCapture();
            dismiss();
            LRESULT(0)
        }
        WM_KEYDOWN if VIRTUAL_KEY(wparam.0 as u16) == VK_ESCAPE => {
            dismiss();
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_ACTIVATE as isize),
        WM_DESTROY => {
            let s = Box::from_raw(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut MaskState);
            let _ = SelectObject(s.memdc, s.old_bitmap);
            let _ = DeleteObject(s.bitmap.into());
            let _ = DeleteDC(s.memdc);
            MASK.store(0, std::sync::atomic::Ordering::Relaxed);
            if IsWindow(Some(s.parent)).as_bool() {
                crate::frame::set_ocr_running(s.parent, false);
            }
            if IsWindow(Some(s.prev_fg)).as_bool() {
                let _ = SetForegroundWindow(s.prev_fg);
            }
            logline("mask: dismissed");
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, l: i32, t: i32, r: i32, b: i32) -> Word {
        Word {
            text: text.to_string(),
            rect: Rect { l, t, r, b },
        }
    }

    #[test]
    fn point_picks_the_smallest_containing_word() {
        let words = [
            word("big", 0, 0, 100, 100),
            word("small", 10, 10, 30, 30),
            word("elsewhere", 200, 0, 300, 100),
        ];
        assert_eq!(word_at(&words, POINT { x: 20, y: 20 }), Some(1));
        assert_eq!(word_at(&words, POINT { x: 50, y: 50 }), Some(0));
        assert_eq!(word_at(&words, POINT { x: 150, y: 50 }), None);
    }

    #[test]
    fn rect_selection_is_by_intersection() {
        let w = word("a", 10, 10, 20, 20);
        assert!(intersects(
            w.rect,
            Rect {
                l: 0,
                t: 0,
                r: 15,
                b: 15
            }
        ));
        assert!(!intersects(
            w.rect,
            Rect {
                l: 0,
                t: 0,
                r: 10,
                b: 10
            }
        ));
        assert!(!intersects(
            w.rect,
            Rect {
                l: 20,
                t: 20,
                r: 30,
                b: 30
            }
        ));
    }

    #[test]
    fn drag_rect_is_direction_independent() {
        let a = POINT { x: 30, y: 40 };
        let b = POINT { x: 10, y: 5 };
        assert_eq!(
            drag_rect(a, b),
            Rect {
                l: 10,
                t: 5,
                r: 30,
                b: 40
            }
        );
        assert_eq!(
            drag_rect(a, a),
            Rect {
                l: 30,
                t: 40,
                r: 30,
                b: 40
            }
        );
    }
}
