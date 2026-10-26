//! Screen capture of the frame region via GDI BitBlt (DDA arrives later;
//! see ROADMAP P1). The image lands on the clipboard as CF_DIB plus a
//! CF_BITMAP alias: clipboard managers (copyq etc.) and many apps only
//! read the DIB form, while GDI-style readers want the bitmap handle.
//! shortcut: GDI cannot see accelerated DirectComposition content
//! (browsers, some game UIs) — upgrade to Graphics Capture API if a
//! captured region comes out black or stale.

use crate::logline;
use anyhow::{Context, Result};
use std::ptr;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GHND};
use windows::Win32::System::Ole::{CF_BITMAP, CF_DIB, CF_UNICODETEXT};
use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

pub unsafe fn capture_frame(hwnd: HWND) -> Result<()> {
    let mut wr = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut wr) }.ok();
    anyhow::ensure!(
        wr.right > wr.left && wr.bottom > wr.top,
        "frame has no area"
    );
    let (w, h) = (wr.right - wr.left, wr.bottom - wr.top);

    let screen = GetDC(None);
    anyhow::ensure!(!screen.0.is_null(), "GetDC(screen) failed");
    let mem = CreateCompatibleDC(Some(screen));
    // 32 bpp bottom-up DIB; rows are 4-byte aligned by construction.
    let mut info = BITMAPINFO::default();
    info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    info.bmiHeader.biWidth = w;
    info.bmiHeader.biHeight = h;
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    info.bmiHeader.biCompression = BI_RGB.0;
    let mut bits: *mut core::ffi::c_void = ptr::null_mut();
    let bmp = CreateDIBSection(Some(screen), &info, DIB_RGB_COLORS, &mut bits, None, 0)
        .context("CreateDIBSection failed")?;
    anyhow::ensure!(!bmp.is_invalid(), "CreateDIBSection returned null");
    let old = SelectObject(mem, bmp.into());
    let ok = BitBlt(mem, 0, 0, w, h, Some(screen), wr.left, wr.top, SRCCOPY).is_ok();
    let _ = SelectObject(mem, old);
    let _ = DeleteDC(mem);
    let _ = ReleaseDC(None, screen);
    if !ok {
        let _ = DeleteObject(bmp.into());
        anyhow::bail!("BitBlt failed");
    }
    // GDI leaves the alpha byte of a BI_RGB 32 bpp DIB undefined (zero);
    // readers that honour alpha (copyq/Qt) would see a fully transparent
    // image. Fill it in.
    let px = (w as usize) * (h as usize);
    let p = bits as *mut u8;
    for i in 0..px {
        unsafe { *p.add(i * 4 + 3) = 0xFF };
    }

    // Flatten header + bits into a movable global block for CF_DIB.
    let header = std::mem::size_of::<BITMAPINFOHEADER>();
    let pixels = (w as usize) * (h as usize) * 4;
    let dib = GlobalAlloc(GHND, header + pixels).context("GlobalAlloc failed")?;
    anyhow::ensure!(!dib.is_invalid(), "GlobalAlloc returned null");
    let dst = GlobalLock(dib);
    anyhow::ensure!(!dst.is_null(), "GlobalLock failed");
    unsafe {
        ptr::copy_nonoverlapping(
            &info.bmiHeader as *const _ as *const u8,
            dst as *mut u8,
            header,
        );
        ptr::copy_nonoverlapping(bits as *const u8, (dst as *mut u8).add(header), pixels);
    }
    let _ = GlobalUnlock(dib);

    OpenClipboard(Some(hwnd)).context("OpenClipboard failed")?;
    let mut clip_err = None;
    if EmptyClipboard().is_err() {
        clip_err = Some("EmptyClipboard failed");
    } else if SetClipboardData(CF_DIB.0 as u32, Some(HANDLE(dib.0))).is_err() {
        clip_err = Some("SetClipboardData(CF_DIB) failed");
    } else if SetClipboardData(CF_BITMAP.0 as u32, Some(HANDLE(bmp.0))).is_err() {
        clip_err = Some("SetClipboardData(CF_BITMAP) failed");
    }
    let _ = CloseClipboard();
    if let Some(e) = clip_err {
        let _ = DeleteObject(bmp.into());
        let _ = GlobalFree(Some(dib));
        anyhow::bail!("{e}");
    }
    // The clipboard owns the DIB block and the bitmap now — do not free them.
    logline(&format!(
        "capture: {w}x{h} at ({},{}) -> clipboard (CF_DIB + CF_BITMAP)",
        wr.left, wr.top
    ));
    Ok(())
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

    OpenClipboard(Some(hwnd)).context("OpenClipboard failed")?;
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
