# ROADMAP.md — desktop-tools-rs-ex

One phase per prompt round; the user physically verifies the on-screen effect
before the next phase starts. Design decisions and the interaction model live
in `AGENTS.md`; this file is only what is left to build.

## Done

- **P0.1** workspace + `dt-common` (keys, layout, exe-relative config).
- **P0.2** frame window: five-state interaction model, SizeSpec resize, mode
  hotkeys, one frame per virtual desktop, native tray, resident host. Single
  instance via `Local\dt-ocr.single`; duplicate `WM_HOTKEY`s inside 150 ms are
  dropped so repeated input cannot stack windows.
- **P0.3** OCR mask: click-through capture, frozen-snapshot overlay with
  per-word hit-testing, line-band fill, reader layout, `toggle` binding.
- **P0.4** OCR engines: persistent RapidOCR worker with native character boxes,
  Tesseract TSV words, automatic mixed-script routing and Windows OCR fallback.
  `mask_test.ps1` passes 25/25; `engine_routing_test.ps1` passes 8/8.
- **P0.5** auto-refresh: hot-reloadable fixed interval and/or on-change triggers,
  250 ms source-window sampling with two-sample debounce. Static content keeps
  the same mask; changed content re-OCRs without writing the clipboard.
  `auto_refresh_test.ps1` passes 7/7.
- **P0.6** release packaging (tag workflow, Scoop manifests; the `-ex` hash
  stays empty until the first release).

## P0.5.1 — Auto OCR lifecycle + colour (in progress)

- Esc on an auto-refresh mask dismisses it **and disarms auto OCR**; a later
  manual OCR arms it again. Changing source content dismisses the stale mask
  immediately, then performs the debounced re-OCR and shows the new mask.
- Replace the existing yellow resize/status/selection colours with orange;
  configured colour names remain unaffected.

## P1.1 — Drag-select quick OCR (next)

- `ctrl+alt+o` draws a region and OCRs it directly. On completion the frame
  flashes orange once and hides; the recognised text is copied to the clipboard.
- Completion uses a system notification or a configured third-party notifier;
  `[notify]` supports silent operation. Reuse the existing engines and joining.
- `ctrl+alt+w` then Ctrl+Enter remains the distinct **window OCR** workflow.

## P0.7 — OCR result presentation and editing

- **Four presentations**: `box` (default) leaves source text unchanged and shows
  an orange outline on click/selection; `box+mask` uses the configured font and
  automatically inverted text/backing colours for extreme light/dark sources,
  with an inverse-colour selection outline; `box+outside` puts a resize-like
  colour bar outside the frame with configured-font black text preserving lines
  and paragraphs; `notify` uses the quick-OCR notification settings and defaults
  to silent. The old `reader` mode is removed.
- Outside placement follows frame orientation: landscape prefers above/below,
  portrait prefers left/right; if both sides lack space, use the roomier side
  and align the floating panel to the available edge.
- Showing or toggling a window-OCR result turns the frame orange. Result-mode
  keys reuse the resize key positions but have result-specific meanings:
  `ctrl+alt+p` pins and enables auto OCR, then flashes the frame orange once.
- In outside mode, Ctrl+I edits the OCR text in place with basic editing keys;
  Esc leaves editing. Keep the existing per-line copy rules.
- **Selection polish**: token-range anchor→head selection, shift/ctrl extend,
  double-click paragraph, Ctrl+A; successful copy flashes cyan twice.

## P1 — Parity with the existing AHK tools + more

### P1.2 Focused-window capture + the screenshot feature set
- Done: `frame_window` (ctrl+alt+w) drops the frame on the focused window (or
  the window under the cursor, or the monitor) and switches to Move — no
  capture, so the OCR flow is "frame it, then ctrl+enter". `frame_window_test.ps1`.
- Done: `pin_shot` (ctrl+alt+p) pins the frame's rect when a frame is on screen
  (frame hidden for the grab, frame put away after) and the focused window
  otherwise; no clipboard and no file. Ctrl+Shift+S saves straight to
  `[capture] dir`, Ctrl+S opens a folder picker and saves there, Ctrl+C copies
  and closes, Ctrl+P is the explicit pin. Both `place()` callers focus the
  frame, so Esc works after a draw too. `pin_resize_test.ps1` (8 checks),
  `pin_no_frame_test.ps1` (3), `pin_border_modes_test.ps1` (2),
  `frame_focus_test.ps1` + the `pin` unit tests.
- Done: cyan capture flash; `[pin] border = hatch | line | none` (hatch is a
  6 px translucent diagonal shadow, `hatch-color` cyan by default; line uses
  `line-color`), and hovering an edge shows only the resize-arrow cursor. Pin
  resize is our own mouse capture, not `WS_THICKFRAME` (the NC border ate
  pixels; layered resize ghosted). No frame shadow.
- Done: tray menu uses the native `TrackPopupMenu` flow from desktop-tools-rs,
  with a foreground host owner and `WM_NULL` cleanup. Hiding a frame also hides
  its status strip; startup creates no visible frame. Single-instance mutex.
- Next: a floating action bar after a selection (copy / save / pin / flameshot /
  record on one rect), per-desktop pinning, and the bridges (flameshot `gui`,
  the recording tool once P1.3 lands). Flameshot stays the better free-form
  capture; dt-ocr owns the frame-for-OCR workflow.
- Done when: a focused-window capture reaches clipboard, disk and a pinned
  window, and the show-desktop case captures something sensible instead of
  failing.

### P1.3 Recording (wcap model)
- The maintainer's own experience: `wcap.exe` is unreliable — it mostly managed
  small windows and video-ish content, so a wrapper is not the goal. Plan on
  own capture (Desktop Duplication API → ffmpeg) and treat `wcap.exe` as a
  fallback only. Selecting the full screen in P1.2 should later become desktop
  recording with the same rect.
- Shared with P1.2: quality, fps, output config; locate `ffmpeg.exe` (Scoop).
- Decide the DDA details at start with context7 + reading wcap's source.

### P1.4 Window scaling (Magpie model)
- Research spike first (Magpie = D3D11 capture + shaders). The OCR path already
  taught us GDI upscaling (`[ocr] scale`, halftone), but that is still-image
  resampling — live window scaling needs a capture-and-present pipeline, so it
  does not shorten this phase. v1 may be a simple integer-ratio fullscreen
  stretch; lecture/presentation is the target. Decide the approach at start.

## P2 — Consolidation
- Extract `dt-common` back into the keytools-rs/kit plan (kit is the shared
  tray/hotkey/config library); revisit after desktop-tools-rs's folder rename
  lands.
