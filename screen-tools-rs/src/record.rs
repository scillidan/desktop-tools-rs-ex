//! Rect recording through ffmpeg's `gdigrab` input plus WASAPI loopback audio.
//!
//! One session at a time. Video and audio are captured separately (ffmpeg's
//! Windows builds have no wasapi input), then muxed with one stream-copy pass at
//! stop. Safeguards: even-sized rect, free-space check before and during the
//! session, a duration cap, fragmented MP4 so a killed session stays playable.
//!
//! `shortcut: gdigrab instead of Desktop Duplication — it is far simpler and
//! fine for a rect, upgrade to DDA when full-screen recording needs the frame
//! rate`.

use anyhow::{bail, Context, Result};
use std::io::Write;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use dt_common::layout::Rect;
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Debug)]
pub struct Settings {
    pub dir: String,
    /// File name stem; `{date}`, `{time}`, `{datetime}` expand at start.
    pub name: String,
    pub fps: u32,
    pub ffmpeg: String,
    /// Capture the desktop (default playback device) audio track.
    pub audio: bool,
    /// x264 CRF; lower is bigger. Clamped to 16..=34.
    pub crf: u32,
    /// Stop automatically after this many minutes; 0 = unlimited.
    pub max_minutes: u32,
    /// Refuse to start, and stop early, below this much free space.
    pub min_free_mb: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            dir: String::new(),
            name: "screen-tools-rs-{date}-{time}".to_string(),
            fps: 30,
            ffmpeg: "ffmpeg".to_string(),
            audio: true,
            crf: 20,
            max_minutes: 60,
            min_free_mb: 500,
        }
    }
}

struct Session {
    child: Child,
    video: PathBuf,
    audio: Option<crate::audio::Capture>,
    final_path: PathBuf,
    ffmpeg: String,
    started: Instant,
    min_free_mb: u32,
    max_minutes: u32,
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);
static WATCHDOG: AtomicBool = AtomicBool::new(false);

pub fn is_recording() -> bool {
    SESSION
        .lock()
        .map(|s| s.is_some())
        .unwrap_or_else(|e| e.into_inner().is_some())
}

/// Why a running session should stop on its own, if it should.
fn auto_stop_reason(
    elapsed_min: u64,
    max_minutes: u32,
    free: Option<u64>,
    min_free_mb: u32,
) -> Option<String> {
    if max_minutes > 0 && elapsed_min >= max_minutes as u64 {
        return Some(format!("reached [record] max_minutes = {max_minutes}"));
    }
    match free {
        Some(free) if free < min_free_mb as u64 => Some(format!(
            "free space dropped to {free} MB (minimum {min_free_mb})"
        )),
        _ => None,
    }
}

fn lock() -> std::sync::MutexGuard<'static, Option<Session>> {
    SESSION.lock().unwrap_or_else(|e| e.into_inner())
}

/// Free megabytes on the volume holding `dir`.
fn free_mb(dir: &Path) -> Option<u64> {
    let mut wide: Vec<u16> = dir.to_string_lossy().encode_utf16().collect();
    wide.push(0);
    let mut free = 0u64;
    let ok = unsafe { GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), Some(&mut free), None, None) };
    ok.is_ok().then_some(free / (1024 * 1024))
}

/// Current local date/time as `YYYYMMDD` / `HHMMSS` for the name template.
fn stamp() -> (String, String) {
    let st = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    (
        format!("{:04}{:02}{:02}", st.wYear, st.wMonth, st.wDay),
        format!("{:02}{:02}{:02}", st.wHour, st.wMinute, st.wSecond),
    )
}

/// Expand `[record] name`: `{date}` (YYYYMMDD), `{time}` (HHMMSS) and
/// `{datetime}`. Path separators and reserved characters become `_` so a
/// template can never escape the output folder.
pub fn name_for(template: &str, date: &str, time: &str) -> String {
    let mut name = template
        .replace("{datetime}", &format!("{date}-{time}"))
        .replace("{date}", date)
        .replace("{time}", time);
    name = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if (c as u32) < 0x20 => '_',
            c => c,
        })
        .collect();
    let name = name.trim().to_string();
    if name.is_empty() {
        "screen-tools-rs".to_string()
    } else {
        name
    }
}

/// Whole recording stem (name without extension) for the current time.
fn stem_for(template: &str) -> String {
    let (date, time) = stamp();
    name_for(template, &date, &time)
}

/// yuv420p cannot encode odd dimensions; 16 px is the safe minimum.
fn even(v: i32) -> i32 {
    (v & !1).max(16)
}

/// Record `rect` into `settings.dir`. The rect is clamped to even dimensions
/// because yuv420p cannot encode odd ones.
pub fn start(rect: Rect, settings: &Settings) -> Result<()> {
    let mut slot = lock();
    if slot.is_some() {
        bail!("already recording");
    }
    if settings.ffmpeg.trim().is_empty() {
        bail!("[record] ffmpeg is empty");
    }
    let dir = crate::capture::expand_env(&settings.dir);
    let dir = PathBuf::from(dir);
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    if let Some(free) = free_mb(&dir) {
        if free < settings.min_free_mb as u64 {
            bail!(
                "only {free} MB free in {} (minimum {} MB); change [record] min_free_mb or free space",
                dir.display(),
                settings.min_free_mb
            );
        }
    }
    let w = even(rect.w());
    let h = even(rect.h());
    let stem = stem_for(&settings.name);
    let video = dir.join(format!("{stem}.video.mp4"));
    let final_path = dir.join(format!("{stem}.mp4"));
    let fps = settings.fps.clamp(5, 60);
    let crf = settings.crf.clamp(16, 34);

    let audio = if settings.audio {
        let wav = dir.join(format!("{stem}.wav"));
        match crate::audio::start(&wav) {
            Ok(cap) => Some(cap),
            Err(e) => {
                crate::logline(&format!("warn: record audio disabled: {e:#}"));
                None
            }
        }
    } else {
        None
    };

    let mut cmd = Command::new(&settings.ffmpeg);
    cmd.args([
        "-y",
        "-hide_banner",
        "-loglevel",
        "error",
        "-thread_queue_size",
        "1024",
        "-f",
        "gdigrab",
        "-framerate",
        &fps.to_string(),
        "-offset_x",
        &rect.l.to_string(),
        "-offset_y",
        &rect.t.to_string(),
        "-video_size",
        &format!("{w}x{h}"),
        "-i",
        "desktop",
        "-c:v",
        "libx264",
        "-preset",
        "veryfast",
        "-crf",
        &crf.to_string(),
        "-g",
        &(fps * 2).to_string(),
        "-pix_fmt",
        "yuv420p",
        // Fragmented so a killed session is still playable.
        "-movflags",
        "+frag_keyframe+empty_moov+default_base_moof",
        "-max_muxing_queue_size",
        "1024",
    ])
    .arg(&video)
    .stdin(Stdio::piped())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .creation_flags(CREATE_NO_WINDOW);
    let child = cmd
        .spawn()
        .with_context(|| format!("cannot start {}", settings.ffmpeg))?;

    crate::logline(&format!(
        "record: start {} {}x{} fps={fps} crf={crf} audio={}",
        final_path.display(),
        w,
        h,
        if audio.is_some() { "on" } else { "off" }
    ));
    *slot = Some(Session {
        child,
        video,
        audio,
        final_path: final_path.clone(),
        ffmpeg: settings.ffmpeg.clone(),
        started: Instant::now(),
        min_free_mb: settings.min_free_mb,
        max_minutes: settings.max_minutes,
    });
    drop(slot);

    if !WATCHDOG.swap(true, Ordering::SeqCst) {
        let dir = dir.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(3));
                let reason = {
                    let slot = lock();
                    let Some(session) = slot.as_ref() else {
                        break;
                    };
                    auto_stop_reason(
                        session.started.elapsed().as_secs() / 60,
                        session.max_minutes,
                        free_mb(&dir),
                        session.min_free_mb,
                    )
                };
                if let Some(reason) = reason {
                    crate::logline(&format!("record: auto-stop: {reason}"));
                    if let Err(e) = stop() {
                        crate::logline(&format!("record auto-stop failed: {e:#}"));
                    }
                    break;
                }
            }
            WATCHDOG.store(false, Ordering::SeqCst);
        });
    }
    Ok(())
}

/// Stop the running session, muxing audio when there is any. No-op when idle.
pub fn stop() -> Result<()> {
    let mut slot = lock();
    let Some(mut session) = slot.take() else {
        return Ok(());
    };
    drop(slot);
    // Audio first: an audio track shorter than the video is fine with -shortest.
    let wav = session.audio.take().map(|a| a.stop());
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
        std::thread::sleep(Duration::from_millis(50));
    }
    if !exited {
        let _ = session.child.kill();
        let _ = session.child.wait();
    }
    let secs = session.started.elapsed().as_secs_f32();
    let audio_bytes = wav
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .unwrap_or(0);
    let muxed = match wav {
        // 44-byte header + at least ~0.25 s of 48 kHz stereo 16-bit.
        Some(wav) if audio_bytes > 44 + 48_000 => {
            match mux(&session.ffmpeg, &session.video, &wav, &session.final_path) {
                Ok(()) => {
                    let _ = std::fs::remove_file(&session.video);
                    let _ = std::fs::remove_file(&wav);
                    true
                }
                Err(e) => {
                    crate::logline(&format!(
                        "warn: record mux failed, keeping video only: {e:#}"
                    ));
                    false
                }
            }
        }
        Some(wav) => {
            let _ = std::fs::remove_file(&wav);
            false
        }
        None => false,
    };
    if !muxed {
        let _ = std::fs::remove_file(&session.final_path);
        std::fs::rename(&session.video, &session.final_path).with_context(|| {
            format!(
                "cannot move {} to {}",
                session.video.display(),
                session.final_path.display()
            )
        })?;
    }
    crate::logline(&format!(
        "record: stop {} ({secs:.1}s, {})",
        session.final_path.display(),
        if muxed { "video+audio" } else { "video only" }
    ));
    Ok(())
}

fn mux(ffmpeg: &str, video: &Path, wav: &Path, out: &Path) -> Result<()> {
    let status = Command::new(ffmpeg)
        .args(["-y", "-hide_banner", "-loglevel", "error", "-i"])
        .arg(video)
        .arg("-i")
        .arg(wav)
        .args([
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
            "-c:v",
            "copy",
            "-c:a",
            "aac",
            "-b:a",
            "192k",
            "-shortest",
            "-movflags",
            "+faststart",
        ])
        .arg(out)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .with_context(|| format!("cannot run {ffmpeg} for the audio mux"))?;
    if !status.success() {
        bail!("mux exited with {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn even_sizes_are_kept_for_yuv420p() {
        assert_eq!((even(501), even(301)), (500, 300));
        assert_eq!((even(500), even(300)), (500, 300));
        assert_eq!((even(3), even(3)), (16, 16));
        assert_eq!((even(1920), even(1080)), (1920, 1080));
    }

    #[test]
    fn name_template_expands_and_stays_in_the_folder() {
        assert_eq!(
            name_for("screen-tools-rs-{date}-{time}", "20261105", "214500"),
            "screen-tools-rs-20261105-214500"
        );
        assert_eq!(
            name_for("{datetime}-demo", "20261105", "214500"),
            "20261105-214500-demo"
        );
        assert_eq!(
            name_for("rec/{date}:{time}", "20260101", "000000"),
            "rec_20260101_000000"
        );
        assert_eq!(name_for("   ", "20260101", "000000"), "screen-tools-rs");
        assert_eq!(name_for("a<b>c|d?e*f", "d", "t"), "a_b_c_d_e_f");
    }

    #[test]
    fn auto_stop_covers_duration_and_disk() {
        // No cap, plenty of space: keep recording.
        assert!(auto_stop_reason(0, 0, Some(10_000), 500).is_none());
        assert!(auto_stop_reason(59, 60, Some(10_000), 500).is_none());
        // Duration cap, and 0 means no cap.
        assert!(auto_stop_reason(60, 60, Some(10_000), 500)
            .unwrap()
            .contains("max_minutes"));
        assert!(auto_stop_reason(600, 0, Some(10_000), 500).is_none());
        // Free-space floor wins over a long session.
        assert!(auto_stop_reason(5, 60, Some(499), 500)
            .unwrap()
            .contains("free space"));
        // An unknown volume never stops the session on its own.
        assert!(auto_stop_reason(5, 60, None, 500).is_none());
    }

    #[test]
    fn settings_defaults_are_audio_on_with_caps() {
        let s = Settings::default();
        assert!(s.audio);
        assert_eq!(s.name, "screen-tools-rs-{date}-{time}");
        assert_eq!(s.crf, 20);
        assert_eq!(s.max_minutes, 60);
        assert_eq!(s.min_free_mb, 500);
        assert_eq!(s.fps, 30);
    }
}
