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
- **P0.6** release packaging (tag workflow, Scoop manifests; the `-ex` hash
  stays empty until the first release).

## P0.5 — Auto-refresh triggers (next)

- Manual key (existing) plus automatic re-OCR: fixed interval and/or
  on-change (frame region hash above a threshold) — the text/graphic-game
  use case.
- `ocr.auto = false | { every_ms, on_change }`, hot-reloadable.
- Done when: changing text inside the frame re-OCRs by itself, a static frame
  does not (no flicker, no CPU spin).

## P0.7 — Selection and feedback polish

- **Token-range selection** instead of rectangle intersection: anchor→head
  token ranges, shift/ctrl extend, double-click a paragraph, ctrl+A; keep the
  per-line copy rules.
- **Copy feedback**: press/selection shows a yellow outline, a successful copy
  flashes cyan twice (120/80/120 ms).
- **Reader limits**: no scrolling (text clips), a click copies everything, no
  per-word selection; paragraphs split on a vertical gap of ≥ one line
  height. Decide whether the reader earns scrolling before adding it.

## P1 — Parity with the existing AHK tools + more

### P1.1 Drag-select screen OCR
- Port `AHK-ScreenOCR`: draw a rect anywhere → OCR → result (copy/show).
  Shares engines + paragraph-join logic with dt-ocr.

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
