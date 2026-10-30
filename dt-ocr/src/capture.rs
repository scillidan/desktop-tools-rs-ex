//! Screen capture via GDI BitBlt (DDA arrives later; see ROADMAP P1). The
//! image lands on the clipboard as CF_DIB plus a CF_BITMAP alias: clipboard
//! managers (copyq etc.) and many apps only read the DIB form, while GDI-style
//! readers want the bitmap handle. Only an explicit Ctrl+S writes a PNG.
//! shortcut: GDI cannot see accelerated DirectComposition content
//! (browsers, some game UIs) — upgrade to Graphics Capture API if a
//! captured region comes out black or stale.

use crate::logline;
use anyhow::{anyhow, ensure, Context, Result};
use std::ptr;
use windows::core::HSTRING;
use windows::Foundation::{AsyncStatus, IAsyncAction, IAsyncOperation};
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapEncoder, BitmapPixelFormat};
use windows::Storage::FileAccessMode;
use windows::Storage::Streams::{FileRandomAccessStream, IRandomAccessStream};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GHND};
use windows::Win32::System::Ole::{CF_BITMAP, CF_DIB, CF_UNICODETEXT};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Classes owned by the daemon (never a capture target) and by the shell's
/// desktop; everything else is fair game.
const OWN_CLASSES: [&str; 6] = [
    "dt-ocr.frame",
    "dt-ocr.mask",
    "dt-ocr.overlay",
    "dt-ocr.host",
    "dt-ocr.status",
    "dt-ocr.pin",
];
const SHELL_DESKTOP_CLASSES: [&str; 4] = [
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
];

/// One grabbed rectangle: a top-down 32 bpp DIB section (pixel access) plus a
/// device-dependent copy for CF_BITMAP, because a DIB *section* handle reads as
/// null in other processes (a .NET `Clipboard.GetImage()` then returns nothing).
struct Shot {
    w: i32,
    h: i32,
    bmp: HBITMAP,
    dd: HBITMAP,
    bits: *mut u8,
}

impl Shot {
    /// Hands the pixels to the clipboard (DIB block + bitmap alias).
    unsafe fn into_clipboard(self, hwnd: HWND) -> Result<()> {
        let header = std::mem::size_of::<BITMAPINFOHEADER>();
        let pixels = (self.w as usize) * (self.h as usize) * 4;
        let dib = GlobalAlloc(GHND, header + pixels).context("GlobalAlloc failed")?;
        anyhow::ensure!(!dib.is_invalid(), "GlobalAlloc returned null");
        let dst = GlobalLock(dib);
        anyhow::ensure!(!dst.is_null(), "GlobalLock failed");
        let mut info = BITMAPINFO::default();
        info.bmiHeader.biSize = header as u32;
        info.bmiHeader.biWidth = self.w;
        info.bmiHeader.biHeight = self.h;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB.0;
        ptr::copy_nonoverlapping(
            &info.bmiHeader as *const _ as *const u8,
            dst as *mut u8,
            header,
        );
        // The Shot is top-down; CF_DIB consumers expect bottom-up rows.
        ptr::copy_nonoverlapping(
            flip_rows(self.bits, self.w, self.h).as_ptr(),
            (dst as *mut u8).add(header),
            pixels,
        );
        let _ = GlobalUnlock(dib);

        // Another app can hold the clipboard for a moment (IME hosts, clipboard
        // managers), so a single failed OpenClipboard is not fatal.
        if !open_clipboard(hwnd) {
            let _ = GlobalFree(Some(dib));
            return Err(anyhow!(
                "OpenClipboard failed: {}",
                windows::core::Error::from_win32()
            ));
        }
        let mut this = self;
        let mut failure = None;
        let mut dib_taken = false;
        if EmptyClipboard().is_err() {
            failure = Some("EmptyClipboard failed".to_string());
        } else if SetClipboardData(CF_DIB.0 as u32, Some(HANDLE(dib.0))).is_err() {
            failure = Some(format!(
                "SetClipboardData(CF_DIB) failed: {}",
                windows::core::Error::from_win32()
            ));
        } else {
            dib_taken = true;
            // The bitmap alias serves GDI readers (.NET's Clipboard.GetImage
            // reads CF_BITMAP); losing it costs compatibility, nothing else.
            if SetClipboardData(CF_BITMAP.0 as u32, Some(HANDLE(this.dd.0))).is_err() {
                logline("warn: clipboard CF_BITMAP alias failed");
            } else {
                this.dd = HBITMAP(std::ptr::null_mut());
            }
        }
        let _ = CloseClipboard();
        if !dib_taken {
            let _ = GlobalFree(Some(dib));
        }
        if let Some(e) = failure {
            return Err(anyhow!("{e}"));
        }
        Ok(())
    }

    /// Top-down BGRA copy of the pixels.
    fn bgra(&self) -> Vec<u8> {
        let len = (self.w as usize) * (self.h as usize) * 4;
        unsafe { std::slice::from_raw_parts(self.bits, len) }.to_vec()
    }

    /// Writes `<dir>/dt-ocr-YYYYMMDD-HHMMSS.png` and returns its path. The
    /// encode runs on a worker thread: WinRT async results on the UI (STA)
    /// thread only complete by pumping messages, so polling there deadlocks.
    fn save_png(&self, dir: &str) -> Result<std::path::PathBuf> {
        let dir = expand_env(dir);
        std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {dir}"))?;
        let st = unsafe { GetLocalTime() };
        let path = std::path::Path::new(&dir).join(file_name(&st));
        let data = self.bgra();
        let (w, h) = (self.w as u32, self.h as u32);
        // WinRT's PNG encoder: no dependency, and the empty file opens as a
        // stream without the StorageFile folder dance.
        std::fs::File::create(&path)
            .with_context(|| format!("cannot create {}", path.display()))?;
        let write = path.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = unsafe {
                if windows::Win32::System::Com::CoInitializeEx(
                    None,
                    windows::Win32::System::Com::COINIT_MULTITHREADED,
                )
                .is_err()
                {
                    Err(anyhow!("cannot enter an MTA apartment"))
                } else {
                    let r = write_png(&write, w, h, &data);
                    windows::Win32::System::Com::CoUninitialize();
                    r
                }
            };
            let _ = tx.send(result);
        });
        match rx.recv() {
            Ok(Ok(())) => Ok(path),
            Ok(Err(e)) => Err(e),
            Err(e) => Err(anyhow!("PNG worker vanished: {e}")),
        }
    }
}

impl Drop for Shot {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.bmp.into());
            let _ = DeleteObject(self.dd.into());
        }
    }
}

/// Opens the clipboard, retrying briefly: IME hosts and clipboard managers hold
/// it for a few hundred milliseconds at a time.
unsafe fn open_clipboard(hwnd: HWND) -> bool {
    for _ in 0..6 {
        if OpenClipboard(Some(hwnd)).is_ok() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    false
}

/// Copies top-down BGRA rows into bottom-up order: CF_DIB consumers read rows
/// bottom-up while our DIB section and the PNG encoder want top-down.
fn flip_rows(bits: *const u8, w: i32, h: i32) -> Vec<u8> {
    let stride = (w as usize) * 4;
    let mut out = vec![0u8; stride * h as usize];
    for y in 0..h as usize {
        let src =
            unsafe { std::slice::from_raw_parts(bits.add((h as usize - 1 - y) * stride), stride) };
        out[y * stride..(y + 1) * stride].copy_from_slice(src);
    }
    out
}

/// `dt-ocr-YYYYMMDD-HHMMSS.png` in local time.
fn file_name(st: &SYSTEMTIME) -> String {
    format!(
        "dt-ocr-{:04}{:02}{:02}-{:02}{:02}{:02}.png",
        st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond
    )
}

/// `%USERPROFILE%\Pictures` style expansion; unknown names stay literal.
fn expand_env(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 1..];
        match tail.find('%') {
            Some(end) => {
                let name = &tail[..end];
                match std::env::var(name) {
                    Ok(v) if !name.is_empty() => out.push_str(&v),
                    _ => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &tail[end + 1..];
            }
            None => {
                out.push('%');
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Classes we never capture: our own windows and the shell's desktop.
fn excluded_class(name: &str) -> bool {
    OWN_CLASSES.contains(&name) || SHELL_DESKTOP_CLASSES.contains(&name)
}

unsafe fn class_of(hwnd: HWND) -> String {
    let mut buf = [0u16; 128];
    let n = GetClassNameW(hwnd, &mut buf);
    String::from_utf16_lossy(&buf[..n as usize])
}

/// Visible frame of `hwnd`, excluding the invisible resize border.
unsafe fn window_rect(hwnd: HWND) -> RECT {
    let mut r = RECT::default();
    if DwmGetWindowAttribute(
        hwnd,
        DWMWA_EXTENDED_FRAME_BOUNDS,
        &mut r as *mut _ as *mut _,
        std::mem::size_of::<RECT>() as u32,
    )
    .is_err()
    {
        let _ = GetWindowRect(hwnd, &mut r);
    }
    r
}

/// A usable capture target, or None when the window is ours, the shell
/// desktop, a non-activatable overlay (IME candidate, notification, shadow), a
/// tool window, or empty. `GetForegroundWindow` can name such a window, so this
/// decides the fallback chain.
unsafe fn usable(hwnd: HWND) -> Option<RECT> {
    if hwnd.0.is_null() || !IsWindowVisible(hwnd).as_bool() {
        return None;
    }
    // Resolve to the top-level owner: a dialog/child still belongs to its app.
    let hwnd = GetAncestor(hwnd, GA_ROOTOWNER);
    if hwnd.0.is_null() {
        return None;
    }
    if excluded_class(&class_of(hwnd)) {
        return None;
    }
    let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
    // An app window that can hold the foreground is never NOACTIVATE; the IME
    // and notification surfaces are, and capturing them means "whole screen".
    if (ex & WS_EX_NOACTIVATE.0 as isize) != 0 {
        return None;
    }
    // Our own pins are tool windows; so are HUDs and overlays. Skip them.
    if (ex & WS_EX_TOOLWINDOW.0 as isize) != 0 {
        return None;
    }
    // XAML/IME surfaces (no redirection bitmap) are composited overlays, not
    // app windows: the IME candidate host is a full-screen one of these.
    if (ex & WS_EX_NOREDIRECTIONBITMAP.0 as isize) != 0 {
        return None;
    }
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid == windows::Win32::System::Threading::GetCurrentProcessId() {
        return None;
    }
    let r = window_rect(hwnd);
    (r.right > r.left && r.bottom > r.top).then_some(r)
}

struct Under {
    pt: POINT,
    found: Option<RECT>,
}

unsafe extern "system" fn under_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let hit = &mut *(lparam.0 as *mut Under);
    if hit.found.is_some() {
        return false.into();
    }
    let mut r = RECT::default();
    if !GetWindowRect(hwnd, &mut r).is_ok() {
        return true.into();
    }
    let inside =
        hit.pt.x >= r.left && hit.pt.x < r.right && hit.pt.y >= r.top && hit.pt.y < r.bottom;
    // Walk the z-order top-down: overlays above the app are rejected by
    // `usable`, so the first hit is the app the user actually sees.
    if inside {
        if let Some(rect) = usable(hwnd) {
            hit.found = Some(rect);
            return false.into();
        }
    }
    true.into()
}

/// Topmost real window containing `pt`, skipping overlays.
unsafe fn window_under(pt: POINT) -> Option<RECT> {
    let mut hit = Under { pt, found: None };
    let _ = EnumWindows(Some(under_cb), LPARAM(&mut hit as *mut _ as isize));
    hit.found
}

/// Focused window, else the window under the cursor, else the monitor there.
/// `win+d`, a lock screen or a fullscreen launcher leave no foreground window,
/// which is why the fallbacks exist.
pub unsafe fn window_target() -> Result<RECT> {
    let fg = GetForegroundWindow();
    if let Some(r) = usable(fg) {
        logline(&format!("capture target: foreground {}", class_of(fg)));
        return Ok(r);
    }
    let mut pt = POINT::default();
    if GetCursorPos(&mut pt).is_err() {
        // Some shell/IME transitions transiently reject GetCursorPos. The
        // hotkey message still carries a usable screen-space cursor position.
        let pos = GetMessagePos();
        pt = POINT {
            x: pos as i16 as i32,
            y: (pos >> 16) as i16 as i32,
        };
        logline("warn: GetCursorPos failed; using the hotkey message position");
    }
    if let Some(r) = window_under(pt) {
        logline(&format!(
            "capture target: under cursor at ({},{})",
            pt.x, pt.y
        ));
        return Ok(r);
    }
    logline("capture target: monitor (no usable window)");
    let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    anyhow::ensure!(
        GetMonitorInfoW(mon, &mut info).as_bool(),
        "GetMonitorInfoW failed"
    );
    Ok(info.rcMonitor)
}

/// BitBlt the screen rect into a top-down 32 bpp DIB with opaque alpha.
/// Retries briefly: right after a shell/desktop change the screen DC can be
/// stale and every GDI call on it fails with ERROR_INVALID_HANDLE.
unsafe fn grab(rect: RECT) -> Result<Shot> {
    let mut last = None;
    for attempt in 0..3 {
        match grab_once(rect) {
            Ok(shot) => return Ok(shot),
            Err(e) => {
                last = Some(e);
                if attempt < 2 {
                    logline(&format!(
                        "warn: capture attempt {} failed, retrying: {}",
                        attempt + 1,
                        last.as_ref().map(|e| format!("{e:#}")).unwrap_or_default()
                    ));
                    std::thread::sleep(std::time::Duration::from_millis(150));
                }
            }
        }
    }
    Err(last.unwrap_or_else(|| anyhow!("capture failed")))
}

unsafe fn grab_once(rect: RECT) -> Result<Shot> {
    let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
    anyhow::ensure!(w > 0 && h > 0, "capture rect has no area");
    let screen = GetDC(None);
    anyhow::ensure!(!screen.0.is_null(), "GetDC(screen) failed");
    let mem = CreateCompatibleDC(Some(screen));
    if mem.is_invalid() {
        let _ = ReleaseDC(None, screen);
        anyhow::bail!("CreateCompatibleDC failed");
    }
    // Negative height = top-down, which is what SoftwareBitmap and the PNG
    // encoder both expect.
    let mut info = BITMAPINFO::default();
    info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    info.bmiHeader.biWidth = w;
    info.bmiHeader.biHeight = -h;
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    info.bmiHeader.biCompression = BI_RGB.0;
    let mut bits: *mut core::ffi::c_void = ptr::null_mut();
    let bmp = CreateDIBSection(Some(screen), &info, DIB_RGB_COLORS, &mut bits, None, 0)
        .context("CreateDIBSection failed")?;
    if bmp.is_invalid() || bits.is_null() {
        let _ = DeleteDC(mem);
        let _ = ReleaseDC(None, screen);
        anyhow::bail!("CreateDIBSection returned null");
    }
    let old = SelectObject(mem, bmp.into());
    let ok = BitBlt(mem, 0, 0, w, h, Some(screen), rect.left, rect.top, SRCCOPY).is_ok();
    // Plain device bitmap copy for CF_BITMAP: cross-process readers get a
    // converted copy, which a DIB section handle cannot provide.
    let dd = CreateCompatibleBitmap(screen, w, h);
    if !dd.is_invalid() {
        let mem2 = CreateCompatibleDC(Some(screen));
        let old2 = SelectObject(mem2, dd.into());
        let _ = BitBlt(mem2, 0, 0, w, h, Some(mem), 0, 0, SRCCOPY);
        let _ = SelectObject(mem2, old2);
        let _ = DeleteDC(mem2);
    }
    let _ = SelectObject(mem, old);
    let _ = DeleteDC(mem);
    let _ = ReleaseDC(None, screen);
    if !ok {
        logline(&format!(
            "warn: BitBlt {w}x{h} at ({},{}) failed: {}",
            rect.left,
            rect.top,
            windows::core::Error::from_win32()
        ));
        let _ = DeleteObject(bmp.into());
        let _ = DeleteObject(dd.into());
        anyhow::bail!("BitBlt failed");
    }
    // GDI leaves the alpha byte of a BI_RGB 32 bpp DIB undefined (zero);
    // readers that honour alpha (copyq/Qt, PNG) would see through the image.
    let px = (w as usize) * (h as usize);
    let p = bits as *mut u8;
    for i in 0..px {
        *p.add(i * 4 + 3) = 0xFF;
    }
    logline(&format!("capture: {w}x{h} at ({},{})", rect.left, rect.top));
    Ok(Shot {
        w,
        h,
        bmp,
        dd,
        bits: p,
    })
}

/// Polls a WinRT async operation: the windows crate has no blocking await,
/// and this is the pattern the OCR path uses too.
fn wait_op<T: windows::core::RuntimeType + 'static>(op: &IAsyncOperation<T>) -> Result<T> {
    loop {
        match op.Status().context("async status failed")? {
            AsyncStatus::Completed => return op.GetResults().context("async results missing"),
            AsyncStatus::Started => std::thread::sleep(std::time::Duration::from_millis(5)),
            other => return Err(anyhow!("async operation finished with {other:?}")),
        }
    }
}

fn wait_action(op: &IAsyncAction) -> Result<()> {
    loop {
        match op.Status().context("async status failed")? {
            AsyncStatus::Completed => return Ok(()),
            AsyncStatus::Started => std::thread::sleep(std::time::Duration::from_millis(5)),
            other => return Err(anyhow!("async action finished with {other:?}")),
        }
    }
}

pub(crate) unsafe fn write_png(path: &std::path::Path, w: u32, h: u32, bgra: &[u8]) -> Result<()> {
    let file = HSTRING::from(path.to_string_lossy().as_ref());
    let stream: IRandomAccessStream = wait_op(&FileRandomAccessStream::OpenAsync(
        &file,
        FileAccessMode::ReadWrite,
    )?)?;
    let encoder = wait_op(&BitmapEncoder::CreateAsync(
        BitmapEncoder::PngEncoderId()?,
        &stream,
    )?)?;
    encoder.SetPixelData(
        BitmapPixelFormat::Bgra8,
        BitmapAlphaMode::Ignore,
        w,
        h,
        96.0,
        96.0,
        bgra,
    )?;
    wait_action(&encoder.FlushAsync()?)?;
    stream.Close().ok();
    Ok(())
}

pub unsafe fn capture_frame(hwnd: HWND) -> Result<()> {
    let mut wr = RECT::default();
    let _ = GetWindowRect(hwnd, &mut wr);
    grab(wr)?.into_clipboard(hwnd)?;
    logline("capture: -> clipboard (CF_DIB + CF_BITMAP)");
    Ok(())
}

/// Puts an explicit rect on screen as a pinned image; no clipboard, no file.
pub unsafe fn pin_rect(rect: RECT, style: crate::pin::Style) -> Result<()> {
    let shot = grab(rect)?;
    let (w, h) = (shot.w, shot.h);
    let data = shot.bgra();
    crate::pin::show(rect, w, h, &data, style)
}

/// Same, for the focused window.
pub unsafe fn pin_window(style: crate::pin::Style) -> Result<()> {
    pin_rect(window_target()?, style)
}

/// Folder picker for Ctrl+S. `SHBrowseForFolderW` needs no extra COM wiring
/// and returns one path.
pub unsafe fn choose_dir(owner: HWND) -> Result<Option<std::path::PathBuf>> {
    use std::ffi::c_void;
    use windows::core::w;
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{
        SHBrowseForFolderW, SHGetPathFromIDListW, BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS,
        BROWSEINFOW,
    };
    let mut shown = [0u16; 260];
    let bi = BROWSEINFOW {
        hwndOwner: owner,
        pszDisplayName: windows::core::PWSTR(shown.as_mut_ptr()),
        lpszTitle: w!("Choose the folder for this capture"),
        ulFlags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE,
        ..Default::default()
    };
    let pidl = SHBrowseForFolderW(&bi);
    if pidl.is_null() {
        return Ok(None); // cancelled
    }
    let mut path = [0u16; 260];
    let ok = SHGetPathFromIDListW(pidl, &mut path).as_bool();
    CoTaskMemFree(Some(pidl as *const c_void));
    ensure!(ok, "the selected folder has no filesystem path");
    let len = path.iter().position(|&c| c == 0).unwrap_or(path.len());
    Ok(Some(std::path::PathBuf::from(String::from_utf16_lossy(
        &path[..len],
    ))))
}

/// Saves the frame rect, then puts the frame away. The frame is hidden during
/// the grab so neither its border nor its shadow can enter the image.
pub unsafe fn save_frame(hwnd: HWND, dir: &str) -> Result<()> {
    let rect = visible_rect(hwnd)?;
    let _ = ShowWindow(hwnd, SW_HIDE);
    let result = grab(rect).and_then(|shot| {
        shot.save_png(dir).map(|path| {
            logline(&format!("capture: frame -> {}", path.display()));
        })
    });
    if result.is_err() {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
    result
}

/// Copies the frame rect, then puts the frame away; no OCR and no file.
pub unsafe fn copy_frame(hwnd: HWND) -> Result<()> {
    let rect = visible_rect(hwnd)?;
    let _ = ShowWindow(hwnd, SW_HIDE);
    let result = grab(rect).and_then(|shot| shot.into_clipboard(hwnd));
    if result.is_err() {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    } else {
        logline("capture: frame -> clipboard, frame hidden");
    }
    result
}

unsafe fn visible_rect(hwnd: HWND) -> Result<RECT> {
    let mut rect = RECT::default();
    GetWindowRect(hwnd, &mut rect)?;
    anyhow::ensure!(
        rect.right > rect.left && rect.bottom > rect.top,
        "frame has no area"
    );
    Ok(rect)
}

/// Plain text for the P0.3 overlay (CF_UNICODETEXT), replacing the previous
/// clipboard contents.
pub unsafe fn copy_text(hwnd: HWND, text: &str) -> Result<()> {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = wide.len() * 2;
    let block = GlobalAlloc(GHND, bytes).context("GlobalAlloc failed")?;
    anyhow::ensure!(!block.is_invalid(), "GlobalAlloc returned null");
    let dst = GlobalLock(block);
    anyhow::ensure!(!dst.is_null(), "GlobalLock failed");
    ptr::copy_nonoverlapping(wide.as_ptr() as *const u8, dst as *mut u8, bytes);
    let _ = GlobalUnlock(block);

    if !open_clipboard(hwnd) {
        let _ = GlobalFree(Some(block));
        anyhow::bail!("OpenClipboard failed");
    }
    let mut clip_err = None;
    if let Err(e) = EmptyClipboard() {
        clip_err = Some(format!("EmptyClipboard failed: {e:?}"));
    } else if let Err(e) = SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(block.0))) {
        clip_err = Some(format!("SetClipboardData(CF_UNICODETEXT) failed: {e:?}"));
    }
    let _ = CloseClipboard();
    if let Some(e) = clip_err {
        let _ = GlobalFree(Some(block));
        anyhow::bail!("{e}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_vars_expand_and_unknown_names_stay_literal() {
        std::env::set_var("DT_OCR_TEST_DIR", "C:\\shots");
        assert_eq!(expand_env("%DT_OCR_TEST_DIR%\\a.png"), "C:\\shots\\a.png");
        assert_eq!(expand_env("%DT_OCR_NOPE%\\b.png"), "%DT_OCR_NOPE%\\b.png");
        assert_eq!(expand_env("plain"), "plain");
        assert_eq!(expand_env("100% sure"), "100% sure");
        assert_eq!(
            expand_env("a%DT_OCR_TEST_DIR%b%DT_OCR_TEST_DIR%"),
            "aC:\\shotsbC:\\shots"
        );
    }

    #[test]
    fn file_name_is_sortable_and_second_precise() {
        let st = SYSTEMTIME {
            wYear: 2026,
            wMonth: 3,
            wDay: 7,
            wHour: 9,
            wMinute: 5,
            wSecond: 1,
            ..Default::default()
        };
        assert_eq!(file_name(&st), "dt-ocr-20260307-090501.png");
    }

    #[test]
    fn flip_rows_turns_top_down_into_bottom_up() {
        // Two 1-px rows: red above, green below (BGRA).
        let top_down = [
            0u8, 0, 255, 255, // red
            0, 255, 0, 255, // green
        ];
        let flipped = flip_rows(top_down.as_ptr(), 1, 2);
        assert_eq!(&flipped[0..4], &[0, 255, 0, 255], "green comes first");
        assert_eq!(&flipped[4..8], &[0, 0, 255, 255], "red last");
    }

    #[test]
    fn own_and_shell_classes_are_never_capture_targets() {
        for c in OWN_CLASSES.iter().chain(SHELL_DESKTOP_CLASSES.iter()) {
            assert!(excluded_class(c), "{c} must be excluded");
        }
        assert!(!excluded_class("Notepad"));
        assert!(!excluded_class("CabinetWClass"));
    }
}
