//! dt-ocr — OCR frame window + overlay (Text-Grab model).
//!
//! P0.2: a borderless always-on-top hollow frame marks a fixed OCR region.
//! Draw it with the draw binding, resize it by typed SizeSpec, pin it
//! click-through, and capture its region to the clipboard. OCR lands in P1.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod capture;
mod config;
mod frame;
mod hotkeys;
mod interaction;
mod mask;
mod ocr;
mod overlay;
mod text;

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
    let cfg_path = dt_common::config::exe_dir_file(config::CONFIG_NAME)?;
    let text = dt_common::config::load_or_create(&cfg_path, include_str!("../../dt-ocr.toml"))?;
    let cfg = config::parse(&text)?;
    logline(&format!(
        "config loaded from {} (frame {}x{})",
        cfg_path.display(),
        cfg.frame_w,
        cfg.frame_h
    ));

    let frame =
        unsafe { frame::create(cfg.frame_w, cfg.frame_h, cfg.show_hint, cfg.font.clone())? };
    FRAME_HWND.store(frame.0 as isize, std::sync::atomic::Ordering::SeqCst);

    let mut hotkeys = register_hotkeys(frame, &cfg);
    let mut mtime = dt_common::config::modified(&cfg_path);

    // 1 s timer drives config hot-reload (bindings only; the desktop-tools-rs
    // pattern — mtime polling, no notify dependency).
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{SetTimer, TIMERPROC};
        let _ = SetTimer(Some(frame), 1, 1000, Some(TIMERPROC::None));
    }

    let mut msg = windows::Win32::UI::WindowsAndMessaging::MSG::default();
    loop {
        let got =
            unsafe { windows::Win32::UI::WindowsAndMessaging::GetMessageW(&mut msg, None, 0, 0) };
        if !got.as_bool() {
            break; // WM_QUIT
        }
        if msg.message == windows::Win32::UI::WindowsAndMessaging::WM_HOTKEY {
            if let Some(action) = hotkeys.action_of(msg.wParam.0 as i32) {
                logline(&format!("hotkey: {action:?}"));
                dispatch(action, frame);
            }
        }
        if msg.message == windows::Win32::UI::WindowsAndMessaging::WM_TIMER && msg.wParam.0 == 1 {
            let now = dt_common::config::modified(&cfg_path);
            if now != mtime {
                mtime = now;
                match reload(&cfg_path) {
                    Ok(new_cfg) => {
                        logline("config reloaded, hotkeys re-registered");
                        hotkeys.unregister_all(frame);
                        hotkeys = register_hotkeys(frame, &new_cfg);
                        unsafe { frame::set_prefs(frame, new_cfg.show_hint, new_cfg.font.clone()) };
                    }
                    Err(e) => logline(&format!("config reload failed, keeping old: {e:#}")),
                }
            }
        }
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::TranslateMessage(&msg);
            windows::Win32::UI::WindowsAndMessaging::DispatchMessageW(&msg);
        }
    }
    hotkeys.unregister_all(frame);
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
    match action {
        Action::Draw => {
            if let Err(e) = unsafe { overlay::begin() } {
                logline(&format!("draw failed: {e:#}"));
            }
        }
        Action::Resize => unsafe { frame::begin_spec(frame) },
        Action::Pin => unsafe { frame::toggle_pin(frame) },
        Action::Move => unsafe { frame::to_move(frame) },
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
    }
}

static FRAME_HWND: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

pub fn frame_hwnd() -> HWND {
    HWND(FRAME_HWND.load(std::sync::atomic::Ordering::SeqCst) as *mut core::ffi::c_void)
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
