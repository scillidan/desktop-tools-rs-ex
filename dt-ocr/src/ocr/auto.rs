//! P0.5 automatic refresh. Sampling happens on the existing host timer; OCR
//! stays on its worker thread. A manual capture arms the loop, so merely
//! starting dt-ocr or showing a frame never starts recognition.

use crate::config::OcrAuto;
use crate::logline;
use crate::ocr::Snapshot;
use anyhow::Result;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, RECT};

const SAMPLE_W: usize = 48;
const SAMPLE_H: usize = 27;
const PIXEL_DELTA: u8 = 32;
const CHANGE_FRACTION: f32 = 0.005;
const MIN_REFRESH: Duration = Duration::from_millis(500);

struct State {
    config: OcrAuto,
    frame: isize,
    baseline: Vec<u8>,
    pending_changes: u8,
    busy: bool,
    last_start: Option<Instant>,
}

impl State {
    const fn new() -> Self {
        Self {
            config: OcrAuto {
                every_ms: None,
                on_change: false,
            },
            frame: 0,
            baseline: Vec::new(),
            pending_changes: 0,
            busy: false,
            last_start: None,
        }
    }
}

static STATE: Mutex<State> = Mutex::new(State::new());

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// A user dismissal leaves the configured auto mode intact but disarms this
/// frame until the next manual OCR. Internal stale-mask dismissal must not call
/// this, or a change-triggered refresh would cancel itself.
pub fn disarm() {
    let mut s = state();
    s.frame = 0;
    s.baseline.clear();
    s.pending_changes = 0;
}

pub fn set_auto(config: OcrAuto) {
    let mut s = state();
    s.config = config;
    s.pending_changes = 0;
}

pub(super) fn before_start(frame: HWND, snapshot: &Snapshot) {
    let baseline = unsafe { crate::frame::visible_rect(frame) }
        .and_then(|rect| unsafe { sample(rect).ok() })
        .unwrap_or_else(|| fingerprint(snapshot));
    let mut s = state();
    s.frame = frame.0 as isize;
    s.baseline = baseline;
    s.pending_changes = 0;
    s.busy = true;
    s.last_start = Some(Instant::now());
}

pub(crate) fn finished(frame: HWND) {
    let baseline =
        unsafe { crate::frame::visible_rect(frame) }.and_then(|rect| unsafe { sample(rect).ok() });
    let mut s = state();
    if s.frame == frame.0 as isize {
        if let Some(baseline) = baseline {
            s.baseline = baseline;
        }
        s.busy = false;
    }
}

/// Host timer entry point. It only samples after a manual capture armed it.
unsafe fn sample(rect: RECT) -> Result<Vec<u8>> {
    // shortcut: on-change needs one containing source window; multi-window
    // regions can use every_ms, upgrade to per-window compositing if needed.
    let (width, height, bgra) = crate::capture::print_source(rect)?;
    let sampled = fingerprint(&Snapshot {
        width,
        height,
        bgra,
    });
    let (min, max) = sampled
        .iter()
        .fold((u8::MAX, u8::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
    anyhow::ensure!(
        max.saturating_sub(min) > 2,
        "PrintWindow returned a blank frame"
    );
    Ok(sampled)
}

/// Host timer entry point. It only samples after a manual capture armed it.
pub unsafe fn auto_tick() {
    let (config, frame, every_due) = {
        let s = state();
        if !s.config.enabled() || s.frame == 0 || s.busy {
            return;
        }
        let elapsed = s.last_start.map(|t| t.elapsed()).unwrap_or_default();
        let every_due = s
            .config
            .every_ms
            .is_some_and(|ms| elapsed >= Duration::from_millis(ms));
        if !every_due && (!s.config.on_change || elapsed < MIN_REFRESH) {
            return;
        }
        (s.config, HWND(s.frame as *mut core::ffi::c_void), every_due)
    };
    let Some(rect) = crate::frame::visible_rect(frame) else {
        return;
    };
    if every_due {
        logline("ocr: auto refresh (interval)");
        crate::mask::dismiss();
        if let Err(e) = super::start(frame) {
            logline(&format!("warn: auto OCR failed: {e:#}"));
        }
        return;
    }
    if !config.on_change {
        return;
    }
    // Fallback for pre-2004 Windows: do not ever OCR our own mask. Newer
    // systems exclude it through WDA_EXCLUDEFROMCAPTURE without hiding it.
    if crate::mask::visible_without_capture_exclusion() {
        crate::mask::dismiss();
    }
    let sampled = sample(rect);
    let changed = match sampled {
        Ok(current) => {
            let mut s = state();
            if s.frame != frame.0 as isize || s.busy {
                return;
            }
            let changed = materially_changed(&s.baseline, &current);
            if changed && s.pending_changes == 0 {
                // The old result is stale now, not two samples from now. This
                // internal dismissal deliberately leaves auto armed.
                crate::mask::dismiss();
                logline("ocr: auto source changing; stale mask dismissed");
            }
            s.pending_changes = if changed {
                s.pending_changes.saturating_add(1)
            } else {
                0
            };
            if s.pending_changes < 2 {
                return;
            }
            s.pending_changes = 0;
            changed
        }
        Err(e) => {
            logline(&format!("warn: auto sample failed: {e:#}"));
            return;
        }
    };
    if changed {
        logline("ocr: auto refresh (source changed)");
        crate::mask::dismiss();
        if let Err(e) = super::start(frame) {
            logline(&format!("warn: auto OCR failed: {e:#}"));
        }
    }
}

/// Fixed-size luma samples: cheap enough for a 250 ms tick and insensitive to
/// a one-pixel caret. OCR still receives the full-resolution capture.
fn fingerprint(snapshot: &Snapshot) -> Vec<u8> {
    let mut out = Vec::with_capacity(SAMPLE_W * SAMPLE_H);
    if snapshot.width <= 0 || snapshot.height <= 0 {
        return out;
    }
    for sy in 0..SAMPLE_H {
        let y = (sy * snapshot.height as usize / SAMPLE_H) as i32;
        for sx in 0..SAMPLE_W {
            let x = (sx * snapshot.width as usize / SAMPLE_W) as i32;
            let i = ((y * snapshot.width + x) * 4) as usize;
            if i + 2 < snapshot.bgra.len() {
                let b = snapshot.bgra[i] as u32;
                let g = snapshot.bgra[i + 1] as u32;
                let r = snapshot.bgra[i + 2] as u32;
                out.push(((b + g * 2 + r) / 4) as u8);
            }
        }
    }
    out
}

fn change_fraction(before: &[u8], after: &[u8]) -> f32 {
    if before.len() != after.len() || before.is_empty() {
        return 0.0;
    }
    let changed = before
        .iter()
        .zip(after)
        .filter(|(a, b)| a.abs_diff(**b) > PIXEL_DELTA)
        .count();
    changed as f32 / before.len() as f32
}

fn materially_changed(before: &[u8], after: &[u8]) -> bool {
    change_fraction(before, after) >= CHANGE_FRACTION
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shot(width: i32, height: i32, value: u8) -> Snapshot {
        Snapshot {
            width,
            height,
            bgra: vec![value; (width * height * 4) as usize],
        }
    }

    #[test]
    fn source_change_uses_hysteresis_threshold() {
        let before = shot(48, 27, 0);
        let mut after = shot(48, 27, 0);
        assert!(!materially_changed(
            &fingerprint(&before),
            &fingerprint(&after)
        ));
        // A tiny caret-sized change must not trigger OCR.
        for i in 0..2 {
            after.bgra[i * 4] = 255;
            after.bgra[i * 4 + 1] = 255;
            after.bgra[i * 4 + 2] = 255;
        }
        assert!(!materially_changed(
            &fingerprint(&before),
            &fingerprint(&after)
        ));
        after.bgra.fill(255);
        assert!(materially_changed(
            &fingerprint(&before),
            &fingerprint(&after)
        ));
    }

    #[test]
    fn auto_is_disabled_by_default() {
        assert!(!OcrAuto::default().enabled());
        assert!(OcrAuto {
            every_ms: None,
            on_change: true
        }
        .enabled());
    }
}
