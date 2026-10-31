//! dt-ocr — OCR frame window + overlay (Text-Grab model).
//!
//! P0.2: a borderless always-on-top hollow frame marks a fixed OCR region.
//! Draw it with the draw binding, resize it by typed SizeSpec, pin it
//! click-through, and capture its region to the clipboard. OCR lands in P1.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod capture;
mod config;
mod desktops;
mod frame;
mod host;
mod hotkeys;
mod icon;
mod interaction;
mod mask;
mod menu;
mod ocr;
mod overlay;
mod pin;
mod text;
mod tray;

use anyhow::Result;
use std::io::Write;
use std::time::SystemTime;

use config::Action;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwareness, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, PROCESS_DPI_AWARENESS,
};

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            // `{:#}` is anyhow's Display chain, not Debug.
            eprintln!("error: {e:#}");
            logline(&format!("error: {e:#}"));
            std::process::ExitCode::FAILURE
        }
    }
}

fn set_dpi_aware() {
    // Provenance: desktop-tools-rs src/win32.rs set_dpi_aware.
    unsafe {
        for ctx in [
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE,
        ] {
            if SetProcessDpiAwarenessContext(ctx).is_ok() {
                return;
            }
        }
        // Fallback for pre-Win10: system-DPI-aware beats nothing.
        let _ = SetProcessDpiAwareness(PROCESS_DPI_AWARENESS(1));
    }
}

fn run() -> Result<()> {
    set_dpi_aware();
    unsafe {
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
    }
    // One resident daemon only. Several instances each own a frame and cannot
    // register the same hotkeys; quitting the tray of one leaves the others
    // running, which looks like the app refuses to exit.
    let _single = unsafe {
        use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
        use windows::Win32::System::Threading::CreateMutexW;
        match CreateMutexW(None, true, windows::core::w!("Local\\dt-ocr.single")) {
            Ok(handle) => {
                if GetLastError() == ERROR_ALREADY_EXISTS {
                    logline("another dt-ocr instance is already running; exiting");
                    return Ok(());
                }
                Some(handle)
            }
            Err(e) => {
                logline(&format!("warn: single-instance mutex unavailable: {e}"));
                None
            }
        }
    };
    let cfg_path = dt_common::config::exe_dir_file(config::CONFIG_NAME)?;
    let text = dt_common::config::load_or_create(&cfg_path, include_str!("../../dt-ocr.toml"))?;
    let mut cfg = config::parse(&text)?;
    ocr::set_engine(cfg.ocr_engine);
    ocr::set_auto(cfg.ocr_auto);
    mask::set_fill(cfg.mask_fill);
    mask::set_style(cfg.mask_font.clone(), cfg.mask_size, cfg.mask_layout);
    set_capture_dir(&cfg.capture_dir);
    set_pin_style(cfg.pin_border, cfg.pin_line_color, cfg.pin_hatch_color);
    logline(&format!(
        "config loaded from {} (frame {}x{})",
        cfg_path.display(),
        cfg.frame_w,
        cfg.frame_h
    ));

    let host = host::Host::new()?;
    let mut frames = desktops::Frames::new(&cfg)?;
    let _tray = match tray::Tray::new(host.hwnd()) {
        Ok(tray) => Some(tray),
        Err(e) => {
            logline(&format!("warn: tray unavailable: {e:#}"));
            None
        }
    };

    let mut hotkeys = register_hotkeys(host.hwnd(), &cfg);
    let mut mtime = dt_common::config::modified(&cfg_path);

    // 1 s timer drives config hot-reload (bindings only; the desktop-tools-rs
    // pattern — mtime polling, no notify dependency).
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{SetTimer, TIMERPROC};
        let _ = SetTimer(Some(host.hwnd()), 1, 250, Some(TIMERPROC::None));
    }

    let mut msg = windows::Win32::UI::WindowsAndMessaging::MSG::default();
    let mut last_config_poll = std::time::Instant::now();
    loop {
        let got =
            unsafe { windows::Win32::UI::WindowsAndMessaging::GetMessageW(&mut msg, None, 0, 0) };
        if !got.as_bool() {
            break; // WM_QUIT
        }
        if msg.message == windows::Win32::UI::WindowsAndMessaging::WM_HOTKEY {
            if let Some(action) = hotkeys.action_of(msg.wParam.0 as i32) {
                // MOD_NOREPEAT already suppresses held-key repeats; this also
                // swallows duplicate injected messages before they stack pins.
                let mut last = LAST_HOTKEY
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner());
                let repeated = hotkey_repeat(&mut last, action, std::time::Instant::now());
                drop(last);
                if repeated {
                    logline(&format!("hotkey repeat ignored: {action:?}"));
                    continue;
                }
                logline(&format!("hotkey: {action:?}"));
                match frames.current(&cfg) {
                    Ok(current) => dispatch(action, current),
                    Err(e) => logline(&format!("warn: cannot select desktop frame: {e:#}")),
                }
            }
        }
        if msg.message == windows::Win32::UI::WindowsAndMessaging::WM_TIMER
            && msg.wParam.0 == 1
            && last_config_poll.elapsed() >= std::time::Duration::from_secs(1)
        {
            last_config_poll = std::time::Instant::now();
            let now = dt_common::config::modified(&cfg_path);
            if now != mtime {
                mtime = now;
                match reload(&cfg_path) {
                    Ok(new_cfg) => {
                        logline("config reloaded, hotkeys re-registered");
                        ocr::set_engine(new_cfg.ocr_engine);
                        ocr::set_auto(new_cfg.ocr_auto);
                        hotkeys.unregister_all(host.hwnd());
                        hotkeys = register_hotkeys(host.hwnd(), &new_cfg);
                        for &current in frames.all() {
                            unsafe {
                                frame::set_prefs(
                                    current,
                                    new_cfg.work_area,
                                    new_cfg.show_hint,
                                    new_cfg.font.clone(),
                                    new_cfg.ocr_scale,
                                    new_cfg.ocr_language.clone(),
                                )
                            };
                        }
                        mask::set_fill(new_cfg.mask_fill);
                        set_capture_dir(&new_cfg.capture_dir);
                        set_pin_style(
                            new_cfg.pin_border,
                            new_cfg.pin_line_color,
                            new_cfg.pin_hatch_color,
                        );
                        mask::set_style(
                            new_cfg.mask_font.clone(),
                            new_cfg.mask_size,
                            new_cfg.mask_layout,
                        );
                        cfg = new_cfg;
                    }
                    Err(e) => logline(&format!("config reload failed, keeping old: {e:#}")),
                }
            }
        }
        unsafe {
            if msg.message == windows::Win32::UI::WindowsAndMessaging::WM_TIMER && msg.wParam.0 == 1
            {
                ocr::auto_tick();
            }
            let _ = windows::Win32::UI::WindowsAndMessaging::TranslateMessage(&msg);
            windows::Win32::UI::WindowsAndMessaging::DispatchMessageW(&msg);
        }
    }
    hotkeys.unregister_all(host.hwnd());
    unsafe { windows::Win32::System::Com::CoUninitialize() };
    logline("message loop exited");
    Ok(())
}

fn register_hotkeys(hwnd: HWND, cfg: &config::Config) -> hotkeys::Hotkeys {
    let (hk, failed) = hotkeys::Hotkeys::register(hwnd, &cfg.bindings);
    for f in &failed {
        logline(&format!("warn: hotkey not registered: {f}"));
    }
    if hk.is_empty() {
        logline("warn: no hotkeys registered; running frame-only");
    }
    hk
}

/// Hot-reload re-reads the file; an unreadable or binding-less result keeps
/// the old config, mirroring the desktop-tools-rs daemon.
fn reload(cfg_path: &std::path::Path) -> Result<config::Config> {
    let text = std::fs::read_to_string(cfg_path)?;
    config::parse(&text)
}

fn dispatch(action: Action, frame: HWND) {
    // Drawing replaces the old region, so hide it instead of flashing it; every
    // other command summons a frame that was closed from the taskbar.
    unsafe {
        match action {
            Action::Draw => {
                frame::hide(frame);
                mask::dismiss();
            }
            // The pin only uses a frame that is already on screen; bringing a
            // hidden one back would pin the wrong thing.
            Action::PinShot => {}
            _ => frame::ensure_visible(frame),
        }
    }
    match action {
        Action::Draw => {
            if let Err(e) = unsafe { overlay::begin() } {
                logline(&format!("draw failed: {e:#}"));
            }
        }
        Action::Resize => unsafe { frame::begin_spec(frame) },
        Action::Pin => unsafe { frame::toggle_pin(frame) },
        Action::Move => unsafe { frame::to_move(frame) },
        Action::Toggle => unsafe { mask::toggle() },
        Action::Capture => {
            // Read the real screen: a previous mask would be re-captured.
            unsafe { mask::dismiss() };
            if let Err(e) = unsafe { capture::capture_frame(frame) } {
                logline(&format!("capture failed: {e:#}"));
            } else {
                unsafe { frame::flash_capture(frame) };
                // P0.3: copy the image (above) AND read the words into the
                // overlay. OCR runs on a worker thread; the frame is
                // click-through until the mask takes over.
                if let Err(e) = unsafe { ocr::start(frame) } {
                    logline(&format!("ocr capture failed: {e:#}"));
                }
            }
        }
        // Frame the window the user is looking at: the frame becomes the region
        // for OCR/capture without a manual drag.
        Action::FrameWindow => match unsafe { capture::window_target() } {
            Ok(rect) => unsafe {
                mask::dismiss();
                frame::place(frame, frame::rect_of(rect));
                frame::to_move(frame);
                logline(&format!("frame: window at {rect:?}"));
            },
            Err(e) => logline(&format!("frame window failed: {e:#}")),
        },
        // Pin what the frame covers; without a frame, pin the focused window.
        Action::PinShot => {
            let r = unsafe {
                match frame::visible_rect(frame) {
                    Some(rect) => {
                        // Hide first: our own border must not be in the image.
                        frame::hide(frame);
                        let r = capture::pin_rect(rect, pin_style());
                        if r.is_err() {
                            frame::ensure_visible(frame);
                        }
                        r
                    }
                    None => capture::pin_window(pin_style()),
                }
            };
            if let Err(e) = r {
                logline(&format!("pin shot failed: {e:#}"));
            }
        }
    }
}

/// Focused-frame Ctrl+Shift+S: straight to `[capture] dir`.
pub(crate) fn frame_quick_save(frame: HWND) {
    if let Err(e) = unsafe { capture::save_frame(frame, &capture_dir()) } {
        logline(&format!("save frame failed: {e:#}"));
    }
}

/// Focused-frame Ctrl+S: pick the folder first, then save. A modal dialog
/// leaves an animation behind, so let it fade before grabbing.
pub(crate) fn frame_save_as(frame: HWND) {
    let dir = match unsafe { capture::choose_dir(frame) } {
        Ok(Some(dir)) => dir,
        Ok(None) => return,
        Err(e) => {
            logline(&format!("folder picker failed: {e:#}"));
            return;
        }
    };
    std::thread::sleep(std::time::Duration::from_millis(220));
    if let Err(e) = unsafe { capture::save_frame(frame, &dir.to_string_lossy()) } {
        logline(&format!("save frame failed: {e:#}"));
    }
}

pub(crate) fn frame_copy(frame: HWND) {
    if let Err(e) = unsafe { capture::copy_frame(frame) } {
        logline(&format!("copy frame failed: {e:#}"));
    }
}

pub(crate) fn frame_pin(frame: HWND) {
    let r = unsafe {
        match frame::visible_rect(frame) {
            Some(rect) => {
                frame::hide(frame);
                let r = capture::pin_rect(rect, pin_style());
                if r.is_err() {
                    frame::ensure_visible(frame);
                }
                r
            }
            None => return,
        }
    };
    if let Err(e) = r {
        logline(&format!("pin frame failed: {e:#}"));
    }
}

static LAST_HOTKEY: std::sync::Mutex<Option<(Action, std::time::Instant)>> =
    std::sync::Mutex::new(None);

/// `MOD_NOREPEAT` covers a held key; this also drops duplicate injected
/// `WM_HOTKEY`s that arrive back-to-back and would stack capture/pin windows.
fn hotkey_repeat(
    last: &mut Option<(Action, std::time::Instant)>,
    action: Action,
    now: std::time::Instant,
) -> bool {
    if let Some((prev, at)) = *last {
        if prev == action && now.duration_since(at).as_millis() < 150 {
            return true;
        }
    }
    *last = Some((action, now));
    false
}

static CAPTURE_DIR: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

fn capture_dir() -> String {
    CAPTURE_DIR
        .lock()
        .map(|d| d.clone())
        .unwrap_or_else(|e| e.into_inner().clone())
}

pub fn set_capture_dir(dir: &str) {
    if let Ok(mut d) = CAPTURE_DIR.lock() {
        *d = dir.to_string();
    }
}

static PIN_STYLE: std::sync::Mutex<pin::Style> = std::sync::Mutex::new(pin::Style::DEFAULT);

fn set_pin_style(border: config::PinBorder, line_color: u32, hatch_color: u32) {
    if let Ok(mut style) = PIN_STYLE.lock() {
        *style = pin::Style {
            border,
            line_color,
            hatch_color,
        };
    }
}

fn pin_style() -> pin::Style {
    PIN_STYLE
        .lock()
        .map(|style| *style)
        .unwrap_or(pin::Style::DEFAULT)
}

static FRAME_HWND: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

pub fn frame_hwnd() -> HWND {
    HWND(FRAME_HWND.load(std::sync::atomic::Ordering::SeqCst) as *mut core::ffi::c_void)
}

fn set_frame_hwnd(hwnd: HWND) {
    FRAME_HWND.store(hwnd.0 as isize, std::sync::atomic::Ordering::SeqCst);
}

/// Append-only log next to the exe (release builds have no console).
/// Timestamps are plain epoch seconds — no chrono dependency for a liveness log.
fn logline(msg: &str) {
    let Ok(path) = dt_common::config::exe_dir_file("dt-ocr.log") else {
        return;
    };
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let secs = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(f, "[{secs}] {msg}");
    }
}

#[cfg(test)]
mod tests {
    use super::hotkey_repeat;
    use crate::config::Action;
    use std::time::{Duration, Instant};

    #[test]
    fn duplicate_hotkeys_inside_the_gate_are_dropped() {
        let mut last = None;
        let t = Instant::now();
        assert!(!hotkey_repeat(&mut last, Action::PinShot, t));
        assert!(hotkey_repeat(
            &mut last,
            Action::PinShot,
            t + Duration::from_millis(40)
        ));
        assert!(!hotkey_repeat(
            &mut last,
            Action::PinShot,
            t + Duration::from_millis(200)
        ));
        // A different action is never treated as a repeat.
        assert!(!hotkey_repeat(
            &mut last,
            Action::Draw,
            t + Duration::from_millis(210)
        ));
    }
}
