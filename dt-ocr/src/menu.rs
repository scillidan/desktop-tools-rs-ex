//! Native tray popup, using the same TrackPopupMenu flow as
//! desktop-tools-rs (`src/win32.rs::pick_sizer` and tray-icon 0.20): the owner
//! is brought to the foreground, the popup closes on an outside click, and the
//! selected command is returned synchronously with TPM_RETURNCMD.
//!
//! Provenance: `desktop-tools-rs src/win32.rs pick_sizer`; `tray-icon-0.20.1
//! src/platform_impl/windows/mod.rs show_tray_menu`.

use windows::core::HSTRING;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, PostMessageW, SetForegroundWindow,
    TrackPopupMenu, MF_STRING, TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_NONOTIFY, TPM_RETURNCMD,
    WM_NULL,
};

const QUIT_ID: usize = 1;
const TEXT: &str = "Quit dt-ocr";

/// Shows the native popup above the cursor and returns `true` when Quit was
/// selected. The foreground host owner is required: without it the menu can
/// remain "stuck" after dismissal.
pub unsafe fn show(owner: HWND) -> bool {
    let Ok(menu) = CreatePopupMenu() else {
        return false;
    };
    let label = HSTRING::from(TEXT);
    if AppendMenuW(menu, MF_STRING, QUIT_ID, &label).is_err() {
        let _ = DestroyMenu(menu);
        return false;
    }
    let mut pt = POINT::default();
    if GetCursorPos(&mut pt).is_err() {
        let _ = DestroyMenu(menu);
        return false;
    }

    let _ = SetForegroundWindow(owner);
    // Keep the menu simple and above the icon: bottom-left anchored at the
    // cursor, synchronous command selection, and no WM_COMMAND notification.
    let selected = TrackPopupMenu(
        menu,
        TPM_BOTTOMALIGN | TPM_LEFTALIGN | TPM_RETURNCMD | TPM_NONOTIFY,
        pt.x,
        pt.y,
        None,
        owner,
        None,
    );
    // Provenance: desktop-tools-rs pick_sizer — without this a dismissed popup
    // can leave the owning window in a modal state.
    let _ = PostMessageW(Some(owner), WM_NULL, WPARAM(0), LPARAM(0));
    let _ = DestroyMenu(menu);
    selected.0 == QUIT_ID as i32
}
