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
  per-word hit-testing, `box` / `box+mask` / `box+outside` presentations,
  line-band fill, `toggle` binding. `box+outside` is an editable multi-line
  orange editor using `[mask] font` (default Sarasa Term SC Nerd, monospaced),
  preloaded with OCR text; no scrollbar, the control still scrolls by wheel and
  keys. Old `reader` mode removed. A result keeps a green 2 px edge (the mask
  paints it too, because the mask covers the frame) until dismissal.
- **P0.4** OCR engines: persistent RapidOCR worker with native character boxes,
  Tesseract TSV words, automatic mixed-script routing and Windows OCR fallback.
  `mask_test.ps1` passes 25/25; `engine_routing_test.ps1` passes 8/8.
- **P0.5** automatic OCR: armed only by `ctrl+shift+alt+o` (existing frame OCRs
  immediately, no frame draws), `[ocr] auto_delay_ms` debounce (default 1000 ms),
  250 ms source-window sampling with a PrintWindow → PW_RENDERFULLCONTENT →
  window-DC render fallback, immediate stale-mask dismissal, a settle timer that
  restarts on every material change, re-OCR without
  writing the clipboard. A second press stops auto OCR and closes the frame;
  `ctrl+alt+s` pauses and drops any in-flight result while the frame is resized;
  mask `Ctrl+C` copies and stays armed, returning Move.
- **P0.5.1** Esc/right-click dismiss a mask while an armed auto session stays
  armed; yellow resize/status/selection colours replaced with orange.
- **Quick OCR** `ctrl+alt+o` draws, OCRs, copies the whole text reflowed into
  paragraphs, holds GreenYellow while recognizing, shows LimeGreen on the result and
  closes the frame. `ocr_actions_test.ps1` passes 22/22.
- **Result copy** mask `Ctrl+C` copies the whole OCR result; without auto OCR it
  closes the frame, with auto OCR it returns to Move and stays armed, waiting for
  the next source change. `mask_copy_test.ps1` passes 11/11.
- **Image keys** from Move or any resize substate: `Ctrl+C` copies the frame
  image and closes, `Ctrl+Shift+S` quick-saves to `[capture] dir`, `Ctrl+S` opens
  a save-file dialog there, `Ctrl+P` pins. On a focused Pinned frame only
  `Ctrl+C` works and keeps the pin. `frame_image_keys_test.ps1` passes 8/8.
- **Outside editor** native EDIT with its own 200-step undo/redo, token-wise
  Ctrl+Left/Right (Shift+Ctrl included, line breaks handled), Ctrl+A, no
  scrollbar, and topmost floating when the preferred side is short.
  `editor_test.ps1` passes 15/15; `outside_lines_test.ps1` 2/2;
  `edit_keys_test.ps1` passes 19/19. External-editor experiment moved to
  `EXPERIMENT.md` (not product code).
- **P0.6** release packaging (tag workflow, Scoop manifests; the `-ex` hash
  stays empty until the first release).
- **Notify** optional tray balloon/toast after a manual OCR result or a quick
  OCR copy; `[notify] enabled` defaults to false and automatic refreshes stay
  silent. `notify_test.ps1` passes 2/2.

## P0.7 — OCR result presentation and editing

- Done: `box`, `box+mask` and `box+outside`, with inverse-colour bands/text and
  per-word inverse outlines; landscape outside editor sits below the frame,
  portrait to the right, and floats up/left when the side is short; native
  editing keys work (letters, Del, Ctrl+Del, selection), the editor adds
  undo/redo history, Ctrl+A and token-wise plain Ctrl+Left/Right; Esc closes.
- Done: optional tray notification after a manual OCR result and a quick-OCR
  copy; silent by default, never on automatic refreshes.
- Done: mask `Ctrl+C` copies the whole result and leaves the result view; with
  auto OCR armed the session stays armed and waits for the next source change.
  The old selection-polish idea
  (token-range anchor/head, double-click paragraph, copy flash) is dropped —
  click/drag copy and the outside editor cover the real use.

## P1 — Parity with the existing AHK tools + more

### P1.2 Focused-window capture + the screenshot feature set
- Done: `frame_window` (ctrl+alt+w) drops the frame on the focused window (or
  the window under the cursor, or the monitor) and switches to Move — no
  capture, so the OCR flow is "frame it, then ctrl+enter". `frame_window_test.ps1`.
- Done: `pin_shot` (ctrl+alt+p) pins the frame's rect when a frame is on screen
  (frame hidden for the grab, frame put away after) and the focused window
  otherwise; no clipboard and no file. From Move or a resize substate (not
  Pinned), Ctrl+Shift+S saves straight to `[capture] dir`, Ctrl+S opens a
  save-file dialog there, Ctrl+C copies and closes, Ctrl+P is the explicit pin.
  Both `place()` callers focus the frame, so Esc works after a draw too.
  `pin_resize_test.ps1` (8 checks), `pin_no_frame_test.ps1` (3),
  `pin_border_modes_test.ps1` (2), `frame_focus_test.ps1` + the `pin` unit
  tests.
- Done: cyan capture flash; `[pin] border = hatch | line | none` (hatch is a
  6 px translucent diagonal shadow, `hatch-color` cyan by default; line uses
  `line-color`), and hovering an edge shows only the resize-arrow cursor. Pin
  resize is our own mouse capture, not `WS_THICKFRAME` (the NC border ate
  pixels; layered resize ghosted). No frame shadow.
- Done: tray menu uses the native `TrackPopupMenu` flow from desktop-tools-rs,
  with a foreground host owner and `WM_NULL` cleanup. Hiding a frame also hides
  its status strip; startup creates no visible frame. Single-instance mutex.
- Next: a floating action bar after a selection (copy / save / pin), per-desktop
  pinning, and the bridges planned separately in P1.5 (Flameshot) and P1.6
  (recording). Flameshot stays the better free-form capture; dt-ocr owns the
  frame-for-OCR workflow.
- Done when: a focused-window capture reaches clipboard, disk and a pinned
  window, and the show-desktop case captures something sensible instead of
  failing.

### P1.5 Flameshot bridge (separate plan)
- Goal: any visible frame hands its rect to Flameshot's own region UI; dt-ocr
  does not grow a second screenshot editor (no colour picker, arrows, blur,
  OCR-of-screenshot, etc.).
- Proposed key: `Ctrl+Alt+F` on a frame (free today). Candidate command:
  `flameshot gui --region WxH+X+Y` (verify the exact CLI at implementation
  time; flameshot accepts a region/geometry argument in current releases).
- Flow: remember rect → hide the frame → launch Flameshot with that region →
  restore the frame when Flameshot exits. Missing `flameshot.exe` logs and
  leaves the frame alone. Config: `[external] flameshot = "flameshot"` (PATH or
  absolute).
- Acceptance: frame over a window + key opens Flameshot preselected to exactly
  that rect; no dt-ocr image/file is written; a missing executable is a visible
  warning, not a crash.
- Open: multi-monitor negative coordinates, and whether Flameshot should keep
  running after the frame is restored.

### P1.6 Recording bridge (separate plan)
- Goal: any visible frame's rect starts/stops the P1.3 recorder; the frame stays
  as the visual guide while recording.
- Proposed key: `Ctrl+Alt+R` on a frame (start/stop). Output goes to
  `[record] dir` with the same timestamped naming as captures; config carries
  fps/quality once P1.3 exists.
- Flow: start → frame stays visible but click-through? (decide) → second key
  stops → writes the file and logs the path. No frame = ignore.
- Depends on P1.3 (DDA + ffmpeg); do not build a second encoder here.
- Acceptance: start/stop from a frame produces a file whose content matches the
  framed rect; the frame never appears in the recording.
- Open: whether the frame should be hidden during capture (like image copy) or
  kept as a guide; audio; cursor.

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
