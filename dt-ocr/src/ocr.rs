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
}

pub struct Outcome {
    /// The frame rect the words belong to (the mask window's screen rect).
    pub rect: Rect,
    pub words: Result<Vec<Word>>,
}

/// Frame border thickness cropped from the capture: the 2 px GDI ring is not
/// content, and cropping it keeps the border out of the OCR image.
const BORDER: i32 = 2;

/// Capture the frame region, mark the frame click-through and start the
/// worker. Returns immediately; the result arrives as [`WM_OCR_DONE`].
pub unsafe fn start(frame: HWND) -> Result<()> {
    let rect = crate::frame::screen_rect(frame);
    ensure!(
        rect.w() > BORDER * 2 && rect.h() > BORDER * 2,
        "frame too small"
    );
    let capture = dt_common::layout::inset(rect, BORDER);
    let (w, h, pixels) = capture_bgra(capture)?;
    crate::frame::set_ocr_running(frame, true);
    logline(&format!(
        "ocr: started on {w}x{h} at ({},{})",
        capture.l, capture.t
    ));
    let frame_addr = frame.0 as isize;
    std::thread::spawn(move || {
        let words = recognize(w, h, &pixels).map(|mut words| {
            for word in &mut words {
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

/// Screen pixels of `rect` as top-down BGRA (the layout SoftwareBitmap
/// expects; GDI DIBs default to bottom-up).
unsafe fn capture_bgra(rect: Rect) -> Result<(i32, i32, Vec<u8>)> {
    let (w, h) = (rect.w(), rect.h());
    let screen = GetDC(None);
    ensure!(!screen.0.is_null(), "GetDC(screen) failed");
    let mem = CreateCompatibleDC(Some(screen));
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
    let old = SelectObject(mem, bmp.into());
    let ok = BitBlt(mem, 0, 0, w, h, Some(screen), rect.l, rect.t, SRCCOPY).is_ok();
    let pixels = if ok && !bits.is_null() {
        Some(
            std::slice::from_raw_parts(bits as *const u8, (w as usize) * (h as usize) * 4).to_vec(),
        )
    } else {
        None
    };
    let _ = SelectObject(mem, old);
    let _ = DeleteObject(bmp.into());
    let _ = DeleteDC(mem);
    let _ = ReleaseDC(None, screen);
    let pixels = pixels.context("BitBlt failed")?;
    Ok((w, h, pixels))
}

/// Runs on the worker thread: MTA init, OCR, word boxes.
fn recognize(w: i32, h: i32, bgra: &[u8]) -> Result<Vec<Word>> {
    unsafe {
        // A fresh thread has no apartment yet; S_FALSE is fine, a different
        // apartment mode would be a bug here but not worth failing over.
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
        let max = OcrEngine::MaxImageDimension().unwrap_or(0);
        ensure!(
            max == 0 || (w as u32 <= max && h as u32 <= max),
            "capture {w}x{h} exceeds the OCR limit {max}px"
        );
        let engine = pick_engine()?;
        let len = (w as usize) * (h as usize) * 4;
        let buffer = Buffer::Create(len as u32).context("Buffer::Create failed")?;
        buffer
            .SetLength(len as u32)
            .context("Buffer::SetLength failed")?;
        let access: IBufferByteAccess = buffer.cast().context("IBufferByteAccess missing")?;
        let dst = access
            .Buffer()
            .context("IBufferByteAccess::Buffer failed")?;
        std::ptr::copy_nonoverlapping(bgra.as_ptr(), dst, len);
        let bitmap = SoftwareBitmap::CreateCopyFromBuffer(&buffer, BitmapPixelFormat::Bgra8, w, h)
            .context("SoftwareBitmap::CreateCopyFromBuffer failed")?;
        let op = engine
            .RecognizeAsync(&bitmap)
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
        let mut words = Vec::new();
        for line_index in 0..lines.Size()? {
            let line = lines.GetAt(line_index)?;
            let line_words = line.Words()?;
            for word_index in 0..line_words.Size()? {
                let word = line_words.GetAt(word_index)?;
                let text = word.Text()?.to_string();
                if text.is_empty() {
                    continue;
                }
                let r = word.BoundingRect()?;
                words.push(Word {
                    text,
                    rect: Rect {
                        l: r.X.round() as i32,
                        t: r.Y.round() as i32,
                        r: (r.X + r.Width).round() as i32,
                        b: (r.Y + r.Height).round() as i32,
                    },
                });
            }
        }
        logline(&format!(
            "ocr: {} words, {} lines (engine {})",
            words.len(),
            lines.Size()?,
            engine
                .RecognizerLanguage()
                .and_then(|l| l.LanguageTag())
                .map(|t| t.to_string())
                .unwrap_or_default()
        ));
        Ok(words)
    }
}

/// First installed recognizer language, preferring CJK then English so one
/// engine covers mixed Latin+CJK text. P0.4 replaces this with the router.
unsafe fn pick_engine() -> Result<OcrEngine> {
    let languages = OcrEngine::AvailableRecognizerLanguages()
        .context("no Windows OCR support on this system")?;
    let count = languages.Size()?;
    ensure!(count > 0, "no Windows OCR language pack installed");
    let mut best: Option<(u8, Language)> = None;
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
            best = Some((rank, language));
        }
    }
    let (_, language) = best.ok_or_else(|| anyhow!("no usable OCR language"))?;
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
