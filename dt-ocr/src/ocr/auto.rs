//! P0.5 automatic refresh. Sampling happens on the existing host timer; OCR
//! stays on its worker thread. Automatic mode is armed only by the
//! `ctrl+shift+alt+o` action; drawing or moving a frame never starts it.

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

struct State {
    config: OcrAuto,
    armed: bool,
    frame: isize,
    baseline: Vec<u8>,
    changed_at: Option<Instant>,
    busy: bool,
    last_start: Option<Instant>,
    sample_failed: bool,
}

impl State {
    const fn new() -> Self {
        Self {
            config: OcrAuto { delay_ms: 1000 },
            armed: false,
            frame: 0,
            baseline: Vec::new(),
            changed_at: None,
            busy: false,
            last_start: None,
            sample_failed: false,
        }
    }
}

static STATE: Mutex<State> = Mutex::new(State::new());

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn set_auto(config: OcrAuto) {
    let mut s = state();
    s.config = config;
    s.changed_at = None;
}

pub fn auto_enabled() -> bool {
    state().armed
}

/// Arm automatic OCR for `frame`; the next tick establishes the baseline.
pub fn enable_auto(frame: HWND) {
    let mut s = state();
    s.armed = true;
    s.frame = frame.0 as isize;
    s.baseline.clear();
    s.changed_at = None;
    s.busy = false;
    s.sample_failed = false;
}

pub fn disable_auto() {
    let mut s = state();
    s.armed = false;
    s.frame = 0;
    s.baseline.clear();
    s.changed_at = None;
    s.sample_failed = false;
}

pub(super) fn before_start(frame: HWND, snapshot: &Snapshot) {
    let baseline = unsafe { crate::frame::visible_rect(frame) }
        .and_then(|rect| unsafe { sample(rect).ok() })
        .unwrap_or_else(|| fingerprint(snapshot));
    let mut s = state();
    s.frame = frame.0 as isize;
    s.baseline = baseline;
    s.changed_at = None;
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
    }
    s.busy = false;
    s.changed_at = None;
}

/// Host timer entry point. It only samples after a manual capture armed it.
unsafe fn sample(rect: RECT) -> Result<Vec<u8>> {
    // shortcut: on-change needs one containing source window; multi-window
    // regions are sampled as one rect, upgrade to per-window compositing if needed.
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

/// Host timer entry point. Auto OCR is always active for a visible frame; the
/// first observed frame only establishes the comparison baseline.
pub unsafe fn auto_tick(frame: HWND) {
    if !auto_enabled() || frame.0.is_null() {
        return;
    }
    let Some(rect) = crate::frame::visible_rect(frame) else {
        let mut s = state();
        if s.frame == frame.0 as isize {
            s.frame = 0;
            s.baseline.clear();
            s.changed_at = None;
            s.sample_failed = false;
        }
        return;
    };
    let current = match sample(rect) {
        Ok(current) => {
            let mut s = state();
            s.sample_failed = false;
            drop(s);
            current
        }
        Err(e) => {
            let mut s = state();
            if !s.sample_failed {
                s.sample_failed = true;
                drop(s);
                logline(&format!("warn: auto sample failed: {e:#}"));
            }
            return;
        }
    };
    let (delay, due) = {
        let mut s = state();
        if s.busy {
            return;
        }
        if s.frame != frame.0 as isize || s.baseline.is_empty() {
            s.frame = frame.0 as isize;
            s.baseline = current;
            s.changed_at = None;
            return;
        }
        let changed = materially_changed(&s.baseline, &current);
        if !changed {
            s.changed_at = None;
            return;
        }
        let now = Instant::now();
        if s.changed_at.is_none() {
            // The old result is stale now, not after the debounce. This
            // internal dismissal leaves auto active.
            s.changed_at = Some(now);
            drop(s);
            crate::mask::dismiss();
            logline("ocr: auto source changing; stale mask dismissed");
            let s = state();
            if s.busy {
                return;
            }
            let delay = Duration::from_millis(s.config.delay_ms.max(100));
            (delay, false)
        } else {
            let delay = Duration::from_millis(s.config.delay_ms.max(100));
            let changed_for = s.changed_at.map(|t| t.elapsed()).unwrap_or_default();
            let since_ocr = s.last_start.map(|t| t.elapsed()).unwrap_or_default();
            if refresh_due(changed_for, since_ocr, delay) {
                s.changed_at = None;
                (delay, true)
            } else {
                (delay, false)
            }
        }
    };
    if due {
        logline(&format!(
            "ocr: auto refresh (source changed, delay {delay:?})"
        ));
        crate::frame::prepare_ocr(frame, super::Intent::Auto, false);
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

fn refresh_due(changed_for: Duration, since_ocr: Duration, delay: Duration) -> bool {
    changed_for >= delay && since_ocr >= delay
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
    fn auto_arms_only_on_demand() {
        disable_auto();
        assert!(!auto_enabled());
        enable_auto(HWND(std::ptr::null_mut()));
        assert!(auto_enabled());
        disable_auto();
        assert!(!auto_enabled());
    }

    #[test]
    fn auto_is_enabled_with_a_default_delay() {
        assert_eq!(OcrAuto::default().delay_ms, 1000);
    }

    #[test]
    fn refresh_waits_for_both_change_and_ocr_delay() {
        let delay = Duration::from_millis(1000);
        assert!(!refresh_due(
            Duration::from_millis(999),
            Duration::from_millis(2000),
            delay
        ));
        assert!(!refresh_due(
            Duration::from_millis(2000),
            Duration::from_millis(999),
            delay
        ));
        assert!(refresh_due(
            Duration::from_millis(1000),
            Duration::from_millis(1000),
            delay
        ));
    }
}
