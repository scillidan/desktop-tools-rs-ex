//! Rect recording through ffmpeg's `gdigrab` input.
//!
//! One session at a time. `start` spawns a hidden ffmpeg with a piped stdin;
//! `stop` writes `q` (ffmpeg's graceful quit) and waits briefly before killing.
//! `shortcut: gdigrab instead of Desktop Duplication — it is far simpler and
//! fine for a rect, upgrade to DDA when full-screen recording needs the frame
//! rate`.

use anyhow::{bail, Context, Result};
use std::io::Write;
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::Instant;

use dt_common::layout::Rect;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

struct Session {
    child: Child,
    path: std::path::PathBuf,
    started: Instant,
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);

pub fn is_recording() -> bool {
    SESSION
        .lock()
        .map(|s| s.is_some())
        .unwrap_or_else(|e| e.into_inner().is_some())
}

/// Record `rect` at `fps` into `dir` with `ffmpeg` (PATH name or absolute).
pub fn start(rect: Rect, dir: &str, fps: u32, ffmpeg: &str) -> Result<()> {
    let mut slot = SESSION
        .lock()
        .map_err(|_| anyhow::anyhow!("recording state poisoned"))?;
    if slot.is_some() {
        bail!("already recording");
    }
    if ffmpeg.trim().is_empty() {
        bail!("[record] ffmpeg is empty");
    }
    let dir = crate::capture::expand_env(dir);
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {dir}"))?;
    let path = std::path::Path::new(&dir).join(crate::capture::record_name());
    let size = format!("{}x{}", rect.w(), rect.h());
    let fps = fps.clamp(5, 60).to_string();
    let x = rect.l.to_string();
    let y = rect.t.to_string();
    let child = Command::new(ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "gdigrab",
            "-framerate",
            &fps,
            "-offset_x",
            &x,
            "-offset_y",
            &y,
            "-video_size",
            &size,
            "-i",
            "desktop",
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-pix_fmt",
            "yuv420p",
            "-y",
        ])
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .with_context(|| format!("cannot start {ffmpeg}"))?;
    crate::logline(&format!(
        "record: start {} fps={fps} {rect:?}",
        path.display()
    ));
    *slot = Some(Session {
        child,
        path,
        started: Instant::now(),
    });
    Ok(())
}

/// Stop the running session; does nothing when idle.
pub fn stop() -> Result<()> {
    let mut slot = SESSION
        .lock()
        .map_err(|_| anyhow::anyhow!("recording state poisoned"))?;
    let Some(mut session) = slot.take() else {
        return Ok(());
    };
    if let Some(mut stdin) = session.child.stdin.take() {
        let _ = stdin.write_all(b"q");
        let _ = stdin.flush();
    }
    let mut exited = false;
    for _ in 0..100 {
        if session.child.try_wait().ok().flatten().is_some() {
            exited = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if !exited {
        let _ = session.child.kill();
        let _ = session.child.wait();
    }
    let secs = session.started.elapsed().as_secs_f32();
    crate::logline(&format!(
        "record: stop {} ({secs:.1}s)",
        session.path.display()
    ));
    Ok(())
}
