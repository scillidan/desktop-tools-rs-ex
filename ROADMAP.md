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
  the next source change. `mask_copy_test.ps1` copy/hide and stays-armed checks
  pass; the next-change GUI check needs physical verification because the test
  form can lose foreground.
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

### P1.5 Flameshot bridge
- Implemented as **experimental, disabled by default** (empty `[external]
  flameshot`; set it to the exe to enable): with the frame focused, `Ctrl+F`
  hides it, starts the Flameshot daemon, moves the cursor to the frame centre,
  launches `flameshot gui --region WxH+X+Y`, and leaves the frame hidden when
  Flameshot exits. Flameshot's known limitation: a preset region within 7 px of
  the monitor origin pins at the top-left corner, so the bridge insets
  edge-touching frames (up to 6 px lost on that edge). dt-ocr writes no file and
  touches no clipboard on this path. One handoff at a time.
  `flameshot_bridge_test.ps1` passes 8/8 (disabled-by-default gate, then hide,
  region log, daemon, frame stays exited, exit log).
- Implemented: `Ctrl+O` on a focused frame (not Pinned, any resize substate)
  saves a temp PNG and launches `[external] editor` with it; empty disables.
  Temp files live in `%TEMP%\dt-ocr` and files older than a day are removed at
  startup and before each handoff.
- Implemented: `Ctrl+A` on a focused frame (not Pinned) fills the whole current
  monitor including the taskbar, without changing the mode.
  `frame_fill_open_test.ps1` passes 8/8 (monitor fill keeps mode, Ctrl+O from a
  resize substate launches the editor, temp PNG arg, logs).
- Remaining: physical verification on a multi-monitor negative-coordinate
  region, the pin location when the Flameshot daemon was already running before
  the bridge, and whether `Print` conflicts with an existing Flameshot-hotkey
  configuration (dt-ocr logs a registration warning if the key is taken).

### P1.6 Recording bridge (separate plan)
- Done: `record` (`ctrl+alt+r`, global, id 11) starts/stops a recording of the
  visible frame's rect. The frame stays on screen with a red REC border while
  recording; the captured rect is inset 2 px so the border is never in the
  video, and stopping clears the border and gives the frame keyboard focus back.
  Start spawns `ffmpeg -f gdigrab -framerate <fps> -offset_x/-offset_y
  -video_size <w>x<h> -i desktop -c:v libx264 -preset veryfast -crf <crf> -g
  <fps*2> -pix_fmt yuv420p -movflags +frag_keyframe+empty_moov
  -thread_queue_size 1024 -max_muxing_queue_size 1024` with CREATE_NO_WINDOW and
  a piped stdin. Stop writes `q`, waits up to 5 s, kills if needed; daemon exit
  also stops an active session. Output is `[record] dir` + the `[record] name`
  template (`{date}`, `{time}`, `{datetime}`; default `dt-ocr-{date}-{time}`).
- Done: desktop audio on by default. ffmpeg here has no `wasapi` input, so
  `audio.rs` captures the default playback device through WASAPI loopback
  (16-bit PCM WAV, silence padded when nothing plays) and ffmpeg muxes it with a
  single video stream copy at stop; a dead capture only warns.
- Done: safeguards — even-sized rect (yuv420p), `min_free_mb` checked before
  start and every 3 s while recording, `max_minutes` cap (default 60, 0 = off),
  fragmented MP4 so a kill leaves a playable file, single-session flag,
  `crf 16..34`, fps 5..60, and stop-on-exit.
- Verification: `record_test.ps1` 12/12 (start/stop logs, frame hidden and
  restored, audio muxed, mp4 content, audio stream present, measured
  `mean_volume -29.6 dB` from a system sound played during the take, low-disk
  refusal + frame restore). `audio.rs` has header/format-conversion unit tests,
  `record.rs` covers even sizing and both auto-stop rules. ffprobe on a take:
  500x360 h264 + aac.
- Remaining: full-screen desktop recording at a high frame rate (DDA upgrade,
  see P1.3), cursor capture, per-application audio, and excluding our own
  mask/pins.
- Acceptance: start/stop from a frame produces a file whose content matches the
  framed rect; the frame never appears in the recording.

### P1.3 Recording (wcap model)
- The maintainer's own experience: `wcap.exe` is unreliable — it mostly managed
  small windows and video-ish content, so a wrapper is not the goal. Plan on
  own capture (Desktop Duplication API → ffmpeg) and treat `wcap.exe` as a
  fallback only.
- Status: the rect recorder shipped in P1.6 with `ffmpeg -f gdigrab` (simple,
  reliable for a rect). DDA stays the upgrade path for full-screen/high-fps
  capture; `shortcut:` is recorded in `dt-ocr/src/record.rs`.
- Shared with P1.2: quality, fps, output config; locate `ffmpeg.exe` (Scoop).
- Decide the DDA details at start with context7 + reading wcap's source.

### P1.4 Window scaling (Magpie model)
- Done (v1): `scale` (`ctrl+alt+z`, global, id 12) zooms the focused window into
  a fullscreen topmost window on its monitor through a DWM thumbnail
  (`DwmRegisterThumbnail`), aspect-preserved letterbox, source client area only.
  Esc / right-click / the hotkey again closes it. `scale_test.ps1` passes 6/6
  and `scale::fit` has unit tests for letterbox math. This is the "simple
  fullscreen stretch" v1; the zoom window takes focus, so the source cannot be
  typed into while zoomed.
- Remaining: sharpened upscaling (D3D11 capture + shader, Magpie model), input
  passthrough so the zoomed app stays usable, per-monitor/integer-ratio modes,
  and moving the module into the planned `dt-scale` crate when that lands.

## P2 — Consolidation
- Extract `dt-common` back into the keytools-rs/kit plan (kit is the shared
  tray/hotkey/config library); revisit after desktop-tools-rs's folder rename
  lands.
