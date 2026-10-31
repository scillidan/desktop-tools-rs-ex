//! P0.3 overlay mask: one topmost layered window over the frame region,
//! rendered per-pixel through `UpdateLayeredWindow`. Untouched pixels come
//! from the native-resolution snapshot captured with the OCR image, so screen
//! changes cannot misalign the mask. OCR words get black backing boxes and
//! white glyphs. Click copies a word; drag copies a selection; Esc dismisses.
//!
//! Every pixel is opaque: transparent layered pixels pass mouse input through,
//! while the mask must own the frame region after OCR.

use crate::capture::copy_text;
use crate::config::{MaskFill, MaskLayout, MaskSize};
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
    ReleaseCapture, SetCapture, SetFocus, VIRTUAL_KEY, VK_ESCAPE,
};
use windows::Win32::UI::WindowsAndMessaging::*;

const BOX_ALPHA: u32 = 0xFF00_0000;
const TEXT_WHITE: u32 = 0x00FF_FFFF;
const SELECT_ORANGE: u32 = 0x0000A5FF; // RGB(255, 165, 0)
const DEFAULT_FACE: &str = "Sarasa Term SC Nerd";

/// `[mask]` style resolved when a mask opens: font face, size and layout.
#[derive(Clone)]
struct Style {
    font: String,
    size: MaskSize,
    layout: MaskLayout,
}

static STYLE: Mutex<Style> = Mutex::new(Style {
    font: String::new(),
    size: MaskSize::Auto,
    layout: MaskLayout::Overlay,
});

/// Live config (`main` calls this at startup and on every hot-reload).
pub fn set_style(font: String, size: MaskSize, layout: MaskLayout) {
    if let Ok(mut s) = STYLE.lock() {
        *s = Style { font, size, layout };
    }
}

fn style() -> Style {
    STYLE.lock().map(|s| s.clone()).unwrap_or(Style {
        font: DEFAULT_FACE.to_string(),
        size: MaskSize::Pt(10),
        layout: MaskLayout::Overlay,
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
    /// Reader-mode text (None = overlay layout).
    reader: Option<String>,
}

/// Style resolved for one draw pass (keeps `draw_base` under the clippy arg
/// limit).
struct Base<'a> {
    fixed_em: Option<i32>,
    layout: MaskLayout,
    face: &'a [u16],
    reader: Option<&'a str>,
}

static MASK: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
static CAPTURE_EXCLUDED: AtomicBool = AtomicBool::new(false);

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
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
        logline("mask: shown");
    }
}

fn mask_hwnd() -> HWND {
    HWND(MASK.load(std::sync::atomic::Ordering::Relaxed) as *mut core::ffi::c_void)
}

/// Closes the overlay (no-op when none is open) and returns the frame to its
/// normal interaction state.
/// True when an old Windows build would capture the visible mask itself.
pub unsafe fn visible_without_capture_exclusion() -> bool {
    IsWindow(Some(mask_hwnd())).as_bool() && !CAPTURE_EXCLUDED.load(Ordering::Relaxed)
}

pub unsafe fn dismiss() {
    let hwnd = mask_hwnd();
    if IsWindow(Some(hwnd)).as_bool() {
        let _ = DestroyWindow(hwnd);
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
    let reader = if style.layout == MaskLayout::Reader {
        Some(reader_text(&words))
    } else {
        None
    };
    let face_w = wide(&style.font);
    draw_base(
        memdc,
        bits,
        w,
        h,
        &words,
        &Base {
            fixed_em,
            layout: style.layout,
            face: &face_w,
            reader: reader.as_deref(),
        },
    );

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
            reader,
        })) as isize,
    );
    MASK.store(hwnd.0 as isize, std::sync::atomic::Ordering::Relaxed);
    // Auto-refresh samples the source without hiding this overlay. Windows
    // omits the mask from the capture and exposes the desktop underneath.
    let excluded = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE).is_ok();
    CAPTURE_EXCLUDED.store(excluded, Ordering::Relaxed);
    if !excluded {
        logline("warn: mask cannot be excluded from capture; auto refresh will dismiss it once");
    }
    present(hwnd);
    let _ = ShowWindow(hwnd, SW_SHOW);
    let _ = SetForegroundWindow(hwnd);
    let _ = SetFocus(Some(hwnd));
    // The first word + rect is the cheapest way to see, from the log alone,
    // what the engine found and where a click test can aim.
    let s = state(hwnd);
    let mode = match (style.layout, fill_lines()) {
        (MaskLayout::Reader, _) => "reader",
        (MaskLayout::Overlay, true) => "overlay:lines",
        (MaskLayout::Overlay, false) => "overlay:boxes",
    };
    logline(&format!(
        "mask: {mode}, {} words, font {}+{}px over {w}x{h} at ({},{}), first `{}` {:?}",
        s.words.len(),
        style.font,
        fixed_em
            .map(|e| e.to_string())
            .unwrap_or_else(|| "auto".into()),
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

/// Paragraphs for reader mode: OCR lines are joined into one flow (spaces via
/// `join_words`) and separated by a blank line when the vertical gap is at
/// least one line height.
fn reader_text(words: &[Word]) -> String {
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

fn median_line_em(words: &[Word]) -> i32 {
    let mut ems: Vec<i32> = line_ems(words).into_iter().map(|(_, em)| em).collect();
    ems.sort_unstable();
    ems.get(ems.len() / 2).copied().unwrap_or(13)
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

/// Draws the mask base once. Overlay: black boxes/bands plus white text at the
/// OCR positions. Reader: an opaque black page with the reflowed text.
/// Finally, makes every pixel opaque so the frozen base stays visible.
unsafe fn draw_base(hdc: HDC, bits: *mut u32, w: i32, h: i32, words: &[Word], base: &Base) {
    let _ = SetBkMode(hdc, TRANSPARENT);
    let _ = SetTextColor(hdc, COLORREF(TEXT_WHITE));
    let black = CreateSolidBrush(COLORREF(0));
    let mut fonts: Vec<(i32, HFONT)> = Vec::new();

    if base.layout == MaskLayout::Reader {
        let page = RECT {
            left: 0,
            top: 0,
            right: w,
            bottom: h,
        };
        let _ = FillRect(hdc, &page, black);
        for i in 0..(w * h) as usize {
            *bits.add(i) = BOX_ALPHA;
        }
        if let Some(text) = base.reader {
            let em = base.fixed_em.unwrap_or_else(|| median_line_em(words));
            let font = font_for(base.face, em, &mut fonts);
            let old = SelectObject(hdc, font.into());
            let mut text: Vec<u16> = text.encode_utf16().collect();
            let mut rc = RECT {
                left: 10,
                top: 8,
                right: (w - 10).max(12),
                bottom: (h - 8).max(10),
            };
            let _ = DrawTextW(
                hdc,
                &mut text,
                &mut rc,
                DT_LEFT | DT_TOP | DT_WORDBREAK | DT_NOPREFIX,
            );
            let _ = SelectObject(hdc, old);
        }
        let _ = DeleteObject(black.into());
        for (_, font) in fonts {
            let _ = DeleteObject(font.into());
        }
        // The page is already opaque; glyph pixels arrive with alpha 0.
        for i in 0..(w * h) as usize {
            let p = *bits.add(i);
            if p & 0x00FF_FFFF != 0 {
                *bits.add(i) = p | BOX_ALPHA;
            }
        }
        return;
    }

    // `lines`: one continuous band per line segment, so the gaps between
    // words stop showing the screen through. The glyphs keep their OCR
    // positions, so Latin spaces and CJK spacing stay exactly as rendered.
    if fill_lines() {
        for seg in line_segments(words) {
            let rc = clip_rect(seg, w, h);
            if rc.w() < 2 || rc.h() < 2 {
                continue;
            }
            let win = to_win(rc);
            let _ = FillRect(hdc, &win, black);
            for y in rc.t..rc.b {
                for x in rc.l..rc.r {
                    *bits.add((y * w + x) as usize) |= BOX_ALPHA;
                }
            }
        }
    }
    let ems = line_ems(words);
    for word in words {
        let rc = clip_rect(word.rect, w, h);
        if rc.w() < 2 || rc.h() < 2 {
            continue;
        }
        if !fill_lines() {
            let win = to_win(rc);
            let _ = FillRect(hdc, &win, black);
            // Opaque box first; the text pass may rewrite alpha on glyph
            // pixels, the RGB pass below restores those.
            for y in rc.t..rc.b {
                for x in rc.l..rc.r {
                    *bits.add((y * w + x) as usize) |= BOX_ALPHA;
                }
            }
        }
        let em = base.fixed_em.unwrap_or_else(|| line_em(&ems, word.line));
        let font = font_for(base.face, em, &mut fonts);
        let old = SelectObject(hdc, font.into());
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
    let _ = DeleteObject(black.into());
    for (_, font) in fonts {
        let _ = DeleteObject(font.into());
    }
    for i in 0..(w * h) as usize {
        *bits.add(i) |= BOX_ALPHA;
    }
}

/// Repaints the layered surface: baseline + selection outlines.
unsafe fn present(hwnd: HWND) {
    let s = state(hwnd);
    let len = (s.rect.w() * s.rect.h()) as usize;
    std::ptr::copy_nonoverlapping(s.base.as_ptr(), s.bits, len);
    let brush = CreateSolidBrush(COLORREF(SELECT_ORANGE));
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
            let _ = SetCapture(hwnd);
            if s.reader.is_some() {
                return LRESULT(0);
            }
            s.drag = Some(point());
            s.selected.clear();
            present(hwnd);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            if state(hwnd).reader.is_some() {
                return LRESULT(0);
            }
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
            if let Some(text) = state(hwnd).reader.clone() {
                match copy_text(hwnd, &text) {
                    Ok(()) => logline(&format!(
                        "mask: copied {} chars (reader): {}",
                        text.chars().count(),
                        preview(&text)
                    )),
                    Err(e) => logline(&format!("mask: clipboard failed: {e:#}")),
                }
                return LRESULT(0);
            }
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
            crate::ocr::auto_disarm();
            dismiss();
            LRESULT(0)
        }
        WM_KEYDOWN if VIRTUAL_KEY(wparam.0 as u16) == VK_ESCAPE => {
            crate::ocr::auto_disarm();
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
            CAPTURE_EXCLUDED.store(false, Ordering::Relaxed);
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
    fn reader_joins_lines_and_separates_paragraphs() {
        let words = [
            word_on(0, "Hello", 0, 0, 40, 20),
            word_on(0, "world", 50, 0, 90, 20),
            word_on(1, "again", 0, 25, 40, 45),
            word_on(2, "你好", 0, 90, 40, 110),
            word_on(2, "世界", 45, 90, 80, 110),
        ];
        assert_eq!(reader_text(&words), "Hello world again\n\n你好世界");
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
