//! P0.3 OCR: capture the frame region on the UI thread, recognize the words
//! on a worker thread, then post the result back. The UI thread never blocks,
//! and the frame is click-through while the worker runs.
//!
//! Engine: the Windows built-in OCR (Windows.Media.Ocr) — no downloads, word
//! boxes out of the box, en + zh packs already installed here. P0.4 replaces
//! this with the RapidOCR/Tesseract router; this module keeps the seam
//! (`Word`, `Outcome`) so the router only swaps `recognize`.

use crate::logline;
use anyhow::{anyhow, bail, ensure, Context, Result};
use dt_common::layout::Rect;
use std::time::Duration;
use windows::core::Interface;
use windows::Foundation::AsyncStatus;
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::Buffer;
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::WinRT::{IBufferByteAccess, RoInitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

/// Worker → UI thread: recognition finished, `lparam` is `Box<Outcome>`.
pub const WM_OCR_DONE: u32 = WM_APP + 1;

/// One recognized word. `rect` is in mask-window client coordinates (the
/// frame border is cropped out of the capture and the offset is added back).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Word {
    pub text: String,
    pub rect: Rect,
    /// OCR line index in reading order; the mask groups lines for the
    /// continuous fill and for line breaks when copying a selection.
    pub line: u32,
}

pub struct Outcome {
    /// The frame rect the words belong to (the mask window's screen rect).
    pub rect: Rect,
    pub words: Result<Vec<Word>>,
}

/// Frame border thickness cropped from the capture: the 2 px GDI ring is not
/// content, and cropping it keeps the border out of the OCR image.
const BORDER: i32 = 2;

/// Upper bound for an upscaled capture: 16 Mpx BGRA is 64 MB for the pixels
/// plus the engine's own copy. Beyond that the extra cost buys little.
const MAX_OCR_PIXELS: u64 = 16_000_000;

/// Used when the engine limit cannot be queried before the capture (WinRT
/// wants an initialized apartment; the UI thread has none).
const FALLBACK_MAX_DIM: u32 = 4096;

/// Capture the frame region, mark the frame click-through and start the
/// worker. Returns immediately; the result arrives as [`WM_OCR_DONE`].
pub unsafe fn start(frame: HWND) -> Result<()> {
    let rect = crate::frame::screen_rect(frame);
    ensure!(
        rect.w() > BORDER * 2 && rect.h() > BORDER * 2,
        "frame too small"
    );
    let capture = dt_common::layout::inset(rect, BORDER);
    let max = OcrEngine::MaxImageDimension().unwrap_or(FALLBACK_MAX_DIM);
    let scale = choose_scale(
        capture.w(),
        capture.h(),
        crate::frame::ocr_scale(frame),
        max,
    );
    let (w, h, pixels) = capture_bgra(capture, scale)?;
    crate::frame::set_ocr_running(frame, true);
    logline(&format!(
        "ocr: capture {w}x{h} (scale {scale}, max {max}px) at ({},{})",
        capture.l, capture.t
    ));
    let frame_addr = frame.0 as isize;
    let language = crate::frame::ocr_language(frame);
    let inv = 1.0 / scale as f32;
    std::thread::spawn(move || {
        let words = recognize(w, h, &pixels, language.as_deref()).map(|lines| {
            let mut words: Vec<Word> = lines.into_iter().flatten().collect();
            for word in &mut words {
                let r = word.rect;
                word.rect = Rect {
                    l: (r.l as f32 * inv).round() as i32,
                    t: (r.t as f32 * inv).round() as i32,
                    r: (r.r as f32 * inv).round() as i32,
                    b: (r.b as f32 * inv).round() as i32,
                };
                word.rect.l += capture.l - rect.l;
                word.rect.r += capture.l - rect.l;
                word.rect.t += capture.t - rect.t;
                word.rect.b += capture.t - rect.t;
            }
            words
        });
        let outcome = Box::into_raw(Box::new(Outcome { rect, words }));
        let frame = HWND(frame_addr as *mut core::ffi::c_void);
        if PostMessageW(
            Some(frame),
            WM_OCR_DONE,
            Default::default(),
            LPARAM(outcome as isize),
        )
        .is_err()
        {
            drop(Box::from_raw(outcome));
        }
    });
    Ok(())
}

/// Upscale the capture by the configured factor, clamped so the engine and
/// the memory budget stay happy. Screen fonts below ~12 px are where the
/// engine degrades; 2-3x turns them into normal-sized text.
fn choose_scale(w: i32, h: i32, requested: u32, max: u32) -> u32 {
    let mut scale = requested.clamp(1, 4);
    while scale > 1 {
        let dw = w as u64 * scale as u64;
        let dh = h as u64 * scale as u64;
        if dw <= max as u64 && dh <= max as u64 && dw * dh <= MAX_OCR_PIXELS {
            break;
        }
        scale -= 1;
    }
    scale
}

/// Screen pixels of `rect` as top-down BGRA, upscaled by `scale` with GDI's
/// halftone stretch (area averaging: keeps small text legible where nearest
/// neighbour turns it to blocky noise). The layout SoftwareBitmap expects
/// top-down rows; GDI DIBs default to bottom-up, hence the negative height.
unsafe fn capture_bgra(rect: Rect, scale: u32) -> Result<(i32, i32, Vec<u8>)> {
    let (sw, sh) = (rect.w(), rect.h());
    let (w, h) = (sw * scale as i32, sh * scale as i32);
    let screen = GetDC(None);
    ensure!(!screen.0.is_null(), "GetDC(screen) failed");

    let src_mem = CreateCompatibleDC(Some(screen));
    let (src_bmp, _src_bits) = make_dib(screen, sw, sh)?;
    let old_src = SelectObject(src_mem, src_bmp.into());
    let blt = BitBlt(src_mem, 0, 0, sw, sh, Some(screen), rect.l, rect.t, SRCCOPY);

    let dst_mem = CreateCompatibleDC(Some(screen));
    let (dst_bmp, dst_bits) = make_dib(screen, w, h)?;
    let old_dst = SelectObject(dst_mem, dst_bmp.into());
    let _ = SetStretchBltMode(dst_mem, HALFTONE);
    let _ = SetBrushOrgEx(dst_mem, 0, 0, None);
    let stretched = blt.is_ok()
        && StretchBlt(dst_mem, 0, 0, w, h, Some(src_mem), 0, 0, sw, sh, SRCCOPY).as_bool();
    let pixels = if stretched && !dst_bits.is_null() {
        Some(
            std::slice::from_raw_parts(dst_bits as *const u8, (w as usize) * (h as usize) * 4)
                .to_vec(),
        )
    } else {
        None
    };

    let _ = SelectObject(dst_mem, old_dst);
    let _ = DeleteObject(dst_bmp.into());
    let _ = DeleteDC(dst_mem);
    let _ = SelectObject(src_mem, old_src);
    let _ = DeleteObject(src_bmp.into());
    let _ = DeleteDC(src_mem);
    let _ = ReleaseDC(None, screen);
    let pixels = pixels.context("BitBlt/StretchBlt failed")?;
    Ok((w, h, pixels))
}

/// A top-down 32-bit DIB section plus its raw pixel pointer.
unsafe fn make_dib(screen: HDC, w: i32, h: i32) -> Result<(HBITMAP, *mut core::ffi::c_void)> {
    let mut info = BITMAPINFO::default();
    info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    info.bmiHeader.biWidth = w;
    info.bmiHeader.biHeight = -h;
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    info.bmiHeader.biCompression = BI_RGB.0;
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let bmp = CreateDIBSection(Some(screen), &info, DIB_RGB_COLORS, &mut bits, None, 0)
        .context("CreateDIBSection failed")?;
    Ok((bmp, bits))
}

/// Runs on the worker thread: MTA init, OCR, word boxes per line.
///
/// `requested` = explicit language tag; `None` = auto. Auto uses the CJK
/// engine, then (when the frame contains Latin) a second English pass:
/// the zh model is weak on long Latin runs, the en model drops CJK, so the
/// two are merged line by line — CJK lines keep the zh result, Latin lines
/// take the en result. At 3x upscale this is the difference between
/// `pack 可 box` and `Pack my box`.
fn recognize(w: i32, h: i32, bgra: &[u8], requested: Option<&str>) -> Result<Vec<Vec<Word>>> {
    unsafe {
        // A fresh thread has no apartment yet; S_FALSE is fine, a different
        // apartment mode would be a bug here but not worth failing over.
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
        let max = OcrEngine::MaxImageDimension().unwrap_or(0);
        ensure!(
            max == 0 || (w as u32 <= max && h as u32 <= max),
            "capture {w}x{h} exceeds the OCR limit {max}px"
        );
        let bitmap = make_bitmap(w, h, bgra)?;
        let primary = pick_engine(requested)?;
        let primary_tag = engine_tag(&primary);
        let mut lines = read_lines(&primary, &bitmap)?;
        if requested.is_none() && !primary_tag.starts_with("en") && has_latin(&lines) {
            match pick_engine(Some("en")) {
                Ok(en) if engine_tag(&en) != primary_tag => match read_lines(&en, &bitmap) {
                    Ok(en_lines) => {
                        lines = merge_lines(&lines, &en_lines);
                        logline(&format!(
                            "ocr: merged {primary_tag} + {} -> {} lines",
                            engine_tag(&en),
                            lines.len()
                        ));
                    }
                    Err(e) => logline(&format!("warn: en OCR pass failed: {e:#}")),
                },
                Ok(_) => {}
                Err(e) => logline(&format!("warn: en OCR pack unavailable: {e:#}")),
            }
        }
        log_preview("ocr: text", &lines);
        // The en pass numbers its own lines; renumber the merged result so
        // the mask sees one contiguous reading order.
        for (i, line) in lines.iter_mut().enumerate() {
            for word in line {
                word.line = i as u32;
            }
        }
        Ok(lines)
    }
}

/// One SoftwareBitmap from top-down BGRA, shared by both engine passes.
unsafe fn make_bitmap(w: i32, h: i32, bgra: &[u8]) -> Result<SoftwareBitmap> {
    let len = (w as usize) * (h as usize) * 4;
    ensure!(bgra.len() >= len, "capture buffer is too small");
    let buffer = Buffer::Create(len as u32).context("Buffer::Create failed")?;
    buffer
        .SetLength(len as u32)
        .context("Buffer::SetLength failed")?;
    let access: IBufferByteAccess = buffer.cast().context("IBufferByteAccess missing")?;
    let dst = access
        .Buffer()
        .context("IBufferByteAccess::Buffer failed")?;
    std::ptr::copy_nonoverlapping(bgra.as_ptr(), dst, len);
    SoftwareBitmap::CreateCopyFromBuffer(&buffer, BitmapPixelFormat::Bgra8, w, h)
        .context("SoftwareBitmap::CreateCopyFromBuffer failed")
}

/// Run one engine over the bitmap and collect its lines of words.
unsafe fn read_lines(engine: &OcrEngine, bitmap: &SoftwareBitmap) -> Result<Vec<Vec<Word>>> {
    let op = engine
        .RecognizeAsync(bitmap)
        .context("RecognizeAsync failed")?;
    // The windows crate has no blocking await; polling Status keeps the
    // worker simple and is what all other .get() helpers do internally.
    loop {
        match op.Status().context("OCR status failed")? {
            AsyncStatus::Completed => break,
            AsyncStatus::Started => std::thread::sleep(Duration::from_millis(5)),
            other => bail!("OCR finished with {other:?}"),
        }
    }
    let result = op.GetResults().context("OCR results missing")?;
    let lines = result.Lines().context("OCR lines missing")?;
    let mut out: Vec<Vec<Word>> = Vec::new();
    for line_index in 0..lines.Size()? {
        let line = lines.GetAt(line_index)?;
        let line_words = line.Words()?;
        let mut words = Vec::new();
        for word_index in 0..line_words.Size()? {
            let word = line_words.GetAt(word_index)?;
            let text = word.Text()?.to_string();
            if text.is_empty() {
                continue;
            }
            let r = word.BoundingRect()?;
            words.push(Word {
                text,
                line: out.len() as u32,
                rect: Rect {
                    l: r.X.round() as i32,
                    t: r.Y.round() as i32,
                    r: (r.X + r.Width).round() as i32,
                    b: (r.Y + r.Height).round() as i32,
                },
            });
        }
        if !words.is_empty() {
            out.push(words);
        }
    }
    logline(&format!(
        "ocr: {} -> {} lines, {} words",
        engine_tag(engine),
        out.len(),
        out.iter().map(Vec::len).sum::<usize>()
    ));
    Ok(out)
}

/// `zh`-model lines keep CJK; Latin-only lines take the matching English line
/// (vertical overlap >= half the smaller line height). An English line is
/// consumed at most once. Mixed-script lines stay with the primary engine.
fn merge_lines(primary: &[Vec<Word>], en: &[Vec<Word>]) -> Vec<Vec<Word>> {
    let mut used = vec![false; en.len()];
    let mut out = Vec::with_capacity(primary.len());
    for line in primary {
        if mostly_cjk(line) {
            out.push(line.clone());
            continue;
        }
        let Some((zt, zb)) = line_bounds(line) else {
            out.push(line.clone());
            continue;
        };
        let mut best: Option<(usize, i32)> = None;
        for (i, other) in en.iter().enumerate() {
            if used[i] {
                continue;
            }
            let Some((et, eb)) = line_bounds(other) else {
                continue;
            };
            let overlap = zb.min(eb) - zt.max(et);
            let min_h = (zb - zt).min(eb - et).max(1);
            if overlap > 0 && overlap * 2 >= min_h && best.is_none_or(|(_, o)| overlap > o) {
                best = Some((i, overlap));
            }
        }
        match best {
            Some((i, _)) => {
                used[i] = true;
                out.push(en[i].clone());
            }
            None => out.push(line.clone()),
        }
    }
    out
}

fn line_bounds(line: &[Word]) -> Option<(i32, i32)> {
    let top = line.iter().map(|w| w.rect.t).min()?;
    let bottom = line.iter().map(|w| w.rect.b).max()?;
    Some((top, bottom))
}

/// A line stays on the primary engine when CJK letters make up at least a
/// fifth of its characters; full-width punctuation or one stray OCR glyph
/// does not make a Latin line "CJK".
fn mostly_cjk(line: &[Word]) -> bool {
    let mut cjk = 0;
    let mut total = 0;
    for c in line.iter().flat_map(|w| w.text.chars()) {
        if c.is_whitespace() {
            continue;
        }
        total += 1;
        if crate::text::is_cjk_letter(c) {
            cjk += 1;
        }
    }
    cjk > 0 && cjk * 5 >= total
}

fn has_latin(lines: &[Vec<Word>]) -> bool {
    lines.iter().flatten().any(|w| {
        w.text
            .chars()
            .any(|c| c.is_ascii_alphanumeric() && !crate::text::is_cjk(c))
    })
}

/// Log a per-line preview; the strongest signal when tuning scale/language.
fn log_preview(tag: &str, lines: &[Vec<Word>]) {
    let text = lines
        .iter()
        .map(|line| {
            line.iter()
                .map(|w| w.text.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join(" | ");
    let mut preview = text;
    if preview.chars().count() > 240 {
        preview = preview.chars().take(240).collect::<String>() + "…";
    }
    logline(&format!("{tag} {preview:?}"));
}

unsafe fn engine_tag(engine: &OcrEngine) -> String {
    engine
        .RecognizerLanguage()
        .and_then(|l| l.LanguageTag())
        .map(|t| t.to_string())
        .unwrap_or_default()
}

/// Pick the recognizer: an explicitly requested language tag (exact, then
/// prefix), otherwise the auto ranking (CJK first, then English) so one
/// engine covers mixed Latin+CJK text. P0.4 replaces this with the router.
unsafe fn pick_engine(requested: Option<&str>) -> Result<OcrEngine> {
    let languages = OcrEngine::AvailableRecognizerLanguages()
        .context("no Windows OCR support on this system")?;
    let count = languages.Size()?;
    ensure!(count > 0, "no Windows OCR language pack installed");
    let req = requested
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty());
    let mut best: Option<(u8, Language)> = None;
    let mut exact: Option<Language> = None;
    let mut prefix: Option<Language> = None;
    for i in 0..count {
        let language = languages.GetAt(i)?;
        let tag = language.LanguageTag()?.to_string().to_ascii_lowercase();
        let rank = if tag.starts_with("zh") {
            0
        } else if tag.starts_with("en") {
            1
        } else {
            2
        };
        if best.as_ref().is_none_or(|(r, _)| rank < *r) {
            best = Some((rank, language.clone()));
        }
        if let Some(req) = req.as_deref() {
            if tag == req {
                exact = Some(language.clone());
            } else if prefix.is_none() && tag.starts_with(req) {
                prefix = Some(language.clone());
            }
        }
    }
    let chosen = exact.or(prefix).or_else(|| {
        if let Some(req) = req.as_deref() {
            logline(&format!(
                "warn: OCR language `{req}` is not installed; using auto"
            ));
        }
        best.map(|(_, language)| language)
    });
    let language = chosen.ok_or_else(|| anyhow!("no usable OCR language"))?;
    let engine = OcrEngine::TryCreateFromLanguage(&language).with_context(|| {
        format!(
            "OCR engine for {}",
            language
                .LanguageTag()
                .map(|t| t.to_string())
                .unwrap_or_default()
        )
    })?;
    ensure!(
        !engine.as_raw().is_null(),
        "no OCR recognizer for this language"
    );
    Ok(engine)
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

    #[test]
    fn merge_keeps_cjk_lines_and_takes_english_for_latin() {
        let zh = vec![
            vec![word("pack", 0, 0, 40, 12), word("my", 45, 0, 60, 12)],
            vec![word("小", 0, 20, 12, 32), word("字", 14, 20, 26, 32)],
        ];
        let en = vec![vec![word("Pack", 0, 0, 40, 12), word("my", 45, 0, 60, 12)]];
        let merged = merge_lines(&zh, &en);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0][0].text, "Pack");
        assert_eq!(merged[1][0].text, "小");
    }

    #[test]
    fn merge_leaves_latin_line_alone_without_a_match() {
        let zh = vec![vec![word("pack", 0, 100, 40, 112)]];
        let en = vec![vec![word("Pack", 0, 0, 40, 12)]];
        let merged = merge_lines(&zh, &en);
        assert_eq!(merged[0][0].text, "pack");
    }

    #[test]
    fn merge_consumes_an_english_line_once() {
        let zh = vec![
            vec![word("one", 0, 0, 30, 12)],
            vec![word("two", 0, 4, 30, 16)],
        ];
        let en = vec![vec![word("ONE", 0, 0, 30, 12)]];
        let merged = merge_lines(&zh, &en);
        assert_eq!(merged[0][0].text, "ONE");
        assert_eq!(merged[1][0].text, "two");
    }

    #[test]
    fn merge_replaces_latin_line_with_a_stray_cjk_glyph() {
        let zh = vec![vec![
            word("The", 0, 0, 20, 12),
            word("quick", 25, 0, 60, 12),
            word("翮", 65, 0, 75, 12),
            word("fox", 80, 0, 100, 12),
        ]];
        let en = vec![vec![
            word("The", 0, 0, 20, 12),
            word("quick", 25, 0, 60, 12),
            word("fox", 80, 0, 100, 12),
        ]];
        let merged = merge_lines(&zh, &en);
        assert_eq!(merged[0].len(), 3);
    }

    #[test]
    fn merge_keeps_a_mixed_line() {
        let zh = vec![vec![
            word("OCR", 0, 0, 20, 12),
            word("测", 25, 0, 35, 12),
            word("试", 36, 0, 46, 12),
            word("123", 50, 0, 70, 12),
        ]];
        let en = vec![vec![word("OCR", 0, 0, 20, 12), word("123", 50, 0, 70, 12)]];
        let merged = merge_lines(&zh, &en);
        assert_eq!(merged[0].len(), 4);
    }

    #[test]
    fn merge_handles_empty_english_pass() {
        let zh = vec![vec![word("pack", 0, 0, 40, 12)]];
        let merged = merge_lines(&zh, &[]);
        assert_eq!(merged[0][0].text, "pack");
    }
}
