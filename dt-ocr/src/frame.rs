//! The OCR frame window: borderless, always-on-top, hollow (color-keyed
//! interior so the screen shows through), draggable by its border when
//! unpinned, click-through when pinned. `ctrl+alt+s` type-ahead size mode is
//! handled here (WM_CHAR/WM_KEYDOWN) once the hotkey flips the window into
//! spec mode.

use crate::interaction::{resize_drag, resize_edge, transition, Edge, Event, Mode};
use crate::logline;
use anyhow::{ensure, Result};
use dt_common::layout::{Rect, SizeSpec};
use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForSystem, SystemParametersInfoForDpi};
use windows::Win32::UI::Input::Ime::{ImmAssociateContextEx, HIMC, IACE_DEFAULT};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, SetFocus, VIRTUAL_KEY, VK_BACK, VK_CONTROL, VK_DOWN,
    VK_ESCAPE, VK_LEFT, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SystemParametersInfoW, SM_CXSCREEN, SM_CYSCREEN, *,
};

/// Interior color keyed out so the frame is hollow.
pub const COLOR_KEY: COLORREF = COLORREF(0x00FE00FF); // RGB(255, 0, 254)
const BORDER_DRAG: u32 = 0x00C8C800; // RGB(0, 200, 200) cyan — draggable
const BORDER_PINNED: u32 = 0x00D77800; // RGB(0, 120, 215) blue — pinned
const BORDER_SPEC: u32 = 0x0000C8FF; // RGB(255, 200, 0) yellow — size mode
const BAR_YELLOW: u32 = 0x0000D2FF; // RGB(255, 210, 0) — status strip bg / inside text
const TEXT_BLACK: u32 = 0x00000000;
const TUNE_RED: u32 = 0x000000FF; // RGB(255, 0, 0) — value clamped at the monitor limit
const FLASH_CYAN: u32 = 0x00FFFF00; // RGB(0, 255, 255) — capture feedback flash
                                    // APPWINDOW gives the shell a per-virtual-desktop application view.
                                    // Without it this popup is considered current on every desktop.
const FRAME_EX_STYLE: WINDOW_EX_STYLE =
    WINDOW_EX_STYLE(WS_EX_LAYERED.0 | WS_EX_TOPMOST.0 | WS_EX_NOACTIVATE.0 | WS_EX_APPWINDOW.0);
const EDGE: i32 = 6; // border hit zone (px) for the spec-mode resize handles
/// Height of the status strip shown OUTSIDE the frame (below preferred,
/// above as fallback), and the gap between frame border and strip.
const STRIP_H: i32 = 24;
const STRIP_GAP: i32 = 2;

pub struct FrameState {
    /// Exclude the taskbar from movement and sizing bounds.
    pub work_area: bool,
    /// The single source of truth: Move / Pinned / Input / Drag / Tune.
    pub mode: Mode,
    /// OCR worker running: the frame is click-through until it finishes
    /// (PLAN P0.3 — nothing under the frame is blocked during recognition).
    pub ocr_running: bool,
    /// Upscale factor for the next OCR capture (`[ocr] scale`).
    pub ocr_scale: u32,
    /// Requested recognizer language (`[ocr] language`); None = auto.
    pub ocr_language: Option<String>,
    /// Type-ahead buffer used by the Input state.
    pub spec: String,
    /// Show the resize status strip (`frame.show_hint`).
    pub show_hint: bool,
    /// Status-line font face (`frame.font`, default Consolas).
    pub font_face: String,
    /// Separate topmost window carrying the strip when there is room
    /// outside the frame; hidden (and `inside` = true) otherwise.
    pub status: HWND,
    /// The strip currently renders inside the frame (yellow text) instead
    /// of in the outside strip window.
    pub inside: bool,
    /// Blink phase of the Input cursor (toggled by the 500 ms timer).
    pub cursor_on: bool,
    /// Fine-tune data; present only in the Tune state.
    pub tune: Option<Tune>,
    /// Set once resize mode has been entered; the first entry of a frame's
    /// lifetime lands on Tune(width), later ones on Drag.
    pub entered_once: bool,
    /// Capture feedback: border flashes cyan once (cleared by timer 4).
    pub flash: bool,
    /// Active client-mouse edge drag; independent of the public mode.
    mouse_drag: Option<MouseDrag>,
}

#[derive(Clone, Copy)]
struct MouseDrag {
    horizontal: Option<Edge>,
    vertical: Option<Edge>,
    start: Rect,
    pointer: POINT,
}

/// Which half of the `w x h` readout is being fine-tuned.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TuneField {
    W,
    H,
}

pub struct Tune {
    pub field: TuneField,
    /// Digits typed while tuning (replace the value); empty = use the
    /// frame's current size.
    pub digits: String,
    /// Work-area clamp for this axis at the frame's current center.
    pub limit: i32,
}

impl Tune {
    fn new(field: TuneField, limit: i32) -> Self {
        Tune {
            field,
            digits: String::new(),
            limit,
        }
    }
}

/// Pure geometry: new frame rect for `spec`, centered on the current center
/// and clamped into the work area. Missing axes keep the current size.
pub fn apply_size(spec: SizeSpec, cur: Rect, work: Rect) -> Rect {
    let (mut w, mut h) = spec.resolve(work, cur);
    w = w.clamp(16, work.w());
    h = h.clamp(16, work.h());
    let cx = (cur.l + cur.r) / 2;
    let cy = (cur.t + cur.b) / 2;
    let mut l = cx - w / 2;
    let mut t = cy - h / 2;
    l = l.clamp(work.l, work.r - w);
    t = t.clamp(work.t, work.b - h);
    Rect {
        l,
        t,
        r: l + w,
        b: t + h,
    }
}

pub(crate) fn rect_of(r: RECT) -> Rect {
    Rect {
        l: r.left,
        t: r.top,
        r: r.right,
        b: r.bottom,
    }
}

/// Configured bounds of the monitor `hwnd` is on (fallback: primary).
/// Percent/fraction specs use the same bounds as pointer and keyboard sizing.
unsafe fn monitor_bounds(hwnd: HWND) -> Rect {
    let full = state_opt(hwnd).is_some_and(|s| !s.work_area);
    let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
    let mut mi = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !monitor.is_invalid() && GetMonitorInfoW(monitor, &mut mi).as_bool() {
        return rect_of(if full { mi.rcMonitor } else { mi.rcWork });
    }
    if full {
        Rect {
            l: 0,
            t: 0,
            r: GetSystemMetrics(SM_CXSCREEN),
            b: GetSystemMetrics(SM_CYSCREEN),
        }
    } else {
        work_area()
    }
}

/// Primary monitor's work area, in physical pixels (the process is
/// per-monitor DPI aware).
pub fn work_area() -> Rect {
    let mut rect = RECT::default();
    unsafe {
        let r = SystemParametersInfoForDpi(
            SPI_GETWORKAREA.0,
            0,
            Some(&mut rect as *mut RECT as *mut core::ffi::c_void),
            Default::default(),
            GetDpiForSystem(),
        );
        if r.is_err() {
            logline(&format!("warn: SystemParametersInfoForDpi failed: {r:?}"));
            // The ForDpi flavor can fail on older shells; plain SPI works
            // because the process is already DPI-aware from startup.
            let _ = SystemParametersInfoW(
                SPI_GETWORKAREA,
                0,
                Some(&mut rect as *mut RECT as *mut core::ffi::c_void),
                Default::default(),
            );
        }
    }
    if rect.right > rect.left && rect.bottom > rect.top {
        return rect_of(rect);
    }
    // Last resort: full primary screen (loses the taskbar margin).
    rect_of(RECT {
        left: 0,
        top: 0,
        right: unsafe { GetSystemMetrics(SM_CXSCREEN) },
        bottom: unsafe { GetSystemMetrics(SM_CYSCREEN) },
    })
}

pub unsafe fn create(
    w: i32,
    h: i32,
    use_work_area: bool,
    show_hint: bool,
    font: Option<String>,
    ocr_scale: u32,
    ocr_language: Option<String>,
) -> Result<HWND> {
    let instance: HINSTANCE = GetModuleHandleW(None)?.into();
    let class = w!("dt-ocr.frame");
    let resource = windows::core::PCWSTR::from_raw(std::ptr::without_provenance(1));
    let app_icon = LoadIconW(Some(instance), resource).unwrap_or_default();
    let wc = WNDCLASSW {
        style: Default::default(),
        lpfnWndProc: Some(wndproc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: app_icon,
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as _),
        lpszMenuName: windows::core::PCWSTR::null(),
        lpszClassName: windows::core::PCWSTR(class.as_ptr()),
    };
    ensure!(
        RegisterClassW(&wc) != 0 || GetLastError() == ERROR_CLASS_ALREADY_EXISTS,
        "RegisterClassW(frame) failed: {}",
        windows::core::Error::from_win32()
    );
    let work = if use_work_area {
        work_area()
    } else {
        Rect {
            l: 0,
            t: 0,
            r: GetSystemMetrics(SM_CXSCREEN),
            b: GetSystemMetrics(SM_CYSCREEN),
        }
    };
    let pos = dt_common::layout::center(
        work,
        Rect {
            l: 0,
            t: 0,
            r: w,
            b: h,
        },
    );
    // Start hidden: only a draw/window/mode command should reveal the frame.
    let hwnd = CreateWindowExW(
        FRAME_EX_STYLE,
        class,
        w!("dt-ocr"),
        WS_POPUP,
        pos.l,
        pos.t,
        pos.w(),
        pos.h(),
        None,
        None,
        Some(instance),
        None,
    )?;
    SetLayeredWindowAttributes(hwnd, COLOR_KEY, 0, LWA_COLORKEY)?;
    // Status strip window: shown outside the frame (below/above) during spec
    // mode; hidden when the frame is full — then the strip renders inside.
    let status_class = w!("dt-ocr.status");
    let swc = WNDCLASSW {
        style: WNDCLASS_STYLES(0),
        lpfnWndProc: Some(status_wndproc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: HICON::default(),
        hCursor: HCURSOR::default(),
        hbrBackground: HBRUSH::default(),
        lpszMenuName: windows::core::PCWSTR::null(),
        lpszClassName: windows::core::PCWSTR(status_class.as_ptr()),
    };
    RegisterClassW(&swc);
    let status = CreateWindowExW(
        // NOT layered: a layered window that never calls
        // SetLayeredWindowAttributes/UpdateLayeredWindow composites nothing,
        // which is why the strip was invisible.
        WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
        status_class,
        w!("dt-ocr status"),
        WS_POPUP,
        0,
        0,
        0,
        0,
        Some(hwnd),
        None,
        Some(instance),
        None,
    )?;
    SetWindowLongPtrW(status, GWLP_USERDATA, hwnd.0 as isize);
    let _ = ShowWindow(status, SW_HIDE);
    SetWindowLongPtrW(
        hwnd,
        GWLP_USERDATA,
        Box::into_raw(Box::new(FrameState {
            work_area: use_work_area,
            mode: Mode::Move,
            ocr_running: false,
            spec: String::new(),
            show_hint,
            font_face: font.unwrap_or_else(|| "Consolas".to_string()),
            ocr_scale,
            ocr_language,
            status,
            inside: false,
            cursor_on: false,
            tune: None,
            entered_once: false,
            flash: false,
            mouse_drag: None,
        })) as isize,
    );
    // The frame never takes CJK text: with no input context there is no
    // IME composition, so `x` always arrives as a plain WM_CHAR (an IME
    // would otherwise swallow it into composition until Enter commits).
    let _ = ImmAssociateContextEx(hwnd, HIMC::default(), IACE_DEFAULT);
    logline(&format!("frame created: {pos:?}"));
    Ok(hwnd)
}

unsafe fn state(hwnd: HWND) -> &'static mut FrameState {
    &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut FrameState)
}

/// Like [`state`], but null-safe: during `CreateWindowExW` (e.g.
/// WM_WINDOWPOSCHANGED from the initial WS_VISIBLE) GWLP_USERDATA is not
/// set yet.
unsafe fn state_opt(hwnd: HWND) -> Option<&'static mut FrameState> {
    let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
    if p == 0 {
        return None;
    }
    Some(&mut *(p as *mut FrameState))
}

/// Live-update the status-strip prefs and the OCR scale (config hot-reload).
pub unsafe fn set_prefs(
    hwnd: HWND,
    work_area: bool,
    show: bool,
    font: Option<String>,
    ocr_scale: u32,
    ocr_language: Option<String>,
) {
    let s = state(hwnd);
    let font = font.unwrap_or_else(|| "Consolas".to_string());
    if s.show_hint != show || s.font_face != font {
        s.show_hint = show;
        s.font_face = font;
        let _ = InvalidateRect(Some(hwnd), None, false);
        let st = s.status;
        if !st.is_invalid() {
            let _ = InvalidateRect(Some(st), None, false);
        }
    }
    s.ocr_scale = ocr_scale.clamp(1, 4);
    s.ocr_language = ocr_language;
    if s.work_area != work_area {
        s.work_area = work_area;
        let r = screen_rect(hwnd);
        let _ = SetWindowPos(
            hwnd,
            None,
            r.l,
            r.t,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

/// Configured OCR upscale factor (read when a capture starts).
pub unsafe fn ocr_scale(hwnd: HWND) -> u32 {
    state(hwnd).ocr_scale
}

/// Requested OCR language tag, `None` = auto.
pub unsafe fn ocr_language(hwnd: HWND) -> Option<String> {
    state(hwnd).ocr_language.clone()
}

/// Enter or reset resize mode from the global hotkey. The command works
/// from Move, Pinned and every resize state, and focuses the frame so the
/// next key/mouse action can continue immediately.
pub unsafe fn begin_spec(hwnd: HWND) {
    if !IsWindowVisible(hwnd).as_bool() {
        return;
    }
    let first = !state(hwnd).entered_once;
    let next = transition(state(hwnd).mode, Event::ResizeHotkey { first_entry: first });
    state(hwnd).entered_once = true;
    if next == Mode::Tune {
        state(hwnd).tune = Some(Tune::new(TuneField::W, tune_limit(hwnd, TuneField::W)));
    }
    set_mode(hwnd, next);
    let _ = SetForegroundWindow(hwnd);
    let _ = SetFocus(Some(hwnd));
}

/// The frame's screen rect, when it is actually on screen.
pub unsafe fn visible_rect(hwnd: HWND) -> Option<RECT> {
    if !IsWindowVisible(hwnd).as_bool() {
        return None;
    }
    let mut r = RECT::default();
    GetWindowRect(hwnd, &mut r).ok()?;
    (r.right > r.left && r.bottom > r.top).then_some(r)
}

/// Capture feedback: flash the border cyan once (timer 4 clears it).
pub unsafe fn flash_capture(hwnd: HWND) {
    if !IsWindowVisible(hwnd).as_bool() {
        return;
    }
    state(hwnd).flash = true;
    let _ = InvalidateRect(Some(hwnd), None, false);
    let _ = SetTimer(Some(hwnd), 4, 180, None);
}

/// Switch to Move mode from any state (including Pinned), then focus the
/// frame. Global mode hotkeys are commands, not focus-dependent toggles.
pub unsafe fn to_move(hwnd: HWND) {
    set_mode(hwnd, transition(state(hwnd).mode, Event::MoveHotkey));
    let _ = SetForegroundWindow(hwnd);
    let _ = SetFocus(Some(hwnd));
}

/// Apply one explicit mode and all of its side effects in one place. This
/// prevents impossible combinations such as pinned+resize or Tune without
/// its tune data.
unsafe fn set_mode(hwnd: HWND, next: Mode) {
    let s = state(hwnd);
    s.mode = next;
    let cancel_mouse = next != Mode::Drag && s.mouse_drag.take().is_some();
    if cancel_mouse {
        let _ = ReleaseCapture();
    }
    if next == Mode::Input {
        s.spec.clear();
        s.cursor_on = true;
        let _ = SetTimer(Some(hwnd), 2, 500, None);
    } else {
        s.cursor_on = false;
        let _ = KillTimer(Some(hwnd), 2);
    }
    if next != Mode::Tune {
        s.tune = None;
        let _ = KillTimer(Some(hwnd), 3);
    }
    if !next.is_resize() {
        s.spec.clear();
        s.inside = false;
        if !s.status.is_invalid() {
            let _ = ShowWindow(s.status, SW_HIDE);
        }
    }

    // Pin is part of the mode, not an independent boolean. Click-through is
    // Pinned or an OCR worker in flight, so Move/Resize hotkeys unlock
    // automatically.
    apply_click_through(hwnd, s);
    if next.is_resize() {
        sync_status(hwnd);
    }
    redraw_now(hwnd);
    logline(&format!("mode: {next:?}"));
}

/// ctrl+shift+alt+t toggles between Pinned and Move from any state.
pub unsafe fn toggle_pin(hwnd: HWND) {
    set_mode(hwnd, transition(state(hwnd).mode, Event::PinHotkey));
}

/// Click-through whenever the frame must not eat input: pinned, or while an
/// OCR worker is reading the region underneath.
unsafe fn apply_click_through(hwnd: HWND, s: &FrameState) {
    let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
    let want = s.mode == Mode::Pinned || s.ocr_running;
    let new_ex = if want {
        ex | WS_EX_TRANSPARENT.0 as isize
    } else {
        ex & !(WS_EX_TRANSPARENT.0 as isize)
    };
    if new_ex == ex {
        return;
    }
    SetWindowLongPtrW(hwnd, GWL_EXSTYLE, new_ex);
    let _ = SetWindowPos(
        hwnd,
        None,
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
    );
}

/// OCR started/finished: keep the frame click-through until the mask owns
/// the region, then hand it back.
pub unsafe fn set_ocr_running(hwnd: HWND, on: bool) {
    let s = state(hwnd);
    if s.ocr_running == on {
        return;
    }
    s.ocr_running = on;
    apply_click_through(hwnd, s);
}

/// The frame's current screen rect.
pub unsafe fn screen_rect(hwnd: HWND) -> Rect {
    let mut r = RECT::default();
    let _ = GetWindowRect(hwnd, &mut r);
    rect_of(r)
}

unsafe fn to_drag(hwnd: HWND) {
    set_mode(hwnd, Mode::Drag);
}

unsafe fn to_input(hwnd: HWND) {
    set_mode(hwnd, transition(state(hwnd).mode, Event::InputKey));
}

/// To Tune from i/w/h navigation, preserving already-created tune data when
/// switching axes.
unsafe fn switch_tune(hwnd: HWND, field: TuneField) {
    let limit = tune_limit(hwnd, field);
    let s = state(hwnd);
    if s.tune.as_ref().map(|t| t.field) != Some(field) {
        s.tune = Some(Tune::new(field, limit));
    }
    set_mode(hwnd, transition(s.mode, Event::TuneKey));
}

/// Closing the frame hides it; any command summons it back without stealing
/// focus.
pub unsafe fn ensure_visible(hwnd: HWND) {
    if !IsWindowVisible(hwnd).as_bool() {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
}

/// Hides the frame while a new region is drawn; `place` shows it again.
/// The status strip is an owned popup, not a child: hiding the frame alone
/// leaves a thin, click-through grey bar on the desktop.
pub unsafe fn hide(hwnd: HWND) {
    if !IsWindow(Some(hwnd)).as_bool() {
        return;
    }
    if let Some(s) = state_opt(hwnd) {
        let _ = ShowWindow(s.status, SW_HIDE);
    }
    let _ = ShowWindow(hwnd, SW_HIDE);
}

/// Moves/sizes the frame to a screen rect (from the draw overlay or resize).
/// Any in-progress size input is discarded: a freshly placed frame starts
/// in the plain draggable state.
pub unsafe fn place(hwnd: HWND, rect: Rect) {
    set_mode(hwnd, Mode::Move);
    SetWindowPos(
        hwnd,
        Some(HWND_TOPMOST),
        rect.l,
        rect.t,
        rect.w(),
        rect.h(),
        SWP_SHOWWINDOW,
    )
    .ok();
    let _ = InvalidateRect(Some(hwnd), None, false);
    // Drawing and ctrl+alt+w enter the same focused Move state: Esc and the
    // Ctrl+S/C/P actions must work regardless of how the frame was placed.
    let _ = SetForegroundWindow(hwnd);
    let _ = SetFocus(Some(hwnd));
    logline(&format!("frame placed: {rect:?}"));
}

/// Arrow/hjkl step: 5 px with Ctrl held, else 1 px.
unsafe fn key_step() -> i32 {
    if GetKeyState(VK_CONTROL.0 as i32) < 0 {
        5
    } else {
        1
    }
}

/// Moves the frame by (dx, dy), clamped to its monitor work area.
unsafe fn nudge(hwnd: HWND, dx: i32, dy: i32) {
    let mut r = RECT::default();
    if GetWindowRect(hwnd, &mut r).is_err() {
        return;
    }
    let work = monitor_bounds(hwnd);
    let w = r.right - r.left;
    let h = r.bottom - r.top;
    let l = (r.left + dx).clamp(work.l, work.r - w);
    let t = (r.top + dy).clamp(work.t, work.b - h);
    let _ = SetWindowPos(
        hwnd,
        None,
        l,
        t,
        0,
        0,
        SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER,
    );
}

/// Drag-state keyboard sizing: arrows move their corresponding edge
/// outward; Shift moves it inward. The opposite edge stays fixed.
unsafe fn resize_from_key(hwnd: HWND, edge: Edge, delta: i32) {
    let mut r = RECT::default();
    if GetWindowRect(hwnd, &mut r).is_err() {
        return;
    }
    let rect = resize_edge(rect_of(r), monitor_bounds(hwnd), edge, delta);
    let _ = SetWindowPos(
        hwnd,
        None,
        rect.l,
        rect.t,
        rect.w(),
        rect.h(),
        SWP_NOACTIVATE | SWP_NOZORDER,
    );
}

/// Force pending paints now; used after modal sizing and mode changes.
unsafe fn redraw_now(hwnd: HWND) {
    let _ = InvalidateRect(Some(hwnd), None, false);
    let _ = UpdateWindow(hwnd);
    let st = state(hwnd).status;
    if !st.is_invalid() && IsWindowVisible(st).as_bool() {
        let _ = InvalidateRect(Some(st), None, false);
        let _ = UpdateWindow(st);
    }
}

/// 6 px border hit zone; returns the resize-handle code for the edge/corner
/// the point falls on, None for the interior.
fn edge_code(client: &RECT, pt: POINT) -> Option<isize> {
    let w = client.right - client.left;
    let h = client.bottom - client.top;
    let left = pt.x < EDGE;
    let right = pt.x >= w - EDGE;
    let top = pt.y < EDGE;
    let bottom = pt.y >= h - EDGE;
    if !(left || right || top || bottom) {
        return None;
    }
    let code = match (left, right, top, bottom) {
        (true, _, true, _) => HTTOPLEFT,
        (true, _, _, true) => HTBOTTOMLEFT,
        (_, true, true, _) => HTTOPRIGHT,
        (_, true, _, true) => HTBOTTOMRIGHT,
        (true, _, _, _) => HTLEFT,
        (_, true, _, _) => HTRIGHT,
        (_, _, true, _) => HTTOP,
        _ => HTBOTTOM,
    };
    Some(code as isize)
}

fn drag_edges(code: isize) -> (Option<Edge>, Option<Edge>) {
    let horizontal = match code as u32 {
        HTLEFT | HTTOPLEFT | HTBOTTOMLEFT => Some(Edge::Left),
        HTRIGHT | HTTOPRIGHT | HTBOTTOMRIGHT => Some(Edge::Right),
        _ => None,
    };
    let vertical = match code as u32 {
        HTTOP | HTTOPLEFT | HTTOPRIGHT => Some(Edge::Up),
        HTBOTTOM | HTBOTTOMLEFT | HTBOTTOMRIGHT => Some(Edge::Down),
        _ => None,
    };
    (horizontal, vertical)
}

unsafe fn set_edge_cursor(code: isize) {
    let id = match code as u32 {
        HTLEFT | HTRIGHT => IDC_SIZEWE,
        HTTOP | HTBOTTOM => IDC_SIZENS,
        HTTOPLEFT | HTBOTTOMRIGHT => IDC_SIZENWSE,
        _ => IDC_SIZENESW,
    };
    let cursor = LoadCursorW(None, id).unwrap_or_default();
    SetCursor(Some(cursor));
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        // Closing the frame (taskbar Close, Alt+F4) hides it instead of
        // destroying it: the process, its hotkeys and its tray icon are the
        // daemon and must outlive any frame.
        WM_CLOSE => {
            crate::mask::dismiss();
            hide(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            drop(Box::from_raw(
                GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut FrameState
            ));
            LRESULT(0)
        }
        WM_NCHITTEST => {
            let s = state(hwnd);
            if s.mode == Mode::Pinned || s.ocr_running {
                return LRESULT(HTTRANSPARENT as isize);
            }
            let pt = POINT {
                x: (lparam.0 & 0xFFFF) as u16 as i32,
                y: ((lparam.0 >> 16) & 0xFFFF) as u16 as i32,
            };
            let mut cpt = pt;
            let _ = ScreenToClient(hwnd, &mut cpt);
            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);
            // Border input is handled as client-mouse dragging below. This
            // avoids the system NC/modal sizing path, which was not
            // delivering WM_NCLBUTTONDOWN to this layered popup.
            if edge_code(&client, cpt).is_some() {
                return LRESULT(HTCLIENT as isize);
            }
            if s.mode.is_resize() {
                // Resize-mode interior: dead (movement locked).
                return LRESULT(HTNOWHERE as isize);
            }
            // Move mode: drag anywhere moves the frame.
            LRESULT(HTCAPTION as isize)
        }
        // Manual border drag: one captured-mouse path for Move and every
        // resize substate. Geometry is pure and unit-tested in interaction.
        WM_LBUTTONDOWN => {
            let mut pt = POINT {
                x: (lparam.0 & 0xFFFF) as u16 as i32,
                y: ((lparam.0 >> 16) & 0xFFFF) as u16 as i32,
            };
            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);
            let Some(code) = edge_code(&client, pt) else {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            };
            set_mode(hwnd, transition(state(hwnd).mode, Event::BorderPress));
            let mut start = RECT::default();
            if GetWindowRect(hwnd, &mut start).is_err() {
                return LRESULT(0);
            }
            let _ = ClientToScreen(hwnd, &mut pt);
            let (horizontal, vertical) = drag_edges(code);
            state(hwnd).mouse_drag = Some(MouseDrag {
                horizontal,
                vertical,
                start: rect_of(start),
                pointer: pt,
            });
            SetCapture(hwnd);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let Some(drag) = state(hwnd).mouse_drag else {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            };
            // Captured client coordinates move with the window; using them
            // feeds the resize back into the next delta.
            let mut pt = POINT::default();
            if GetCursorPos(&mut pt).is_err() {
                return LRESULT(0);
            }
            let rect = resize_drag(
                drag.start,
                monitor_bounds(hwnd),
                drag.horizontal,
                drag.vertical,
                pt.x - drag.pointer.x,
                pt.y - drag.pointer.y,
            );
            let _ = SetWindowPos(
                hwnd,
                None,
                rect.l,
                rect.t,
                rect.w(),
                rect.h(),
                SWP_NOACTIVATE | SWP_NOZORDER,
            );
            redraw_now(hwnd);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            if state(hwnd).mouse_drag.take().is_some() {
                let _ = ReleaseCapture();
                redraw_now(hwnd);
            }
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            if state(hwnd).mouse_drag.take().is_some() {
                redraw_now(hwnd);
            }
            LRESULT(0)
        }
        WM_SETCURSOR => {
            if (lparam.0 & 0xFFFF) == HTCLIENT as isize {
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let _ = ScreenToClient(hwnd, &mut pt);
                let mut client = RECT::default();
                let _ = GetClientRect(hwnd, &mut client);
                if let Some(code) = edge_code(&client, pt) {
                    set_edge_cursor(code);
                    return LRESULT(1);
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        // A queued WM_PAINT can starve behind hover hit-tests after a modal
        // drag, so repaint both windows synchronously on release. The system
        // move/size loop is foreign input (desktop-tools-rs rectangle): stop
        // our own captured border drag and land back in plain Move.
        WM_ENTERSIZEMOVE => {
            if let Some(s) = state_opt(hwnd) {
                s.mouse_drag = None;
            }
            let _ = ReleaseCapture();
            LRESULT(0)
        }
        WM_EXITSIZEMOVE => {
            if let Some(s) = state_opt(hwnd) {
                if s.mode.is_resize() {
                    set_mode(hwnd, Mode::Move);
                }
            }
            redraw_now(hwnd);
            LRESULT(0)
        }
        // Constrain every move/resize (spec-mode edge drags included) to the
        // monitor work area with a 16 px minimum.
        WM_WINDOWPOSCHANGING => {
            let wp = &mut *(lparam.0 as *mut WINDOWPOS);
            let mut r = RECT::default();
            let _ = GetWindowRect(hwnd, &mut r);
            let (cur_w, cur_h) = (r.right - r.left, r.bottom - r.top);
            let w = if (wp.flags & SWP_NOSIZE).0 != 0 {
                cur_w
            } else {
                wp.cx
            }
            .max(16);
            let h = if (wp.flags & SWP_NOSIZE).0 != 0 {
                cur_h
            } else {
                wp.cy
            }
            .max(16);
            let work = monitor_bounds(hwnd);
            if (wp.flags & SWP_NOMOVE).0 == 0 {
                wp.x = wp.x.clamp(work.l, (work.r - w).max(work.l));
                wp.y = wp.y.clamp(work.t, (work.b - h).max(work.t));
            }
            if (wp.flags & SWP_NOSIZE).0 == 0 {
                wp.cx = w;
                wp.cy = h;
            }
            LRESULT(0)
        }
        // Defocusing during resize always returns to Move. Activations
        // moving to another window of THIS process (the draw overlay) are
        // ignored so drawing does not cancel the frame interaction.
        WM_ACTIVATE => {
            let inactive = (wparam.0 & 0xFFFF) == 0; // WA_INACTIVE
            let mut own = false;
            if inactive && lparam.0 != 0 {
                let mut pid = 0u32;
                let _ = GetWindowThreadProcessId(
                    HWND(lparam.0 as *mut core::ffi::c_void),
                    Some(&mut pid),
                );
                own = pid == std::process::id();
            }
            if inactive && !own && state(hwnd).mode.is_resize() {
                set_mode(hwnd, transition(state(hwnd).mode, Event::Defocus));
            }
            LRESULT(0)
        }
        WM_CHAR => {
            match state(hwnd).mode {
                Mode::Tune => {
                    let ch = wparam.0 as u8 as char;
                    if ch.is_ascii_digit() {
                        if let Some(t) = state(hwnd).tune.as_mut() {
                            if t.digits.len() < 5 {
                                t.digits.push(ch);
                                tune_scheduled(hwnd);
                            }
                        }
                    }
                    LRESULT(0)
                }
                Mode::Input => {
                    // SizeSpec only: digits, x, %, /. The frame has no IME.
                    let ch = wparam.0 as u8 as char;
                    let s = state(hwnd);
                    if s.spec.len() < 16
                        && (ch.is_ascii_digit() || matches!(ch, '%' | '/' | 'x' | 'X'))
                    {
                        s.spec.push(ch);
                        invalidate_status_ui(hwnd);
                    }
                    LRESULT(0)
                }
                Mode::Move => {
                    let ch = (wparam.0 as u8 as char).to_ascii_lowercase();
                    if matches!(ch, 'h' | 'j' | 'k' | 'l') {
                        let step = key_step();
                        let (dx, dy) = match ch {
                            'h' => (-step, 0),
                            'l' => (step, 0),
                            'k' => (0, -step),
                            _ => (0, step),
                        };
                        nudge(hwnd, dx, dy);
                        return LRESULT(0);
                    }
                    DefWindowProcW(hwnd, msg, wparam, lparam)
                }
                Mode::Drag | Mode::Pinned => LRESULT(0),
            }
        }
        WM_KEYDOWN => {
            let mode = state(hwnd).mode;
            let key = VIRTUAL_KEY(wparam.0 as u16);
            let ch = (wparam.0 as u8 as char).to_ascii_lowercase();
            match mode {
                Mode::Tune => {
                    let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
                    let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
                    let step = if ctrl { 5 } else { 1 };
                    let sgn = if shift { -1 } else { 1 };
                    match key {
                        VK_RETURN => {
                            let _ = KillTimer(Some(hwnd), 3);
                            apply_tune(hwnd);
                            to_drag(hwnd);
                        }
                        VK_ESCAPE => {
                            set_mode(hwnd, transition(mode, Event::Escape));
                        }
                        VK_BACK => {
                            if !ctrl {
                                if let Some(t) = state(hwnd).tune.as_mut() {
                                    t.digits.pop();
                                }
                                tune_scheduled(hwnd);
                            }
                        }
                        VK_UP | VK_DOWN | VK_LEFT | VK_RIGHT => {
                            let field = state(hwnd).tune.as_ref().map(|t| t.field);
                            let delta = match (field, key) {
                                (Some(TuneField::W), VK_RIGHT) => step * sgn,
                                (Some(TuneField::W), VK_LEFT) => -step * sgn,
                                (Some(TuneField::H), VK_UP) => step * sgn,
                                (Some(TuneField::H), VK_DOWN) => -step * sgn,
                                _ => 0,
                            };
                            if delta != 0 {
                                tune_adjust(hwnd, delta);
                            }
                        }
                        _ => match (state(hwnd).tune.as_ref().map(|t| t.field), ch) {
                            (Some(TuneField::W), 'h') => switch_tune(hwnd, TuneField::H),
                            (_, 'l') => tune_adjust(hwnd, step * sgn),
                            (_, 'h') => tune_adjust(hwnd, -step * sgn),
                            (_, 'k') => tune_adjust(hwnd, step * sgn),
                            (_, 'j') => tune_adjust(hwnd, -step * sgn),
                            (_, 'w') => switch_tune(hwnd, TuneField::W),
                            (_, 'i') => to_input(hwnd),
                            _ => {}
                        },
                    }
                    invalidate_status_ui(hwnd);
                    LRESULT(0)
                }
                Mode::Input => {
                    match key {
                        VK_RETURN => {
                            let text = state(hwnd).spec.clone();
                            apply_spec(hwnd, &text);
                        }
                        VK_ESCAPE => set_mode(hwnd, transition(mode, Event::Escape)),
                        VK_BACK => {
                            if GetKeyState(VK_CONTROL.0 as i32) >= 0 {
                                state(hwnd).spec.pop();
                            }
                        }
                        _ => match ch {
                            'w' => switch_tune(hwnd, TuneField::W),
                            'h' => switch_tune(hwnd, TuneField::H),
                            _ => {}
                        },
                    }
                    invalidate_status_ui(hwnd);
                    LRESULT(0)
                }
                Mode::Drag => {
                    if key == VK_ESCAPE {
                        set_mode(hwnd, transition(mode, Event::Escape));
                        return LRESULT(0);
                    }
                    match ch {
                        'i' => to_input(hwnd),
                        'w' => switch_tune(hwnd, TuneField::W),
                        'h' => switch_tune(hwnd, TuneField::H),
                        _ => {
                            let edge = match key {
                                VK_LEFT => Some(Edge::Left),
                                VK_RIGHT => Some(Edge::Right),
                                VK_UP => Some(Edge::Up),
                                VK_DOWN => Some(Edge::Down),
                                _ => None,
                            };
                            if let Some(edge) = edge {
                                let sign = if GetKeyState(VK_SHIFT.0 as i32) < 0 {
                                    -1
                                } else {
                                    1
                                };
                                resize_from_key(hwnd, edge, key_step() * sign);
                            }
                        }
                    }
                    LRESULT(0)
                }
                Mode::Move => {
                    let step = key_step();
                    let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
                    let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
                    match key {
                        // Esc on a focused Move frame puts it away; the daemon
                        // keeps running and any command brings it back.
                        VK_ESCAPE => {
                            crate::mask::dismiss();
                            hide(hwnd);
                        }
                        _ if ctrl && shift && ch == 's' => crate::frame_quick_save(hwnd),
                        _ if ctrl && ch == 's' => crate::frame_save_as(hwnd),
                        _ if ctrl && ch == 'c' => crate::frame_copy(hwnd),
                        _ if ctrl && ch == 'p' => crate::frame_pin(hwnd),
                        VK_LEFT => nudge(hwnd, -step, 0),
                        VK_RIGHT => nudge(hwnd, step, 0),
                        VK_UP => nudge(hwnd, 0, -step),
                        VK_DOWN => nudge(hwnd, 0, step),
                        _ => return DefWindowProcW(hwnd, msg, wparam, lparam),
                    }
                    LRESULT(0)
                }
                Mode::Pinned => LRESULT(0),
            }
        }
        // OCR worker finished (numbers come from the capture it took; the
        // frame may have moved since, in which case the mask still matches
        // the pixels that were read).
        crate::ocr::WM_OCR_DONE => {
            let outcome = *Box::from_raw(lparam.0 as *mut crate::ocr::Outcome);
            set_ocr_running(hwnd, false);
            match outcome.words {
                Ok(words) if !words.is_empty() => {
                    if let Err(e) = crate::mask::show(hwnd, outcome.rect, words, outcome.snapshot) {
                        logline(&format!("mask failed: {e:#}"));
                    }
                }
                Ok(_) => logline("ocr: no words found"),
                Err(e) => logline(&format!("ocr failed: {e:#}")),
            }
            LRESULT(0)
        }
        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }
        // Blink the spec cursor at 2 Hz and keep the strip glued to the
        // frame while dragging/resizing.
        WM_TIMER if wparam.0 == 2 => {
            let s = state(hwnd);
            if s.mode == Mode::Input {
                s.cursor_on = !s.cursor_on;
                invalidate_status_ui(hwnd);
            }
            LRESULT(0)
        }
        // Debounced fine-tune apply: the geometry catches up 250 ms after
        // the last keystroke.
        WM_TIMER if wparam.0 == 3 => {
            let _ = KillTimer(Some(hwnd), 3);
            apply_tune(hwnd);
            LRESULT(0)
        }
        // Capture feedback: one cyan flash, then back to the normal border.
        WM_TIMER if wparam.0 == 4 => {
            let _ = KillTimer(Some(hwnd), 4);
            state(hwnd).flash = false;
            let _ = InvalidateRect(Some(hwnd), None, false);
            LRESULT(0)
        }
        WM_WINDOWPOSCHANGED => {
            let r = DefWindowProcW(hwnd, msg, wparam, lparam);
            if let Some(s) = state_opt(hwnd) {
                if s.mode.is_resize() && !s.inside {
                    if let Some(rc) = status_pos(hwnd) {
                        let st = s.status;
                        let _ = SetWindowPos(
                            st,
                            None,
                            rc.left,
                            rc.top,
                            rc.right - rc.left,
                            rc.bottom - rc.top,
                            SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                        );
                    }
                }
                // A foreign tool (desktop-tools-rs rectangle) can move or
                // resize the frame: nothing invalidates a layered window, so
                // the old border stays on screen next to the new one.
                if s.mouse_drag.is_none() {
                    redraw_now(hwnd);
                }
            }
            r
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn apply_spec(hwnd: HWND, text: &str) {
    if text.trim().is_empty() {
        // Enter on an empty buffer steps down to the drag substate (B).
        to_drag(hwnd);
        return;
    }
    match text.parse::<SizeSpec>() {
        Ok(spec) => {
            let cur: Rect = {
                let mut r = RECT::default();
                let _ = GetWindowRect(hwnd, &mut r);
                rect_of(r)
            };
            let rect = apply_size(spec, cur, monitor_bounds(hwnd));
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                rect.l,
                rect.t,
                rect.w(),
                rect.h(),
                SWP_NOACTIVATE,
            );
            logline(&format!("resize: {text} -> {rect:?}"));
            // Enter applies and stays in Input with a fresh buffer.
            let s = state(hwnd);
            s.spec.clear();
            s.cursor_on = true;
            let _ = SetTimer(Some(hwnd), 2, 500, None);
            sync_status(hwnd);
            redraw_now(hwnd);
        }
        Err(e) => {
            logline(&format!(
                "resize: `{text}` invalid ({e:#}); frame unchanged"
            ));
            state(hwnd).spec = text.to_string();
        }
    }
}

/// Live-applies the tuned value (quiet — no log per keystroke): keeps the
/// frame centered and locks the value so a center-preserving resize never
/// crosses the work-area edges.
unsafe fn apply_tune(hwnd: HWND) {
    let (field, v) = {
        let s = state(hwnd);
        let Some(t) = s.tune.as_ref() else {
            return;
        };
        let (fw, fh) = frame_size(hwnd);
        let v = if t.digits.is_empty() {
            match t.field {
                TuneField::W => fw,
                TuneField::H => fh,
            }
        } else {
            t.digits.parse::<i32>().unwrap_or(0)
        };
        (t.field, v)
    };
    if v < 1 {
        return;
    }
    let mut r = RECT::default();
    if GetWindowRect(hwnd, &mut r).is_err() {
        return;
    }
    let cur = rect_of(r);
    let work = monitor_bounds(hwnd);
    let cx = (cur.l + cur.r) / 2;
    let cy = (cur.t + cur.b) / 2;
    let (v, limit) = match field {
        TuneField::W => {
            let max = (2 * (cx - work.l).min(work.r - cx)).max(16);
            (v.clamp(16, max), max)
        }
        TuneField::H => {
            let max = (2 * (cy - work.t).min(work.b - cy)).max(16);
            (v.clamp(16, max), max)
        }
    };
    if let Some(t) = state(hwnd).tune.as_mut() {
        t.limit = limit;
    }
    let text = match field {
        TuneField::W => format!("{v}"),
        TuneField::H => format!("x{v}"),
    };
    let Ok(spec) = text.parse::<SizeSpec>() else {
        return;
    };
    let rect = apply_size(spec, cur, work);
    let _ = SetWindowPos(
        hwnd,
        Some(HWND_TOPMOST),
        rect.l,
        rect.t,
        rect.w(),
        rect.h(),
        SWP_NOACTIVATE,
    );
    let _ = InvalidateRect(Some(hwnd), None, false);
    let st = state(hwnd).status;
    if !st.is_invalid() {
        let _ = InvalidateRect(Some(st), None, false);
    }
}

/// Arrow/hjkl step in fine-tune mode: shift the current value and
/// re-apply (debounced).
unsafe fn tune_adjust(hwnd: HWND, delta: i32) {
    let v = {
        let s = state(hwnd);
        let Some(t) = s.tune.as_ref() else {
            return;
        };
        let (fw, fh) = frame_size(hwnd);
        let base = if t.digits.is_empty() {
            match t.field {
                TuneField::W => fw,
                TuneField::H => fh,
            }
        } else {
            t.digits.parse::<i32>().unwrap_or(0)
        };
        (base + delta).max(1)
    };
    if let Some(t) = state(hwnd).tune.as_mut() {
        t.digits = v.to_string();
    }
    tune_scheduled(hwnd);
}

/// Debounced fine-tune apply: repaint the readout immediately (typed digits
/// preview), apply the geometry 250 ms after the last keystroke — resizing
/// a fullscreen frame per keystroke is what made typing lag.
unsafe fn tune_scheduled(hwnd: HWND) {
    let _ = SetTimer(Some(hwnd), 3, 250, None);
    invalidate_status_ui(hwnd);
}

/// Repaints only the status UI: the outside strip, or just the inside line
/// rect on the frame. Invalidating the whole frame (a fullscreen layered
/// window) on every keystroke is what made typing lag.
unsafe fn invalidate_status_ui(hwnd: HWND) {
    let s = state(hwnd);
    let st = s.status;
    if s.inside {
        let mut client = RECT::default();
        let _ = GetClientRect(hwnd, &mut client);
        let line = inside_line(&client);
        let _ = InvalidateRect(Some(hwnd), Some(&line), false);
    } else if !st.is_invalid() {
        let _ = InvalidateRect(Some(st), None, false);
    }
}

/// Work-area clamp for the tuned axis at the frame's current center: a
/// center-preserving resize can extend at most to the nearer edge twice.
unsafe fn tune_limit(hwnd: HWND, field: TuneField) -> i32 {
    let mut r = RECT::default();
    if GetWindowRect(hwnd, &mut r).is_err() {
        return 0;
    }
    let work = monitor_bounds(hwnd);
    let (d1, d2) = match field {
        TuneField::W => (
            (r.left + r.right) / 2 - work.l,
            work.r - (r.left + r.right) / 2,
        ),
        TuneField::H => (
            (r.top + r.bottom) / 2 - work.t,
            work.b - (r.top + r.bottom) / 2,
        ),
    };
    (2 * d1.min(d2)).max(16)
}

unsafe fn paint(hwnd: HWND) {
    let s = state(hwnd);
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut client = RECT::default();
    let _ = GetClientRect(hwnd, &mut client);
    let key_brush = CreateSolidBrush(COLOR_KEY);
    let _ = FillRect(hdc, &client, key_brush);
    let frame_client = client; // pre-inset rect for the command bar
    let border = if s.flash {
        FLASH_CYAN
    } else {
        match s.mode {
            Mode::Move => BORDER_DRAG,
            Mode::Pinned => BORDER_PINNED,
            Mode::Input | Mode::Drag | Mode::Tune => BORDER_SPEC,
        }
    };
    let outer_brush = CreateSolidBrush(COLORREF(border));
    let inner_brush = CreateSolidBrush(COLORREF(border));
    // 2 px border (FrameRect is always one logical unit = one physical px).
    for brush in [outer_brush, inner_brush] {
        let _ = FrameRect(hdc, &client, brush);
        client.left += 1;
        client.top += 1;
        client.right -= 1;
        client.bottom -= 1;
    }
    let show = s.show_hint;
    let inside = s.inside;
    let cursor_on = s.cursor_on;
    if s.mode.is_resize() && show && inside {
        draw_inside_status(hdc, hwnd, &frame_client, cursor_on);
    }
    let _ = DeleteObject(key_brush.into());
    let _ = DeleteObject(outer_brush.into());
    let _ = DeleteObject(inner_brush.into());
    let _ = EndPaint(hwnd, &ps);
}

/// Bottom-inner line rect (2 px above the border) for the inside fallback.
fn inside_line(client: &RECT) -> RECT {
    RECT {
        left: client.left + 6,
        top: client.bottom - 26,
        right: client.right - 6,
        bottom: client.bottom - 2,
    }
}

/// Geometric result of laying out one status line.
struct Layout {
    cursor: RECT,
    /// Black input field behind the typed text + cursor.
    field: RECT,
    /// Clip zone for the typed input (never paints over the readout).
    input: RECT,
    /// Left edge where the input text is drawn; < input.left while scrolled
    /// (overflow: the cursor then hugs the zone's right edge, tail visible).
    text_x: i32,
    /// Right-aligned info zone (full or ellipsis text).
    info_zone: RECT,
    shown_info: String,
    /// Clickable `w`/`h` sub-rects; only when the full info fits.
    parts: Option<InfoParts>,
}

struct InfoParts {
    w_part: RECT,
    sep: RECT,
    h_part: RECT,
}

/// One status line laid out in char widths: 1 char margin on each end, the
/// cursor cell = 1 char sitting ON the last typed char, the input reserves
/// 8 char widths (nothing while `show_input` is false — the input zone is
/// hidden in B/C); the info on the right is ellipsis-clipped from its start
/// when tight, hidden below ~4 chars. `w_str`/`h_str` may carry clamped
/// tune digits.
unsafe fn status_layout(
    hdc: HDC,
    line: &RECT,
    spec: &str,
    w_str: &str,
    h_str: &str,
    show_input: bool,
) -> Layout {
    let (cw, th) = text_size(hdc, "0");
    let m = cw.max(8);
    let line_h = line.bottom - line.top;
    let text_h = (th + 2).clamp(10, line_h);
    let top = line.top + (line_h - text_h) / 2;
    let input_left = line.left + m + if show_input { cw } else { 0 };
    let right_edge = line.right - m;
    let (w_w, _) = text_size(hdc, w_str);
    let (sep_w, _) = text_size(hdc, " x ");
    let (h_w, _) = text_size(hdc, h_str);
    let info_w = w_w + sep_w + h_w;
    let avail = right_edge - input_left - m - if show_input { 8 * cw } else { 0 };
    let mut parts = None;
    let mut shown_info = String::new();
    let slot = if info_w > 0 && avail >= info_w {
        let left = right_edge - info_w;
        parts = Some(InfoParts {
            w_part: RECT {
                left,
                top,
                right: left + w_w,
                bottom: top + text_h,
            },
            sep: RECT {
                left: left + w_w,
                top,
                right: left + w_w + sep_w,
                bottom: top + text_h,
            },
            h_part: RECT {
                left: left + w_w + sep_w,
                top,
                right: right_edge,
                bottom: top + text_h,
            },
        });
        shown_info = format!("{w_str} x {h_str}");
        info_w
    } else if info_w > 0 && avail >= 4 * cw {
        let mut tail: String = format!("{w_str} x {h_str}");
        shown_info = loop {
            let cand = format!("…{tail}");
            if text_size(hdc, &cand).0 <= avail.saturating_sub(2) || tail.chars().count() <= 3 {
                break cand;
            }
            tail = tail.chars().skip(1).collect();
        };
        // The visible tail is the rightmost part of "w x h", i.e. the
        // height — clicking it tunes the height on narrow frames.
        parts = Some(InfoParts {
            w_part: RECT::default(),
            sep: RECT::default(),
            h_part: RECT {
                left: right_edge - avail,
                top,
                right: right_edge,
                bottom: top + text_h,
            },
        });
        avail
    } else {
        0
    };
    let zone_right = if slot > 0 {
        (right_edge - slot - m).max(input_left)
    } else {
        right_edge
    };
    let spec_w = if spec.is_empty() {
        0
    } else {
        text_size(hdc, spec).0
    };
    // The block cursor sits ON the last typed char (empty buffer = head
    // cell); on overflow the text scrolls left so the tail + cursor stay
    // visible at the zone's right edge.
    let zone_w = (zone_right - input_left).max(0);
    let (text_x, cursor_left) = if !show_input {
        (input_left, input_left)
    } else if spec_w + cw <= zone_w {
        (input_left, input_left + spec_w - cw)
    } else {
        (zone_right - cw - spec_w, zone_right - 2 * cw)
    };
    let cursor = RECT {
        left: cursor_left,
        top,
        right: cursor_left + cw,
        bottom: top + text_h,
    };
    Layout {
        cursor,
        field: RECT {
            left: input_left,
            top,
            right: if show_input { cursor.right } else { input_left },
            bottom: top + text_h,
        },
        input: RECT {
            left: input_left,
            top: line.top,
            right: zone_right.max(input_left),
            bottom: line.bottom,
        },
        text_x,
        info_zone: RECT {
            left: right_edge - slot,
            top: line.top,
            right: right_edge,
            bottom: line.bottom,
        },
        shown_info,
        parts,
    }
}

struct StatusDraw {
    spec: String,
    w_str: String,
    h_str: String,
    /// A substate: draw the input field, typed text and blinking caret.
    /// False in B (drag) and C (tune) — the readout takes the width.
    show_input: bool,
    tune_field: Option<TuneField>,
    /// The tuned value is clamped at the monitor limit — draw it red.
    tune_red: bool,
    font_face: String,
    layout: Layout,
}

/// Reads the parent frame state and measures the line. Uses its own DC with
/// the status font so metrics match the paint pass.
unsafe fn status_draw(parent: HWND, line: &RECT) -> StatusDraw {
    let s = state(parent);
    let spec = s.spec.clone();
    let (fw, fh) = frame_size(parent);
    let show_input = s.mode == Mode::Input;
    let tune_field = if s.mode == Mode::Tune {
        s.tune.as_ref().map(|t| t.field)
    } else {
        None
    };
    let tune_red = s.tune.as_ref().is_some_and(|t| {
        !t.digits.is_empty() && t.limit > 0 && t.digits.parse::<i32>().unwrap_or(0) >= t.limit
    });
    let (w_str, h_str) = tune_strings(s, fw, fh);
    let font_face = s.font_face.clone();
    let hdc = GetDC(Some(parent));
    let font = status_font(&font_face);
    let old = SelectObject(hdc, font.into());
    let layout = status_layout(hdc, line, &spec, &w_str, &h_str, show_input);
    let _ = SelectObject(hdc, old);
    let _ = DeleteObject(font.into());
    let _ = ReleaseDC(Some(parent), hdc);
    StatusDraw {
        spec,
        w_str,
        h_str,
        show_input,
        tune_field,
        tune_red,
        font_face,
        layout,
    }
}

/// The readout strings, showing in-progress tune digits for the tuned side
/// (clamped to the work-area limit so an unattainable size is never shown).
fn tune_strings(s: &FrameState, fw: i32, fh: i32) -> (String, String) {
    match &s.tune {
        Some(t) if !t.digits.is_empty() => {
            let v = t.digits.parse::<i32>().unwrap_or(0);
            let shown = if t.limit > 0 {
                v.clamp(1, t.limit)
            } else {
                v.max(1)
            };
            match t.field {
                TuneField::W => (shown.to_string(), fh.to_string()),
                TuneField::H => (fw.to_string(), shown.to_string()),
            }
        }
        _ => (fw.to_string(), fh.to_string()),
    }
}

unsafe fn frame_size(hwnd: HWND) -> (i32, i32) {
    let mut r = RECT::default();
    if GetWindowRect(hwnd, &mut r).is_err() {
        return (0, 0);
    }
    (r.right - r.left, r.bottom - r.top)
}

/// Fallback when there is no room outside the frame: the same status line
/// rendered at the bottom inner edge in yellow over the screen.
unsafe fn draw_inside_status(hdc: HDC, frame: HWND, client: &RECT, cursor_on: bool) {
    let line = inside_line(client);
    if line.top <= client.top + 2 {
        return; // frame too small for the inside line
    }
    let sd = status_draw(frame, &line);
    draw_status_line(hdc, &sd, cursor_on, BAR_YELLOW);
}

/// Width/height of `text` in the currently selected font (DrawText calc).
unsafe fn text_size(hdc: HDC, text: &str) -> (i32, i32) {
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    let mut rc = RECT::default();
    if wide.is_empty() {
        return (0, 16);
    }
    let _ = DrawTextW(hdc, &mut wide, &mut rc, DT_CALCRECT | DT_SINGLELINE);
    (rc.right - rc.left, rc.bottom - rc.top)
}

/// Bold font for the status line (`frame.font`, default Consolas); the
/// stock GUI font is too thin over arbitrary screen content.
unsafe fn status_font(face: &str) -> HFONT {
    let wide: Vec<u16> = face.encode_utf16().chain(std::iter::once(0)).collect();
    CreateFontW(
        -12,
        0,
        0,
        0,
        FW_BOLD.0 as i32,
        0,
        0,
        0,
        DEFAULT_CHARSET,
        OUT_DEFAULT_PRECIS,
        CLIP_DEFAULT_PRECIS,
        DEFAULT_QUALITY,
        DEFAULT_PITCH.0 as u32,
        windows::core::PCWSTR(wide.as_ptr()),
    )
}

/// Renders one laid-out status line: black input field with yellow text, a
/// full-cell cursor blinking yellow/black ON the last typed char, and the
/// `w x h` readout on the right — the tuned half inverted (black field +
/// yellow text, red when clamped at the monitor limit). The input zone is
/// not drawn at all while fine-tuning. `info_color` is the readout color
/// (black on the yellow strip, yellow inside the frame).
unsafe fn draw_status_line(hdc: HDC, sd: &StatusDraw, cursor_on: bool, info_color: u32) {
    let l = &sd.layout;
    let font = status_font(&sd.font_face);
    let old_font = SelectObject(hdc, font.into());
    let _ = SetBkMode(hdc, TRANSPARENT);

    let black = CreateSolidBrush(COLORREF(TEXT_BLACK));

    if sd.show_input {
        // 1. Black input field behind cursor + text.
        let _ = FillRect(hdc, &l.field, black);
        // 2. Typed input at text_x, clipped so it never paints over the readout.
        if !sd.spec.is_empty() && l.input.right > l.input.left {
            let _ = SetTextColor(hdc, COLORREF(BAR_YELLOW));
            let rgn = CreateRectRgn(l.input.left, l.input.top, l.input.right, l.input.bottom);
            let _ = SelectClipRgn(hdc, Some(rgn));
            let mut rc = RECT {
                left: l.text_x,
                top: l.input.top,
                right: l.input.right,
                bottom: l.input.bottom,
            };
            let mut wide: Vec<u16> = sd.spec.encode_utf16().collect();
            let _ = DrawTextW(
                hdc,
                &mut wide,
                &mut rc,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
            let _ = SelectClipRgn(hdc, None);
            let _ = DeleteObject(rgn.into());
        }
        // 3. Cursor cell: full char cell, yellow phase / black phase. The
        // char under the block sits on the cursor cell; redraw it in the
        // inverse color so it stays visible in both blink phases.
        let cur = CreateSolidBrush(COLORREF(if cursor_on { BAR_YELLOW } else { TEXT_BLACK }));
        let _ = FillRect(hdc, &l.cursor, cur);
        let _ = DeleteObject(cur.into());
        if let Some(c) = sd.spec.chars().last() {
            let _ = SetTextColor(
                hdc,
                COLORREF(if cursor_on { TEXT_BLACK } else { BAR_YELLOW }),
            );
            let mut rc = l.cursor;
            let mut w: Vec<u16> = c.to_string().encode_utf16().collect();
            let _ = DrawTextW(
                hdc,
                &mut w,
                &mut rc,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
        }
    }

    // 4. Readout.
    let _ = SetTextColor(hdc, COLORREF(info_color));
    if let Some(parts) = &l.parts {
        let draw_seg = |hdc: HDC, rc: RECT, text: &str| {
            let mut r = RECT {
                left: rc.left - 1,
                top: l.cursor.top,
                right: rc.right + 1,
                bottom: l.cursor.bottom,
            };
            let mut w: Vec<u16> = text.encode_utf16().collect();
            let _ = DrawTextW(
                hdc,
                &mut w,
                &mut r,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
        };
        let tuned = if sd.tune_red { TUNE_RED } else { BAR_YELLOW };
        let inv_w = sd.tune_field == Some(TuneField::W);
        let inv_h = sd.tune_field == Some(TuneField::H);
        if inv_w {
            let _ = FillRect(hdc, &parts.w_part, black);
            let _ = SetTextColor(hdc, COLORREF(tuned));
        }
        draw_seg(hdc, parts.w_part, &sd.w_str);
        let _ = SetTextColor(hdc, COLORREF(info_color));
        draw_seg(hdc, parts.sep, " x ");
        if inv_h {
            let _ = FillRect(hdc, &parts.h_part, black);
            let _ = SetTextColor(hdc, COLORREF(tuned));
        }
        draw_seg(hdc, parts.h_part, &sd.h_str);
    } else if !l.shown_info.is_empty() {
        let mut rc = l.info_zone;
        let mut sw: Vec<u16> = l.shown_info.encode_utf16().collect();
        let _ = DrawTextW(
            hdc,
            &mut sw,
            &mut rc,
            DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
    }
    let _ = DeleteObject(black.into());
    let _ = SelectObject(hdc, old_font);
    let _ = DeleteObject(font.into());
    let _ = SetBkMode(hdc, OPAQUE);
}

/// Screen rect for the outside strip: below the frame if the monitor work
/// area has room, else above; `None` when neither fits (caller falls back
/// to the inside line).
unsafe fn status_pos(hwnd: HWND) -> Option<RECT> {
    let mut r = RECT::default();
    if GetWindowRect(hwnd, &mut r).is_err() {
        return None;
    }
    let work = monitor_bounds(hwnd);
    if work.b - r.bottom >= STRIP_H + STRIP_GAP {
        Some(RECT {
            left: r.left,
            top: r.bottom + STRIP_GAP,
            right: r.right,
            bottom: r.bottom + STRIP_GAP + STRIP_H,
        })
    } else if r.top - work.t >= STRIP_H + STRIP_GAP {
        Some(RECT {
            left: r.left,
            top: r.top - STRIP_GAP - STRIP_H,
            right: r.right,
            bottom: r.top - STRIP_GAP,
        })
    } else {
        None
    }
}

/// Show the outside strip when there is room and `show_hint` is on; hide it
/// and switch to the inside line otherwise.
unsafe fn sync_status(hwnd: HWND) {
    let s = state(hwnd);
    if s.status.is_invalid() {
        return;
    }
    if !s.mode.is_resize() {
        s.inside = false;
        let _ = ShowWindow(s.status, SW_HIDE);
        return;
    }
    let mut show_outside = false;
    if s.show_hint {
        if let Some(rc) = status_pos(hwnd) {
            show_outside = true;
            let st = s.status;
            let _ = SetWindowPos(
                st,
                None,
                rc.left,
                rc.top,
                rc.right - rc.left,
                rc.bottom - rc.top,
                SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
        }
    }
    s.inside = !show_outside;
    if !show_outside {
        let st = s.status;
        let _ = ShowWindow(st, SW_HIDE);
    }
    let _ = InvalidateRect(Some(hwnd), None, false);
    if show_outside {
        let st = s.status;
        let _ = InvalidateRect(Some(st), None, false);
    }
}

unsafe extern "system" fn status_wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint_status(hwnd);
            LRESULT(0)
        }
        // Display-only: fully click-through. Fine-tune is entered with
        // w/h from the frame's keyboard states, not by clicking here.
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        _ => DefWindowProcW(hwnd, msg, w, l),
    }
}

/// Paints the outside strip: yellow background with the shared status line.
unsafe fn paint_status(hwnd: HWND) {
    let parent = HWND(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as _);
    let cursor_on = state(parent).cursor_on;
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut client = RECT::default();
    let _ = GetClientRect(hwnd, &mut client);
    let fill = CreateSolidBrush(COLORREF(BAR_YELLOW));
    let _ = FillRect(hdc, &client, fill);
    let _ = DeleteObject(fill.into());
    let sd = status_draw(parent, &client);
    draw_status_line(hdc, &sd, cursor_on, TEXT_BLACK);
    let _ = EndPaint(hwnd, &ps);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_stays_on_its_virtual_desktop() {
        assert_eq!(FRAME_EX_STYLE.0 & WS_EX_TOOLWINDOW.0, 0);
        assert_ne!(FRAME_EX_STYLE.0 & WS_EX_NOACTIVATE.0, 0);
        assert_ne!(FRAME_EX_STYLE.0 & WS_EX_APPWINDOW.0, 0);
    }

    const WORK: Rect = Rect {
        l: 0,
        t: 0,
        r: 1000,
        b: 700,
    };

    #[test]
    fn width_px_keeps_height_and_center() {
        let cur = Rect {
            l: 100,
            t: 100,
            r: 500,
            b: 400,
        }; // 400x300, center (300,250)
        let r = apply_size("1600".parse().unwrap(), cur, WORK);
        assert_eq!(r.w(), 1000); // clamped to work width
        assert_eq!(r.h(), 300); // height kept
        assert_eq!((r.l + r.r) / 2, 500); // center x kept after clamp
    }

    #[test]
    fn percent_and_fraction_use_work_area() {
        let cur = Rect {
            l: 300,
            t: 200,
            r: 500,
            b: 400,
        }; // center (400,300)
        let r = apply_size("50%".parse().unwrap(), cur, WORK);
        assert_eq!((r.w(), r.h()), (500, 350));
        assert_eq!((r.l + r.r) / 2, 400);
        assert_eq!((r.t + r.b) / 2, 300);
        let r = apply_size("1/2".parse().unwrap(), cur, WORK);
        assert_eq!((r.w(), r.h()), (500, 350));
    }

    #[test]
    fn oversized_specs_clamp_into_work_area() {
        let cur = Rect {
            l: 0,
            t: 0,
            r: 100,
            b: 100,
        };
        let r = apply_size("20000x20000".parse().unwrap(), cur, WORK);
        assert_eq!((r.w(), r.h()), (1000, 700));
        assert!(r.l >= WORK.l && r.r <= WORK.r);
        assert!(r.t >= WORK.t && r.b <= WORK.b);
    }

    #[test]
    fn result_never_degenerate() {
        let tiny_work = Rect {
            l: 0,
            t: 0,
            r: 20,
            b: 20,
        };
        let cur = Rect {
            l: 0,
            t: 0,
            r: 10,
            b: 10,
        };
        let r = apply_size("16x16".parse().unwrap(), cur, tiny_work);
        assert!(r.has_area());
    }
}
