//! P0.3/P0.7 overlay mask: one topmost layered window over the frame region,
//! rendered per-pixel through `UpdateLayeredWindow`. Untouched pixels come
//! from the native-resolution snapshot captured with the OCR image, so screen
//! changes cannot misalign the mask. `box` leaves source pixels untouched,
//! `box+mask` replaces text with inverse-colour bands, and `box+outside` adds
//! a preview bar outside the frame. Click copies a word; drag copies a
//! selection; Esc dismisses.
//!
//! Every pixel is opaque: transparent layered pixels pass mouse input through,
//! while the mask must own the frame region after OCR.

use crate::capture::copy_text;
use crate::config::{MaskFill, MaskMode, MaskSize};
use crate::logline;
use crate::ocr::{Snapshot, Word};
use crate::text::join_words;
use anyhow::{ensure, Context, Result};
use dt_common::layout::Rect;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, SetFocus, VIRTUAL_KEY, VK_CONTROL, VK_ESCAPE,
};
use windows::Win32::UI::WindowsAndMessaging::*;

const BOX_ALPHA: u32 = 0xFF00_0000;
const SELECT_ORANGE: u32 = 0x0000A5FF; // RGB(255, 165, 0)
const RESULT_GREEN: u32 = 0xFF32_CD32; // LimeGreen #32CD32, opaque
const DEFAULT_FACE: &str = "Sarasa Term SC Nerd";

/// `[mask]` style resolved when a mask opens: font face, size and mode.
#[derive(Clone)]
struct Style {
    font: String,
    size: MaskSize,
    mode: MaskMode,
}

static STYLE: Mutex<Style> = Mutex::new(Style {
    font: String::new(),
    size: MaskSize::Auto,
    mode: MaskMode::Box,
});

/// Live config (`main` calls this at startup and on every hot-reload).
pub fn set_style(font: String, size: MaskSize, mode: MaskMode) {
    if let Ok(mut s) = STYLE.lock() {
        *s = Style { font, size, mode };
    }
}

fn style() -> Style {
    STYLE.lock().map(|s| s.clone()).unwrap_or(Style {
        font: DEFAULT_FACE.to_string(),
        size: MaskSize::Pt(10),
        mode: MaskMode::Box,
    })
}

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
    /// Per-word selection outline: orange for source `.box`, inverse text
    /// colour for `box+mask`.
    outlines: Vec<u32>,
    /// The external preview bar (`box+outside`); null otherwise.
    page: HWND,
}

/// Style resolved for one draw pass (keeps `draw_base` under the clippy arg
/// limit).
struct Base<'a> {
    fixed_em: Option<i32>,
    mode: MaskMode,
    face: &'a [u16],
}

static MASK: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// `[mask] fill = "lines"`: continuous band per line segment. Off = boxes.
static FILL_LINES: AtomicBool = AtomicBool::new(true);

/// Live config (`main` calls this at startup and on every hot-reload).
pub fn set_fill(fill: MaskFill) {
    FILL_LINES.store(fill == MaskFill::Lines, Ordering::Relaxed);
}

fn fill_lines() -> bool {
    FILL_LINES.load(Ordering::Relaxed)
}

/// Show/hide the open mask without re-running OCR (bound to `toggle`).
pub unsafe fn toggle() {
    let hwnd = mask_hwnd();
    if !IsWindow(Some(hwnd)).as_bool() {
        logline("mask: toggle ignored (no mask)");
        return;
    }
    if IsWindowVisible(hwnd).as_bool() {
        let _ = ShowWindow(hwnd, SW_HIDE);
        let page = state(hwnd).page;
        if IsWindow(Some(page)).as_bool() {
            let _ = ShowWindow(page, SW_HIDE);
        }
        crate::frame::set_result(state(hwnd).parent, false);
        logline("mask: hidden");
    } else {
        present(hwnd);
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
        );
        let page = state(hwnd).page;
        if IsWindow(Some(page)).as_bool() {
            let _ = ShowWindow(page, SW_SHOWNOACTIVATE);
            let _ = SetForegroundWindow(page);
            let _ = SetFocus(Some(state_userdata(page).edit));
        } else {
            let _ = SetForegroundWindow(hwnd);
            let _ = SetFocus(Some(hwnd));
        }
        crate::frame::set_result(state(hwnd).parent, true);
        logline("mask: shown");
    }
}

fn mask_hwnd() -> HWND {
    HWND(MASK.load(std::sync::atomic::Ordering::Relaxed) as *mut core::ffi::c_void)
}

/// Closes the overlay (no-op when none is open) and returns the frame to its
/// normal interaction state.
pub unsafe fn dismiss() {
    let hwnd = mask_hwnd();
    if IsWindow(Some(hwnd)).as_bool() {
        let parent = state(hwnd).parent;
        let _ = DestroyWindow(hwnd);
        if IsWindow(Some(parent)).as_bool() {
            crate::frame::set_result(parent, false);
        }
    }
}

/// Mask Ctrl+C: copy the whole result, then leave the result view. With auto
/// OCR armed the session stays armed but paused and the frame returns to Move;
/// otherwise the frame is put away.
unsafe fn copy_all_and_close(hwnd: HWND) {
    let (text, parent) = {
        let s = state(hwnd);
        (all_text(&s.words), s.parent)
    };
    // Read the session state before dismiss: destroying the mask must not
    // change whether C-c pauses an armed auto session or closes the frame.
    let auto = crate::ocr::auto_enabled();
    match crate::capture::copy_text(hwnd, &text) {
        Ok(()) => {
            logline(&format!(
                "mask: Ctrl+C copied {} chars: {}",
                text.chars().count(),
                preview(&text)
            ));
            dismiss();
            if auto {
                crate::frame::finish_auto_copy(parent);
            } else {
                crate::frame::hide(parent);
            }
        }
        Err(e) => logline(&format!("mask: Ctrl+C copy failed: {e:#}")),
    }
}

/// Builds and shows the mask for `words`, which are already in coordinates
/// relative to `rect` (the frame's screen rect).
pub unsafe fn show(parent: HWND, rect: Rect, words: Vec<Word>, snapshot: Snapshot) -> Result<()> {
    dismiss();
    ensure!(rect.has_area(), "mask rect has no area");
    ensure!(!words.is_empty(), "mask has no words");
    ensure!(
        snapshot.width == rect.w() && snapshot.height == rect.h(),
        "snapshot dimensions do not match mask"
    );
    ensure!(
        snapshot.bgra.len() == (rect.w() as usize) * (rect.h() as usize) * 4,
        "snapshot buffer has the wrong size"
    );

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
    std::ptr::copy_nonoverlapping(snapshot.bgra.as_ptr(), bits as *mut u8, len * 4);
    for i in 0..len {
        *bits.add(i) |= BOX_ALPHA;
    }
    let style = style();
    let dpi = match GetDpiForWindow(parent) {
        0 => 96,
        d => d,
    };
    let fixed_em = match style.size {
        MaskSize::Auto => None,
        MaskSize::Pt(pt) => Some(((pt as i32 * dpi as i32 + 36) / 72).max(6)),
    };
    let outside = if style.mode == MaskMode::BoxOutside {
        Some(panel_text(&words))
    } else {
        None
    };
    let face_w = wide(&style.font);
    let outlines = draw_base(
        memdc,
        bits,
        w,
        h,
        &words,
        &Base {
            fixed_em,
            mode: style.mode,
            face: &face_w,
        },
    );
    paint_result_border(bits, w, h);

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
            outlines,
            page: HWND::default(),
        })) as isize,
    );
    MASK.store(hwnd.0 as isize, std::sync::atomic::Ordering::Relaxed);
    // Auto-refresh samples the source without hiding this overlay. Windows
    // omits the mask from the capture and exposes the desktop underneath.
    let excluded = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE).is_ok();
    if !excluded {
        logline("warn: mask cannot be excluded from capture on this Windows build");
    }
    present(hwnd);
    let _ = ShowWindow(hwnd, SW_SHOW);
    let mut page = HWND::default();
    if let Some(text) = outside.as_deref() {
        match create_page(parent, rect, text) {
            Ok(created) => {
                page = created;
                state(hwnd).page = created;
            }
            Err(e) => logline(&format!("warn: outside editor failed: {e:#}")),
        }
    }
    if IsWindow(Some(page)).as_bool() {
        // Proofreading mode: the OCR text is preloaded and the editor owns
        // the keyboard (Esc is handled by its subclass).
        let _ = SetForegroundWindow(page);
        let _ = SetFocus(Some(state_userdata(page).edit));
    } else {
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
    }
    let _ = SetForegroundWindow(hwnd);
    let _ = SetFocus(Some(hwnd));
    crate::frame::set_result(parent, true);
    // The first word + rect is the cheapest way to see, from the log alone,
    // what the engine found and where a click test can aim.
    let s = state(hwnd);
    let mode = match style.mode {
        MaskMode::Box => "box",
        MaskMode::BoxMask if fill_lines() => "box+mask:lines",
        MaskMode::BoxMask => "box+mask:boxes",
        MaskMode::BoxOutside => "box+outside",
    };
    let font_info = match style.mode {
        MaskMode::Box => "source pixels".to_string(),
        MaskMode::BoxMask => format!(
            "{}+{}px",
            style.font,
            fixed_em
                .map(|e| e.to_string())
                .unwrap_or_else(|| "auto".into())
        ),
        MaskMode::BoxOutside => format!("{} 12px editor", style.font),
    };
    logline(&format!(
        "mask: {mode}, {} words, {font_info} over {w}x{h} at ({},{}), first `{}` {:?}",
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

/// Black bands for the continuous fill: within one OCR line, words closer
/// than 3/4 em are one run; bigger gaps (tabs, columns) stay separate.
fn line_segments(words: &[Word]) -> Vec<Rect> {
    let mut out = Vec::new();
    let mut start = 0;
    while start < words.len() {
        let line = words[start].line;
        let mut end = start;
        while end < words.len() && words[end].line == line {
            end += 1;
        }
        let mut run: Option<Rect> = None;
        for w in &words[start..end] {
            match run {
                None => run = Some(w.rect),
                Some(mut r) => {
                    let h = (w.rect.b - w.rect.t).min(r.h()).max(1);
                    if w.rect.l - r.r <= h * 3 / 4 {
                        r.t = r.t.min(w.rect.t);
                        r.b = r.b.max(w.rect.b);
                        r.r = r.r.max(w.rect.r);
                        run = Some(r);
                    } else {
                        out.push(r);
                        run = Some(w.rect);
                    }
                }
            }
        }
        if let Some(r) = run {
            out.push(r);
        }
        start = end;
    }
    out
}

/// Whole result in reading order, used by quick OCR's clipboard copy. Wrapped
/// OCR lines are joined back into their paragraph, with a blank line between
/// paragraphs, matching what a direct screen selection copies.
pub fn all_text(words: &[Word]) -> String {
    panel_text(words)
}

/// Text for a selection in reading order: `join_words` rules inside a line
/// (space between Latin words, none inside CJK) and `\n` between lines.
fn selection_text(words: &[Word], mut picked: Vec<usize>) -> String {
    picked.sort_by_key(|&i| (words[i].line, words[i].rect.l));
    let mut lines: Vec<(u32, Vec<&str>)> = Vec::new();
    for i in picked {
        let w = &words[i];
        match lines.last_mut() {
            Some((line, parts)) if *line == w.line => parts.push(w.text.as_str()),
            _ => lines.push((w.line, vec![w.text.as_str()])),
        }
    }
    lines
        .iter()
        .map(|(_, parts)| join_words(parts.iter().copied()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Preview paragraphs for `box+outside`: OCR lines are joined into one flow
/// (spaces via `join_words`) and separated by a blank line when the vertical gap
/// is at least one line height.
fn panel_text(words: &[Word]) -> String {
    let mut lines: Vec<(i32, i32, Vec<&Word>)> = Vec::new();
    for w in words {
        match lines.last_mut() {
            Some((t, b, ws)) if ws[0].line == w.line => {
                *t = (*t).min(w.rect.t);
                *b = (*b).max(w.rect.b);
                ws.push(w);
            }
            _ => lines.push((w.rect.t, w.rect.b, vec![w])),
        }
    }
    let mut paras: Vec<Vec<&Word>> = Vec::new();
    let mut prev: Option<(i32, i32)> = None;
    for (t, b, ws) in lines {
        let height = (b - t).max(1);
        let new_para = match prev {
            Some((pt, pb)) => t - pb > height.min((pb - pt).max(1)),
            None => true,
        };
        if new_para {
            paras.push(Vec::new());
        }
        paras.last_mut().unwrap().extend(ws);
        prev = Some((t, b));
    }
    paras
        .iter()
        .map(|ws| join_words(ws.iter().map(|w| w.text.as_str())))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Em size (px) per OCR line from the line's median word height: auto mode
/// keeps a line uniform instead of sizing every word on its own box.
fn line_ems(words: &[Word]) -> Vec<(u32, i32)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let line = words[i].line;
        let start = i;
        while i < words.len() && words[i].line == line {
            i += 1;
        }
        let mut heights: Vec<i32> = words[start..i].iter().map(|w| w.rect.h()).collect();
        heights.sort_unstable();
        let median = heights[heights.len() / 2].max(1);
        out.push((line, (median * 4 / 5).clamp(6, 96)));
    }
    out
}

fn line_em(ems: &[(u32, i32)], line: u32) -> i32 {
    ems.iter()
        .find(|(l, _)| *l == line)
        .map(|(_, em)| *em)
        .unwrap_or(13)
}

unsafe fn font_for(face: &[u16], em: i32, fonts: &mut Vec<(i32, HFONT)>) -> HFONT {
    if let Some((_, font)) = fonts.iter().find(|(size, _)| *size == em) {
        return *font;
    }
    let font = CreateFontW(
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
    fonts.push((em, font));
    font
}

/// Draws the mask base once. Plain `box` and `box+outside` leave the source
/// pixels untouched; `box+mask` replaces each line/word with its inverse-colour
/// band and text. Every pixel is made opaque so the frozen base stays visible.
unsafe fn draw_base(
    hdc: HDC,
    bits: *mut u32,
    w: i32,
    h: i32,
    words: &[Word],
    base: &Base,
) -> Vec<u32> {
    let len = (w * h) as usize;
    let mut outlines = vec![SELECT_ORANGE; words.len()];
    if base.mode != MaskMode::BoxMask {
        for i in 0..len {
            *bits.add(i) |= BOX_ALPHA;
        }
        return outlines;
    }
    let _ = SetBkMode(hdc, TRANSPARENT);
    let mut fonts: Vec<(i32, HFONT)> = Vec::new();
    let ems = line_ems(words);
    // Sample every colour before any band is overwritten, or the inverse pass
    // would read our own painted blocks instead of the source.
    let word_colors: Vec<(u32, u32)> = words
        .iter()
        .map(|word| inverse_colors(bits, w, clip_rect(word.rect, w, h)))
        .collect();
    if fill_lines() {
        for seg in line_segments(words) {
            let rc = clip_rect(seg, w, h);
            if rc.w() < 2 || rc.h() < 2 {
                continue;
            }
            let (block, _) = inverse_colors(bits, w, rc);
            let brush = CreateSolidBrush(COLORREF(block));
            let _ = FillRect(hdc, &to_win(rc), brush);
            let _ = DeleteObject(brush.into());
        }
    }
    for (index, word) in words.iter().enumerate() {
        let rc = clip_rect(word.rect, w, h);
        if rc.w() < 2 || rc.h() < 2 {
            continue;
        }
        let (block, text_color) = word_colors[index];
        outlines[index] = text_color;
        if !fill_lines() {
            let brush = CreateSolidBrush(COLORREF(block));
            let _ = FillRect(hdc, &to_win(rc), brush);
            let _ = DeleteObject(brush.into());
        }
        let em = base.fixed_em.unwrap_or_else(|| line_em(&ems, word.line));
        let font = font_for(base.face, em, &mut fonts);
        let old = SelectObject(hdc, font.into());
        let _ = SetTextColor(hdc, COLORREF(text_color));
        let mut text: Vec<u16> = word.text.encode_utf16().collect();
        let mut text_rc = to_win(rc);
        text_rc.left += 1;
        let _ = DrawTextW(
            hdc,
            &mut text,
            &mut text_rc,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        let _ = SelectObject(hdc, old);
    }
    for (_, font) in fonts {
        let _ = DeleteObject(font.into());
    }
    for i in 0..len {
        *bits.add(i) |= BOX_ALPHA;
    }
    outlines
}

/// Dominant source colour inside `rc` and its inverse. The dominant colour is
/// the background, so inverting it and drawing the inverse again as text keeps
/// dark and light terminals equally readable.
fn inverse_colors(bits: *mut u32, w: i32, rc: Rect) -> (u32, u32) {
    let mut counts = [0u32; 16];
    let mut sums = [[0u64; 3]; 16];
    for y in rc.t..rc.b {
        for x in rc.l..rc.r {
            let p = unsafe { *bits.add((y * w + x) as usize) };
            let b = (p >> 16) & 0xff;
            let g = (p >> 8) & 0xff;
            let r = p & 0xff;
            let luma = (r * 2 + g * 5 + b) / 8;
            let bin = (luma / 16) as usize;
            counts[bin] += 1;
            sums[bin][0] += r as u64;
            sums[bin][1] += g as u64;
            sums[bin][2] += b as u64;
        }
    }
    let bin = counts
        .iter()
        .enumerate()
        .max_by_key(|(_, count)| **count)
        .map(|(i, _)| i)
        .unwrap_or(0);
    let n = counts[bin].max(1) as u64;
    let r = (sums[bin][0] / n) as u32 & 0xff;
    let g = (sums[bin][1] / n) as u32 & 0xff;
    let b = (sums[bin][2] / n) as u32 & 0xff;
    let block = (!r & 0xff) | ((!g & 0xff) << 8) | ((!b & 0xff) << 16);
    let text = (!block) & 0x00ff_ffff;
    (block, text)
}

/// The mask covers the frame rect, so its 2 px edge carries the result colour:
/// the frame reads as continuously green for as long as a result is open.
fn paint_result_border(bits: *mut u32, w: i32, h: i32) {
    let t = 2.min(w).min(h);
    for y in 0..h {
        for x in 0..w {
            if x < t || y < t || x >= w - t || y >= h - t {
                unsafe {
                    *bits.add((y * w + x) as usize) = RESULT_GREEN;
                }
            }
        }
    }
}

/// Repaints the layered surface: baseline + selection outlines.
unsafe fn present(hwnd: HWND) {
    let s = state(hwnd);
    let len = (s.rect.w() * s.rect.h()) as usize;
    std::ptr::copy_nonoverlapping(s.base.as_ptr(), s.bits, len);
    for &index in &s.selected {
        let rc = to_win(s.words[index].rect);
        let brush = CreateSolidBrush(COLORREF(
            s.outlines.get(index).copied().unwrap_or(SELECT_ORANGE),
        ));
        let _ = FrameRect(s.memdc, &rc, brush);
        let _ = DeleteObject(brush.into());
    }
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

/// Multiline `EDIT` treats only CRLF as a line break; a bare LF leaves the
/// whole result on one visual line.
fn edit_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\n', "\r\n")
}

/// Preferred outside span: below the frame the editor grows with the OCR
/// text, capped at 2/3 of the work area; beside it a readable fixed width.
fn page_span(frame: Rect, work: Rect, text: &str) -> i32 {
    if frame.w() >= frame.h() {
        let lines = text.lines().count().max(1) as i32;
        (lines * 18 + 20).clamp(64, (work.h() * 2 / 3).max(64))
    } else {
        (frame.w() * 3 / 2)
            .clamp(220, 480)
            .min((work.w() * 2 / 3).max(120))
    }
}

/// Outside editor placement. Landscape goes BELOW the frame, portrait to its
/// RIGHT. When that side is short (full-screen OCR), the topmost bar floats
/// over the frame and is shifted up/left just enough to stay on the monitor.
fn outside_rect(frame: Rect, work: Rect, span: i32) -> Rect {
    if frame.w() >= frame.h() {
        let height = span.clamp(1, work.h().max(1));
        let below = work.b - frame.b;
        let top = if below >= height {
            frame.b
        } else {
            (work.b - height).max(work.t)
        };
        Rect {
            l: frame.l,
            t: top,
            r: frame.r,
            b: top + height,
        }
    } else {
        let width = span.clamp(1, work.w().max(1));
        let right = work.r - frame.r;
        let left = if right >= width {
            frame.r
        } else {
            (work.r - width).max(work.l)
        };
        Rect {
            l: left,
            t: frame.t,
            r: left + width,
            b: frame.b,
        }
    }
}

struct UndoSnap {
    text: String,
    start: i32,
    end: i32,
}

struct PageState {
    edit: HWND,
    font: HFONT,
    brush: HBRUSH,
    old_edit: isize,
    undo: Vec<UndoSnap>,
    redo: Vec<UndoSnap>,
    last_edit: Option<std::time::Instant>,
    last_typing: bool,
    anchor: Option<i32>,
}

unsafe fn create_page(owner: HWND, frame: Rect, text: &str) -> Result<HWND> {
    let instance: HINSTANCE = GetModuleHandleW(None)?.into();
    let class = w!("dt-ocr.maskpage");
    let wc = WNDCLASSW {
        style: WNDCLASS_STYLES(0),
        lpfnWndProc: Some(page_wndproc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: HICON::default(),
        hCursor: LoadCursorW(None, IDC_IBEAM).unwrap_or_default(),
        hbrBackground: HBRUSH::default(),
        lpszMenuName: windows::core::PCWSTR::null(),
        lpszClassName: windows::core::PCWSTR(class.as_ptr()),
    };
    let atom = RegisterClassW(&wc);
    if atom == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
        anyhow::bail!(
            "RegisterClassW(maskpage) failed: {}",
            windows::core::Error::from_win32()
        );
    }
    let work = crate::frame::bounds(owner);
    let work = if work.has_area() {
        work
    } else {
        crate::frame::work_area()
    };
    let span = page_span(frame, work, text);
    let r = outside_rect(frame, work, span);
    if r.w() < 40 || r.h() < 40 {
        anyhow::bail!("outside editor has no room");
    }
    let face = style().font;
    let wide_face = wide(&face);
    // Monospaced OCR result (`[mask] font`, default Sarasa Term SC Nerd).
    let font = CreateFontW(
        -12,
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
        DEFAULT_QUALITY,
        FIXED_PITCH.0 as u32,
        windows::core::PCWSTR(wide_face.as_ptr()),
    );
    let brush = CreateSolidBrush(COLORREF(SELECT_ORANGE));
    let page = CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
        class,
        windows::core::PCWSTR::null(),
        WS_POPUP | WS_VISIBLE | WS_CLIPCHILDREN,
        r.l,
        r.t,
        r.w(),
        r.h(),
        Some(owner),
        None,
        Some(instance),
        None,
    )?;
    SetWindowLongPtrW(
        page,
        GWLP_USERDATA,
        Box::into_raw(Box::new(PageState {
            edit: HWND::default(),
            font,
            brush,
            old_edit: 0,
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit: None,
            last_typing: false,
            anchor: None,
        })) as isize,
    );
    let edit_style = WINDOW_STYLE(
        WS_CHILD.0
            | WS_VISIBLE.0
            | WS_TABSTOP.0
            | 0x0004 // ES_MULTILINE
            | 0x0040 // ES_AUTOVSCROLL
            | 0x1000 // ES_WANTRETURN
            | 0x0100, // ES_NOHIDESEL
    );
    let edit = match CreateWindowExW(
        WINDOW_EX_STYLE(0),
        w!("EDIT"),
        windows::core::PCWSTR::null(),
        edit_style,
        6,
        6,
        (r.w() - 12).max(1),
        (r.h() - 12).max(1),
        Some(page),
        None,
        Some(instance),
        None,
    ) {
        Ok(edit) => edit,
        Err(e) => {
            let _ = DestroyWindow(page);
            return Err(e.into());
        }
    };
    state_userdata(page).edit = edit;
    SendMessageW(
        edit,
        WM_SETFONT,
        Some(WPARAM(font.0 as usize)),
        Some(LPARAM(1)),
    );
    let text = wide(&edit_newlines(text));
    let _ = SetWindowTextW(edit, windows::core::PCWSTR(text.as_ptr()));
    SendMessageW(edit, 0x00B1, Some(WPARAM(0)), Some(LPARAM(0))); // EM_SETSEL
    let old = SetWindowLongPtrW(edit, GWLP_WNDPROC, edit_proc as *const () as isize);
    state_userdata(page).old_edit = old;
    let _ = InvalidateRect(Some(page), None, true);
    logline(&format!(
        "mask: outside editor {}x{} at ({},{})",
        r.w(),
        r.h(),
        r.l,
        r.t
    ));
    Ok(page)
}

unsafe fn state_userdata(hwnd: HWND) -> &'static mut PageState {
    &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut PageState)
}

unsafe fn page_state(hwnd: HWND) -> Option<&'static mut PageState> {
    let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut PageState;
    p.as_mut()
}

unsafe fn edit_text(edit: HWND) -> String {
    let len = SendMessageW(edit, 0x000E, None, None).0 as usize; // WM_GETTEXTLENGTH
    let mut buf = vec![0u16; len + 1];
    let got = SendMessageW(
        edit,
        0x000D, // WM_GETTEXT
        Some(WPARAM(len + 1)),
        Some(LPARAM(buf.as_mut_ptr() as isize)),
    )
    .0 as usize;
    String::from_utf16_lossy(&buf[..got.min(len)])
}

unsafe fn edit_sel(edit: HWND) -> (i32, i32) {
    let mut start = 0i32;
    let mut end = 0i32;
    SendMessageW(
        edit,
        0x00B0, // EM_GETSEL
        Some(WPARAM(&mut start as *mut i32 as usize)),
        Some(LPARAM(&mut end as *mut i32 as isize)),
    );
    (start, end)
}

unsafe fn set_edit_sel(edit: HWND, start: i32, end: i32) {
    SendMessageW(
        edit,
        0x00B1, // EM_SETSEL
        Some(WPARAM(start as usize)),
        Some(LPARAM(end as isize)),
    );
}

unsafe fn snapshot(edit: HWND) -> UndoSnap {
    let (start, end) = edit_sel(edit);
    UndoSnap {
        text: edit_text(edit),
        start,
        end,
    }
}

unsafe fn apply_snapshot(page: HWND, snap: &UndoSnap) {
    let edit = state_userdata(page).edit;
    let text = wide(&snap.text);
    let _ = SetWindowTextW(edit, windows::core::PCWSTR(text.as_ptr()));
    set_edit_sel(edit, snap.start, snap.end);
    SendMessageW(edit, 0x00B7, None, None); // EM_SCROLLCARET
    state_userdata(page).anchor = None;
}

unsafe fn remember(page: HWND, typing: bool) {
    let s = state_userdata(page);
    s.anchor = None;
    let now = std::time::Instant::now();
    let coalesce = typing
        && s.last_typing
        && s.last_edit
            .map(|t| now.duration_since(t) < std::time::Duration::from_millis(700))
            .unwrap_or(false);
    if !coalesce {
        let snap = snapshot(s.edit);
        if s.undo.len() >= 200 {
            s.undo.remove(0);
        }
        s.undo.push(snap);
    }
    s.redo.clear();
    s.last_edit = Some(now);
    s.last_typing = typing;
}

unsafe fn undo(page: HWND) {
    let s = state_userdata(page);
    let Some(prev) = s.undo.pop() else {
        return;
    };
    let current = snapshot(s.edit);
    if s.redo.len() >= 200 {
        s.redo.remove(0);
    }
    s.redo.push(current);
    s.last_edit = None;
    s.last_typing = false;
    s.anchor = None;
    apply_snapshot(page, &prev);
}

unsafe fn redo(page: HWND) {
    let s = state_userdata(page);
    let Some(next) = s.redo.pop() else {
        return;
    };
    let current = snapshot(s.edit);
    if s.undo.len() >= 200 {
        s.undo.remove(0);
    }
    s.undo.push(current);
    s.last_edit = None;
    s.last_typing = false;
    s.anchor = None;
    apply_snapshot(page, &next);
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn is_symbol(c: char) -> bool {
    !c.is_whitespace() && !is_word_char(c)
}

fn is_newline(c: char) -> bool {
    c == '\r' || c == '\n'
}

/// Ctrl+Left/Right stops at token edges: words (letters, digits, `_`, CJK),
/// runs of other symbols (`=`, `<=`, …) or a line break. It never parks after
/// the whitespace that follows a word, which is what the stock EDIT does.
fn next_token(chars: &[char], from: usize) -> usize {
    let mut i = from;
    if i >= chars.len() {
        return chars.len();
    }
    if is_newline(chars[i]) {
        while i < chars.len() && is_newline(chars[i]) {
            i += 1;
        }
        return i;
    }
    if chars[i].is_whitespace() {
        while i < chars.len() && chars[i].is_whitespace() && !is_newline(chars[i]) {
            i += 1;
        }
        if i < chars.len() && is_newline(chars[i]) {
            while i < chars.len() && is_newline(chars[i]) {
                i += 1;
            }
            return i;
        }
    }
    if i >= chars.len() {
        return chars.len();
    }
    let word = is_word_char(chars[i]);
    let symbol = is_symbol(chars[i]);
    while i < chars.len()
        && !chars[i].is_whitespace()
        && ((word && is_word_char(chars[i])) || (symbol && is_symbol(chars[i])))
    {
        i += 1;
    }
    i
}

fn prev_token(chars: &[char], from: usize) -> usize {
    let mut i = from.min(chars.len());
    while i > 0 && chars[i - 1].is_whitespace() && !is_newline(chars[i - 1]) {
        i -= 1;
    }
    if i > 0 && is_newline(chars[i - 1]) {
        while i > 0 && is_newline(chars[i - 1]) {
            i -= 1;
        }
        return i;
    }
    if i == 0 {
        return 0;
    }
    let word = is_word_char(chars[i - 1]);
    let symbol = is_symbol(chars[i - 1]);
    while i > 0
        && !chars[i - 1].is_whitespace()
        && ((word && is_word_char(chars[i - 1])) || (symbol && is_symbol(chars[i - 1])))
    {
        i -= 1;
    }
    i
}

unsafe fn move_word(page: HWND, dir: i32, extend: bool) {
    let s = state_userdata(page);
    let text = edit_text(s.edit);
    let chars: Vec<char> = text.chars().collect();
    let mut offsets = Vec::with_capacity(chars.len() + 1);
    let mut off = 0usize;
    offsets.push(0);
    for c in &chars {
        off += c.len_utf16();
        offsets.push(off);
    }
    let (start, end) = edit_sel(s.edit);
    let (anchor, head) = if extend {
        let anchor = s.anchor.unwrap_or(start);
        let head = if anchor == start { end } else { start };
        (Some(anchor), head)
    } else {
        (None, if start == end { end } else { end.max(start) })
    };
    let idx = offsets
        .iter()
        .position(|&o| o as i32 >= head)
        .unwrap_or(chars.len());
    let next = if dir > 0 {
        next_token(&chars, idx)
    } else {
        prev_token(&chars, idx)
    };
    let target = offsets[next] as i32;
    match anchor {
        Some(anchor) => set_edit_sel(s.edit, anchor.min(target), anchor.max(target)),
        None => set_edit_sel(s.edit, target, target),
    }
    s.anchor = anchor;
    SendMessageW(s.edit, 0x00B7, None, None); // EM_SCROLLCARET
}

unsafe fn select_all(page: HWND) {
    let s = state_userdata(page);
    set_edit_sel(s.edit, 0, -1);
    s.anchor = Some(0);
}

unsafe fn layout_page(page: HWND) {
    let Some(s) = page_state(page) else {
        return;
    };
    let mut client = RECT::default();
    let _ = GetClientRect(page, &mut client);
    let _ = MoveWindow(
        s.edit,
        6,
        6,
        (client.right - 12).max(1),
        (client.bottom - 12).max(1),
        true,
    );
}
unsafe extern "system" fn edit_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let page = GetParent(hwnd).unwrap_or_default();
    let old = state_userdata(page).old_edit;
    if msg == WM_LBUTTONDOWN || msg == WM_LBUTTONDBLCLK {
        state_userdata(page).anchor = None;
    }
    if msg == WM_KEYDOWN {
        let key = VIRTUAL_KEY(wparam.0 as u16);
        if key == VK_ESCAPE {
            let _ = PostMessageW(Some(page), WM_CLOSE, WPARAM(0), LPARAM(0));
            return LRESULT(0);
        }
        let ctrl = GetKeyState(0x11) < 0; // VK_CONTROL
        let shift = GetKeyState(0x10) < 0; // VK_SHIFT
        let alt = GetKeyState(0x12) < 0; // VK_MENU
        let is_move = matches!(key.0, 0x21..=0x28);
        if is_move {
            if shift {
                let s = state_userdata(page);
                if s.anchor.is_none() {
                    let (start, _) = edit_sel(s.edit);
                    s.anchor = Some(start);
                }
            } else {
                state_userdata(page).anchor = None;
            }
        }
        if ctrl && !alt {
            match key.0 {
                0x41 => {
                    select_all(page);
                    return LRESULT(0);
                }
                0x5A => {
                    if shift {
                        redo(page);
                    } else {
                        undo(page);
                    }
                    return LRESULT(0);
                }
                0x59 => {
                    redo(page);
                    return LRESULT(0);
                }
                0x25 => {
                    move_word(page, -1, shift);
                    return LRESULT(0);
                }
                0x27 => {
                    move_word(page, 1, shift);
                    return LRESULT(0);
                }
                _ => {}
            }
        }
        if key.0 == 0x08 || key.0 == 0x2E {
            remember(page, false);
        }
    } else if msg == WM_CHAR {
        let ch = wparam.0 as u32;
        if ch >= 32 && ch != 127 {
            remember(page, true);
        } else if ch == 13 {
            remember(page, false);
        }
    } else if msg == WM_PASTE || msg == WM_CUT || msg == WM_CLEAR {
        remember(page, false);
    } else if msg == 0x0286 {
        // WM_IME_CHAR: committed IME text is an edit too.
        remember(page, true);
    }
    let old: WNDPROC = std::mem::transmute(old);
    CallWindowProcW(old, hwnd, msg, wparam, lparam)
}

unsafe extern "system" fn page_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_ERASEBKGND => {
            let Some(s) = page_state(hwnd) else {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            };
            let hdc = HDC(wparam.0 as *mut core::ffi::c_void);
            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);
            let _ = FillRect(hdc, &client, s.brush);
            LRESULT(1)
        }
        WM_CTLCOLOREDIT => {
            let Some(s) = page_state(hwnd) else {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            };
            let hdc = HDC(wparam.0 as *mut core::ffi::c_void);
            let _ = SetBkColor(hdc, COLORREF(SELECT_ORANGE));
            let _ = SetTextColor(hdc, COLORREF(0));
            LRESULT(s.brush.0 as isize)
        }
        WM_SIZE => {
            layout_page(hwnd);
            LRESULT(0)
        }
        WM_KEYDOWN if VIRTUAL_KEY(wparam.0 as u16) == VK_ESCAPE => {
            dismiss();
            LRESULT(0)
        }
        WM_CLOSE => {
            dismiss();
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            dismiss();
            LRESULT(0)
        }
        WM_DESTROY => {
            if let Some(s) = page_state(hwnd) {
                if IsWindow(Some(s.edit)).as_bool() {
                    let _ = DestroyWindow(s.edit);
                }
                let s = Box::from_raw(s as *mut PageState);
                let _ = DeleteObject(s.font.into());
                let _ = DeleteObject(s.brush.into());
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
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
    let text = selection_text(&s.words, picked.clone());
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

pub(crate) fn preview(text: &str) -> String {
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
            let _ = SetCapture(hwnd);
            s.drag = Some(point());
            s.selected.clear();
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
            let _ = ReleaseCapture();
            let start = state(hwnd).drag.take();
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
        WM_KEYDOWN => {
            let key = VIRTUAL_KEY(wparam.0 as u16);
            if GetKeyState(VK_CONTROL.0 as i32) < 0 && key.0 == 0x43 {
                copy_all_and_close(hwnd);
                return LRESULT(0);
            }
            if key == VK_ESCAPE {
                dismiss();
            }
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_ACTIVATE as isize),
        WM_DESTROY => {
            let s = Box::from_raw(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut MaskState);
            if IsWindow(Some(s.page)).as_bool() {
                let _ = DestroyWindow(s.page);
            }
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

    #[test]
    fn word_navigation_stops_at_words_symbols_and_line_breaks() {
        let chars: Vec<char> = "foo = bar\r\nbaz".chars().collect();
        assert_eq!(next_token(&chars, 0), 3);
        assert_eq!(next_token(&chars, 3), 5);
        assert_eq!(next_token(&chars, 5), 9);
        assert_eq!(next_token(&chars, 9), 11);
        assert_eq!(next_token(&chars, 11), 14);
        assert_eq!(prev_token(&chars, 14), 11);
        assert_eq!(prev_token(&chars, 11), 9);
        assert_eq!(prev_token(&chars, 9), 6);
        assert_eq!(prev_token(&chars, 6), 4);
        assert_eq!(prev_token(&chars, 4), 0);
    }

    #[test]
    fn word_navigation_handles_cjk_as_one_token() {
        let chars: Vec<char> = "你好 world".chars().collect();
        assert_eq!(next_token(&chars, 0), 2);
        assert_eq!(next_token(&chars, 2), 8);
        assert_eq!(prev_token(&chars, 8), 3);
        assert_eq!(prev_token(&chars, 3), 0);
    }

    fn word(text: &str, l: i32, t: i32, r: i32, b: i32) -> Word {
        Word {
            text: text.to_string(),
            line: 0,
            rect: Rect { l, t, r, b },
        }
    }

    fn word_on(line: u32, text: &str, l: i32, t: i32, r: i32, b: i32) -> Word {
        Word {
            text: text.to_string(),
            line,
            rect: Rect { l, t, r, b },
        }
    }

    #[test]
    fn line_segments_join_small_gaps_and_keep_columns_apart() {
        let words = [
            word_on(0, "ab", 0, 0, 20, 20),
            word_on(0, "cd", 24, 0, 44, 20),
            word_on(0, "ef", 60, 0, 80, 20),
            word_on(1, "gh", 0, 30, 20, 50),
        ];
        let seg = line_segments(&words);
        assert_eq!(seg.len(), 3);
        assert_eq!(
            seg[0],
            Rect {
                l: 0,
                t: 0,
                r: 44,
                b: 20
            }
        );
        assert_eq!(
            seg[1],
            Rect {
                l: 60,
                t: 0,
                r: 80,
                b: 20
            }
        );
        assert_eq!(
            seg[2],
            Rect {
                l: 0,
                t: 30,
                r: 20,
                b: 50
            }
        );
    }

    #[test]
    fn selection_text_keeps_line_breaks_and_cjk_spacing() {
        let words = [
            word_on(0, "Quick", 0, 0, 40, 20),
            word_on(0, "brown", 50, 0, 100, 20),
            word_on(0, "你", 110, 0, 120, 20),
            word_on(0, "好", 120, 0, 130, 20),
            word_on(1, "world", 0, 30, 40, 50),
        ];
        let text = selection_text(&words, vec![4, 1, 3, 0, 2]);
        assert_eq!(text, "Quick brown 你好\nworld");
        assert_eq!(selection_text(&words, vec![1]), "brown");
    }

    #[test]
    fn wide_text_is_nul_terminated() {
        let w = wide("故障abc");
        assert_eq!(w.last(), Some(&0));
        assert_eq!(String::from_utf16_lossy(&w[..w.len() - 1]), "故障abc");
    }

    #[test]
    fn edit_text_uses_crlf_line_breaks() {
        assert_eq!(edit_newlines("a\n\nb"), "a\r\n\r\nb");
        assert_eq!(edit_newlines("a\r\nb"), "a\r\nb");
    }

    #[test]
    fn outside_joins_lines_and_separates_paragraphs() {
        let words = [
            word_on(0, "Hello", 0, 0, 40, 20),
            word_on(0, "world", 50, 0, 90, 20),
            word_on(1, "again", 0, 25, 40, 45),
            word_on(2, "你好", 0, 90, 40, 110),
            word_on(2, "世界", 45, 90, 80, 110),
        ];
        assert_eq!(panel_text(&words), "Hello world again\n\n你好世界");
        assert_eq!(all_text(&words), panel_text(&words));
    }

    #[test]
    fn outside_bar_follows_frame_orientation() {
        let work = Rect {
            l: 0,
            t: 0,
            r: 1080,
            b: 1080,
        };
        let landscape = Rect {
            l: 100,
            t: 300,
            r: 900,
            b: 700,
        };
        assert_eq!(
            outside_rect(landscape, work, 160),
            Rect {
                l: 100,
                t: 700,
                r: 900,
                b: 860
            }
        );
        let portrait = Rect {
            l: 600,
            t: 100,
            r: 900,
            b: 700,
        };
        assert_eq!(
            outside_rect(portrait, work, 120),
            Rect {
                l: 900,
                t: 100,
                r: 1020,
                b: 700
            }
        );
    }

    #[test]
    fn outside_bar_floats_up_when_below_is_short() {
        let work = Rect {
            l: 0,
            t: 0,
            r: 800,
            b: 620,
        };
        let frame = Rect {
            l: 100,
            t: 30,
            r: 700,
            b: 600,
        };
        let bar = outside_rect(frame, work, 100);
        assert_eq!(bar.b, work.b);
        assert!(bar.t < frame.b);
        assert!(bar.t >= work.t);
    }

    #[test]
    fn outside_bar_floats_left_when_right_is_short() {
        let work = Rect {
            l: 0,
            t: 0,
            r: 1080,
            b: 1080,
        };
        let frame = Rect {
            l: 700,
            t: 100,
            r: 1050,
            b: 900,
        };
        let bar = outside_rect(frame, work, 200);
        assert_eq!(bar.r, work.r);
        assert!(bar.l < frame.r);
    }

    #[test]
    fn outside_editor_span_grows_with_lines_and_caps() {
        let work = Rect {
            l: 0,
            t: 0,
            r: 1920,
            b: 1080,
        };
        let landscape = Rect {
            l: 0,
            t: 0,
            r: 1920,
            b: 1080,
        };
        assert_eq!(page_span(landscape, work, "one line"), 64);
        let many = "line\n".repeat(50);
        assert_eq!(page_span(landscape, work, &many), 720);
        let portrait = Rect {
            l: 0,
            t: 0,
            r: 300,
            b: 900,
        };
        assert_eq!(page_span(portrait, work, "one line"), 450);
    }

    #[test]
    fn inverse_colours_swap_dark_and_light() {
        let mut dark = vec![0u32; 16];
        let rc = Rect {
            l: 0,
            t: 0,
            r: 4,
            b: 4,
        };
        assert_eq!(inverse_colors(dark.as_mut_ptr(), 4, rc), (0xffffff, 0));
        let mut light = vec![0x00ff_ffffu32; 16];
        assert_eq!(inverse_colors(light.as_mut_ptr(), 4, rc), (0, 0xffffff));
    }

    #[test]
    fn auto_size_is_per_line_median() {
        let words = [
            word_on(0, "a", 0, 0, 10, 10),
            word_on(0, "b", 12, 0, 22, 30),
            word_on(0, "c", 24, 0, 34, 20),
            word_on(1, "d", 0, 40, 10, 60),
        ];
        let ems = line_ems(&words);
        assert_eq!(ems.len(), 2);
        assert_eq!(line_em(&ems, 0), 16);
        assert_eq!(line_em(&ems, 1), 16);
        assert_eq!(line_em(&ems, 9), 13);
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
