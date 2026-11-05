# AGENTS.md — desktop-tools-rs-ex

Extended desktop tools, the heavy siblings of desktop-tools-rs (window
snapping daemon). Where desktop-tools-rs owns global hotkeys + small window
ops, this repo owns OCR (frame + overlay + drag-select), recording, and
window scaling. Same user, same machine, same conventions.

## Relationship to desktop-tools-rs

- **Inherit conventions, do not depend on it.** No cargo dependency: its
  folder is slated to be renamed and its tray/keys code is meant to flow into
  keytools-rs/kit eventually. When reusing code, copy it into `dt-common`
  with a one-line provenance comment (`// from desktop-tools-rs src/keys.rs`).
- Worth copying on day one: `keys.rs` (Combo parsing), `layout.rs`
  (`SizeSpec`: `1600x1200`, `1/2`, `100%` — exactly the Ctrl+Alt+S size
  presets), the kbdhook pattern (Win-combo suppression), the tray icon
  pipeline, the rebuild-script + RegisterHotKey-probe verification workflow.

## Commands

- `cargo build` / `cargo build --release`
- `cargo test` — unit tests only
- `cargo clippy --all-targets` + `cargo fmt` — must be clean before handoff
- After every daemon/window change: kill, rebuild, relaunch automatically
  via `tools/rebuild.ps1` (taskkill → build → sync config next to exe → CIM
  launch → RegisterHotKey probe), then `tools/check_fresh.ps1` (exe newer
  than every source file). The user never builds or restarts anything
  manually and verifies only the on-screen effect.

## Conventions (inherited from desktop-tools-rs / keytools-rs)

- `anyhow` errors with context; a failed action logs a warning, the app
  keeps running — never crash the loop.
- Config: TOML next to the exe, seeded from a repo-root template on first
  run; one bad binding/preset is skipped with a warning, never fatal;
  hot-reload on change where the tool is long-running.
- Tests are inline `#[cfg(test)] mod tests` next to the code.
- Release profile: `lto = true`, `codegen-units = 1`, `strip = true`.

## Windows shell + machine rules (inherited, verified on this machine)

- **Never use bare `find`** — use `rg`/`grep` tools or `fff`; bare `find` is
  find.exe on Windows.
- The opencode shell wrapper kills child processes on command exit (job
  object): launch daemons via `Invoke-CimMethod Win32_Process Create`;
  `tasklist`/process listings are unreliable through it — verify liveness
  with RegisterHotKey conflict probes or CIM queries.
- SendInput-injected combos containing alt+shift are swallowed by the input
  stack here and never reach foreground apps; injected input generally
  cannot be used to test hotkeys — verify by physical press or probe.
- debug builds are console apps: clicking the console enables QuickEdit and
  suspends the process ("hotkeys randomly stop working"). Test release only.
- PowerShell 5.1 reads a BOM-less `.ps1` as ANSI/GBK; build CJK strings from
  code points or save UTF-8 with BOM. Writing TOML from PS needs BOM-less
  UTF-8 (`UTF8Encoding($false)`).
- Injected input cannot drive a popup menu here either (menu windows ignore
  synthetic mouse and keyboard): a tray menu can only be clicked physically.
- The verification harnesses are not headless: each opens a real WinForms test
  window, calls `SetForegroundWindow` and pins images. Never run them while the
  user may be working — tell them first and run the batch once at the end, or
  the focus stealing looks like the app is spawning windows.
- `RegisterHotKey` ids are 1-based indexes into `Action::ALL` (1 draw,
  2 resize, 3 pin, 4 capture, 5 frame_window, 6 pin_shot, 7 move, 8 toggle,
  9 quick_ocr, 10 auto_ocr, 11 record, 12 scale); `PostMessage WM_HOTKEY`
  drives message-level behavior in harnesses. Flameshot is not a global
  hotkey: it is the frame-local `Ctrl+F` handoff.
- `InvalidateRect` alone does not repaint under hit-test traffic; the drag
  path calls `UpdateWindow` directly. `SystemParametersInfoForDpi` fails with
  E_INVALIDARG here — DPI comes from `GetDpiForWindow`; mask `size` is points
  (`em_px = pt * dpi / 72`).
- A `PCWSTR` needs a NUL terminator: `"…".encode_utf16().collect::<Vec<_>>()`
  does not include one, and `SetWindowTextW` then reads past the buffer into
  heap data (the arbitrary CJK/`stdout -l eng…` suffix seen in the outside
  editor). Always append `0` (the `wide()` helper) before handing a string to a
  Win32 API that takes a pointer instead of a length.
- A multiline `EDIT` only breaks lines on `\r\n`; a bare `\n` collapses the
  whole result onto one visual line. Convert OCR output with
  `edit_newlines()` before `SetWindowTextW` (the clipboard keeps plain `\n`).

## Agent workflow

1. cq first (query before acting; propose non-obvious findings).
2. One ROADMAP phase per unit of work; the user physically verifies the
   on-screen effect before the next phase starts.
3. Rebuild + relaunch automatically after every change; hand the user a
   short physical-verification list, never a build step.

## Layout

- `dt-common` — keys/layout/config primitives copied from desktop-tools-rs
  (repo folder `desktop-tools-rs`) with provenance comments; shared by tools
- `dt-ocr` — OCR frame window, overlay mask, drag-select OCR, engines; native
  Quit tray and procedural exe/tray icon (`icon.rs`, `build.rs`)
- `dt-rec` (future) — recording (wcap model)
- `dt-scale` (future) — window scaling (Magpie model)

`dt-ocr/src` in one line each: `main.rs` config load, hotkeys, message loop,
dispatch, log; `host.rs` resident host window; `frame.rs` frame window, modes,
SizeSpec input/tune, status strip; `interaction.rs` pure five-state machine;
`overlay.rs` red draw band; `capture.rs` GDI BitBlt, clipboard, PNG sink; `hotkeys.rs`
RegisterHotKey; `ocr.rs` engine routing + Windows OCR fallback; `ocr/auto.rs`
250 ms source sampling + debounce; `ocr/external.rs`
persistent RapidOCR worker + Tesseract TSV; `scripts/rapidocr_worker.py` embedded
uv/ONNX worker; `mask.rs` layered mask, box/box+mask/box+outside rendering, selection, toggle; `text.rs`
`join_words`/CJK rules; `desktops.rs` one frame per virtual desktop; `tray.rs`
Shell_NotifyIconW; `menu.rs` native tray popup; `record.rs` ffmpeg gdigrab rect
recorder; `flameshot.rs` experimental Flameshot handoff; `scale.rs` DWM-thumbnail
window zoom; `icon.rs`/`build.rs` procedural icon.

## Capture sinks

`capture` (ctrl+enter) grabs the frame region for OCR and the clipboard. Every
OCR trigger turns the border GreenYellow (#ADFF2F) while recognition runs; a
successful result turns it green (LimeGreen #32CD32) and it STAYS green while
the mask or outside editor is open. The mask covers the frame rect, so it paints
the same 2 px green edge itself; the border returns to the mode colour when the
result is dismissed or hidden. A manual trigger also dims the frame to 66% for
~180 ms.

`quick_ocr` (ctrl+alt+o) draws a region, OCRs it, copies the whole text to the
clipboard, shows the GreenYellow trigger and then the LimeGreen result beat, and
closes the frame. No mask is shown. The copy reflows wrapped OCR lines back
into their paragraph (blank line between paragraphs), matching a direct screen
selection instead of emitting every visual line.

`auto_ocr` (ctrl+shift+alt+o) is the only way to enter automatic OCR. With no
frame it draws one; with a frame visible (any mode) it OCRs that frame
immediately. Each background change dismisses the stale mask after the
configured `[ocr] auto_delay_ms` debounce and re-OCRs. Pressing `ctrl+alt+s`
pauses the session, discards any in-flight result and returns the frame to
normal resize mode; `ctrl+shift+alt+o` starts a fresh session. The same hotkey
turns it off and closes the frame; hiding the frame any other way also stops it.
Mask `Ctrl+C` copies the whole result; with auto OCR armed the mask closes,
the frame returns to Move and stays on screen, the session stays armed and the
next source change triggers a fresh OCR after the settle delay. Drawing
(`ctrl+alt+d`) and moving/resizing never arm it; re-targeting with
`ctrl+alt+w` keeps an armed session armed and only restarts the comparison
against the new region (`ocr::retarget_auto`). Note that a fully occluded source
window that stops painting (a WinForms form behind a topmost frame) yields an
unchanged sample, so its change is not detected; real apps repaint.

`flameshot` is an **experimental, frame-local handoff**, not a global hotkey:
with the frame focused, `Ctrl+F` hides it, starts the Flameshot daemon (which
hosts pinned screenshots), moves the cursor to the frame centre so Flameshot's
overlay lands on the right monitor, runs `flameshot gui --region WxH+X+Y`, and
leaves the frame hidden after Flameshot exits. It is disabled by default — the
switch is `[external] flameshot = ""` (empty); set it to an exe name or path to
enable — because of a Flameshot limitation: a preset region within 7 px of the
monitor origin pins at the top-left corner, so the bridge must inset
edge-touching frames (losing up to 6 px on that edge). The region is expressed
relative to the frame's monitor. Flameshot keeps its own keys (Print included)
and its own editor. Only one handoff can be open at a time.

`Ctrl+O` on a focused frame (not Pinned) saves the frame to a temp PNG and
launches `[external] editor` with it (e.g. `photocraft`); the frame is put away.
It works from Move and every resize substate. Empty `editor` disables the key.
Handoff PNGs go to `%TEMP%\dt-ocr`; files older than a day are deleted at
startup and before each new handoff.

`Ctrl+A` on a focused frame (not Pinned) grows it to the whole monitor
(taskbar included, regardless of `[frame] work_area`) without changing the
mode, so a resize substate stays in that substate.

`record` (`ctrl+alt+r`, global) starts and stops a rect recording of the frame.
Start needs a visible frame: it clamps the rect to even dimensions (yuv420p
cannot encode odd ones), refuses to start below `[record] min_free_mb`, turns the
frame's border red (REC) and keeps it on screen as the recording indicator, makes
the frame click-through so the app being recorded still takes clicks, and
spawns ffmpeg with the `gdigrab` input at `[record] dir`, the
`[record] name` template (`{date}`, `{time}`, `{datetime}`; separators and
reserved characters become `_`), `[record] fps` (5–60), `[record] crf` (16–34),
large input/mux queues, and `+frag_keyframe+empty_moov` so a killed session is
still playable. The captured rect is inset 2 px inside that border, so the red
edge never lands in the video, and a recording owns the screen: starting it drops
any OCR mask, every other action logs `<action> ignored: recording (stop it
first)`, and automatic OCR is skipped, so no overlay can appear in the take. Desktop audio is captured separately with WASAPI
loopback of the default playback device (`audio = true` by default) into a WAV
and muxed into the mp4 with one stream copy at stop; silence is padded while
nothing plays so the track stays aligned with the video, and a missing device
only warns. Stop writes `q` to ffmpeg's stdin, waits up to 5 s, then kills,
clears the red border, re-shows the frame and gives it keyboard focus again. A
watchdog auto-stops at `[record] max_minutes` (default 60, 0 = no cap) or when
free space drops below `min_free_mb`. Output is
`dt-ocr-YYYYMMDD-HHMMSS.mp4` unless the name template says otherwise. A missing
frame logs `record ignored: no frame`; exiting the daemon stops an active
recording. Our own mask/pins are still recordable, so close them first.

`scale` (`ctrl+alt+z`, global) zooms the window the user is looking at — the
same target chain as `frame_window` (foreground → under the cursor), but the
HWND itself. It opens a borderless topmost window over that window's monitor and
presents the source through a DWM thumbnail (`DwmRegisterThumbnail`), letterboxed
with the aspect preserved, source client area only. Esc, a right-click, the same
hotkey again or closing the window ends the zoom. `shortcut:` the thumbnail is
hardware-composited and live but not sharpened; Magpie-style D3D11 upscaling is
the later upgrade. The zoom window takes focus, so the source app cannot be
typed into while zoomed.

Only `draw`, `frame_window`, `quick_ocr`, `auto_ocr`, the no-frame `pin_shot`,
`record` and `scale` may run without a visible frame; resize/pin/capture/move/
toggle log an ignore instead of resurrecting a hidden frame.

`frame_window` (ctrl+alt+w) puts the frame on the window the user is looking at
(focused window → topmost real window under the cursor → the monitor) and goes
to Move mode; it captures nothing. That is the OCR workflow: frame the window,
then ctrl+enter reads it. The target must be usable: our own classes
(frame/mask/overlay/host/status/pin), shell desktop/taskbar classes, our own
process, `WS_EX_NOACTIVATE` (IME and notification surfaces),
`WS_EX_TOOLWINDOW` and `WS_EX_NOREDIRECTIONBITMAP` (XAML overlays — the IME
candidate host is a full-screen one, so without this rule every target was the
whole screen) are rejected, and the under-cursor step walks the z-order instead
of using `WindowFromPoint`, which returns the overlay above the app. `grab`
retries three times with 150 ms gaps: right after a shell restart the screen DC
is stale and every GDI call on it fails.

`pin_shot` (ctrl+alt+p) pins an image and writes neither the clipboard nor a
file. A visible frame decides the source: its own rect, with the frame hidden
for the grab and put away after a successful pin. With no frame it pins the
focused window (same target chain). The command must not re-show a hidden frame,
or it would pin the wrong thing. On a focused frame in Move or any resize
substate: Ctrl+C copies the image and closes the frame; Ctrl+Shift+S saves the
rect straight to `[capture] dir` with the timestamped default name and closes;
Ctrl+S opens a `GetSaveFileNameW` dialog starting in `[capture] dir` with that
name prefilled, then saves there and closes; Ctrl+P is the explicit pin. Ctrl+C
also works on a Pinned frame while it holds keyboard focus, but then it only
hides the frame for the grab and re-shows it, so the pin survives; the other
keys stay disabled on Pinned. Cancel leaves the frame alone. Only the save keys
write PNG.

A pinned shot is a borderless layered tool window: drag inside to move
(`HTCAPTION`), and drag an image edge/corner through our own mouse capture to
resize with the source aspect (`pin::lock_aspect`). Hovering an edge shows only
the matching resize-arrow cursor (`WM_SETCURSOR`) — no painted grip blocks.
`[pin] border` selects the edge: `hatch` (default, 6 px translucent diagonal
shadow with a 1 px outline, `hatch-color` default cyan), `line` (1 px,
`line-color` accepts names or `#RRGGBB`) or `none`.
Do not bring back `WS_THICKFRAME`: its non-client border ate image pixels while
shrinking, and a layered window was not invalidated during enlargement, leaving
ghosts. Esc/right-click closes; creation fits the image into the work area.
Existing pins keep the style they were created with. No per-desktop pinning yet.

`flash_capture` flashes the image-capture border cyan (RGB(0,255,255)); OCR
triggers hold GreenYellow (#ADFF2F) until the result replaces it, then
the result is LimeGreen (#32CD32) on timer 4 for 180 ms before the 200 ms
timer reveals the mask/editor or closes a quick-OCR frame. A manual OCR trigger
also sets frame opacity to 168/255 (66%) and restores it on timer 5.
Esc on a focused Move frame puts it away; both `place()` callers
(a completed draw and ctrl+alt+w) focus the frame, so their Esc and Ctrl+S /
Ctrl+Shift+S / Ctrl+C / Ctrl+P behavior is identical. The daemon stays resident and any command brings it
back.

Clipboard details that cost a debugging session: CF_DIB rows are bottom-up
while our DIB section and the PNG encoder are top-down (flip on the way out),
and CF_BITMAP must be a plain `CreateCompatibleBitmap` copy — handing over the
DIB *section* handle reads as `null` in other processes (a .NET
`Clipboard.GetImage()` returns nothing). `OpenClipboard` is retried for ~300 ms:
IME hosts and clipboard managers hold it briefly. When the clipboard wedges for
good, every app in the session loses copy/paste and only an explorer restart (or
a logoff) clears it — that is machine state, not dt-ocr. Bisected 2026-10-28:
text writes stay healthy for minutes; writing an image wedges it within ~20
writes via both .NET/OLE and our raw CF_DIB, with or without CopyQ, so image
writes are the trigger and the holder is not CopyQ (`GetOpenClipboardWindow`
stays 0, `GetClipboardOwner` is explorer's `CLIPBRDWNDCLASS`). Keep image
clipboard writes best-effort.
WinRT's PNG encoder runs on a worker thread in an MTA: an async result polled on
the STA UI thread never completes, because that thread only finishes the
operation by pumping messages.

**The shell restarted**: `TaskbarCreated` is broadcast to every top-level
window, so the host window re-adds the tray icon (`tray::restore`); without it
an explorer restart silently removes the icon for the rest of the session.

## Machine prerequisites

- Windows OCR language packs (Settings → Language): English (US) and Chinese
  (Simplified), each with the OCR feature; without zh, the Windows fallback
  drops CJK.
- `uv` (Scoop `main/uv`) for the embedded RapidOCR worker; its PEP-723 metadata
  provisions `rapidocr-onnxruntime` on first use, so no manual pip install.
- Tesseract plus `eng` and `chi_sim` traineddata (Scoop `tesseract` and
  `tesseract-languages`). Auto's secondary pass needs only `eng`; a missing
  executable or language leaves the RapidOCR result intact.
- Fonts: `Sarasa Term SC Nerd` (mask default, the WezTerm face) and
  `Microsoft YaHei` (outside-panel fallback).
- Flameshot on `PATH` (or an absolute path in `[external] flameshot`) is only
  needed for the experimental `Ctrl+F` handoff; `[external] editor` names the
  app that `Ctrl+O` opens captures with. Both are empty/missing-safe.
- `ffmpeg` on `PATH` (or `[record] ffmpeg`) for `Ctrl+Alt+R`; `gdigrab` is
  built into the shared build.

## Resident host

The process is a daemon; frames are disposable. `host.rs` owns a hidden
`WS_EX_TOOLWINDOW` STATIC-style window that carries every `RegisterHotKey`, the
tray icon and the 1 s config timer, because a destroyed window silently takes
its hotkey and tray registrations with it. Frames are created without
`WS_VISIBLE`, so launching the daemon shows no frame or status strip; only a
draw/window/mode command reveals one. Its tray callback must be handled in
the host window *procedure*: the shell sends (`SendNotifyMessage`) rather than
posts it, so it never appears in the `GetMessage` loop — handling it there made
right-click do nothing. `WM_CLOSE` on a frame hides it and its status strip
(mask dismissed) instead of destroying it; the next command calls
`frame::ensure_visible` and the draw overlay re-shows it via `place`. Only the
tray Quit item (or WM_QUIT) ends the process. The tray menu is the native
`TrackPopupMenu` popup used by desktop-tools-rs (`menu.rs`): `CreatePopupMenu`,
one `AppendMenuW` row, the host foregrounded first, then
`TPM_BOTTOMALIGN | TPM_LEFTALIGN | TPM_RETURNCMD | TPM_NONOTIFY`; after dismissal
it posts `WM_NULL` and destroys the HMENU. That is the tray-icon/tray-rs flow,
not a self-drawn Win11-style window, and the owner must be foreground or a
popup can stay stuck on screen.

A named mutex (`Local\dt-ocr.single`) makes the process single-instance: a
second launch logs `another dt-ocr instance is already running; exiting` before
creating a host, frame or tray icon. Without it, quitting one tray leaves the
other instances running and looking like the app cannot be exited. Hotkeys
already use `MOD_NOREPEAT`; the message loop also drops the same action arriving
twice within 150 ms, so injected duplicates cannot stack capture/pin windows.

P0.1 status: workspace + dt-common (`keys`, `layout`, `config` exe-relative
helpers) + dt-ocr (empty window, `dt-ocr.toml` next to exe, `dt-ocr.log`).
The kbdhook pipeline is intentionally NOT copied yet; P0.2 owns the hotkeys
and a minimal native tray.

P0.2 status: dt-ocr frame window complete. `interaction.rs` is the tested,
explicit five-state model: Move (cyan), Pinned (blue), and orange resize
substates Input / Drag / Tune. `frame.rs` only applies those transitions;
there are no independent pinned/spec booleans. pin (`ctrl+shift+alt+t`)
toggles Pinned/Move from every state; ctrl+alt+m/s are global absolute mode
commands that unlock Pin and focus the frame. From outside resize, the
first ctrl+alt+s lands on Tune(width), later ones on Drag; inside resize it
resets Input. Esc:
Input→Drag, Drag/Tune→Move; any defocus exits resize to Move. Capture never
changes mode. Move drags by its interior/arrows/hjkl; a border press from
Move/Input/Tune enters Drag and the same captured mouse drag resizes without
WS_THICKFRAME or NC sizing; the border press explicitly focuses the frame so Esc
cannot remain with the previously OCR'd app (notably WezTerm). Mouse geometry
uses GetCursorPos:
WM_MOUSEMOVE client coordinates move with the frame and fed resize deltas
back into themselves, collapsing the frame to 16 px. Every drag step repaints
synchronously. Drag arrows move their matching edge (Shift inward, Ctrl ±5);
`i`→Input, `w`/`h`→Tune. Tune digits/arrows keep the 250 ms geometry debounce.
The 2 px border has exactly one mode color.
The status strip is display-only (below/above/inside fallback), Consolas or
`frame.font`, inverse-video SizeSpec input with a block caret; readout tune
is keyboard-entered, not clicked. SizeSpec uses per-axis DimSpec against the
configured monitor bounds. `[frame] work_area` defaults true (exclude the
taskbar); false uses the full monitor and hot-reloads. The frame needs
WS_EX_APPWINDOW (and no WS_EX_TOOLWINDOW): without a shell application view,
GetWindowDesktopId fails and it follows every desktop. `desktops.rs` keeps one
frame per desktop, creates/reuses it on the current desktop's global hotkey,
and merges duplicates after a desktop closes. Both `frame::place` and
`frame::ensure_visible` call `desktops::is_on_current` first, so a stale frame
handle can never be shown on the wrong desktop (it is hidden instead). Pins stay
`WS_EX_TOOLWINDOW` and global; per-desktop pins are not implemented. `tray.rs` uses
Shell_NotifyIconW with exe icon resource 1 and a right-click Quit menu; the
shell sends its callback to the host window, which handles it in its own
window procedure (see Resident host). `tray::notify` reuses that icon for the
optional OCR result balloon.
`overlay.rs` = red draw band; `capture.rs` = GDI
→ clipboard CF_DIB + CF_BITMAP; `hotkeys.rs` = RegisterHotKey with the
ported vk map. `~\.temp\opencode\frame_state_test.ps1` verifies background
mode commands, Pin/Esc/defocus and real border geometry (mouse_event drag;
single-event SendInput did not deliver the button-down here).

P0.3 status: overlay mask complete. Capture keeps the image capture and starts
Windows.Media.Ocr on a worker thread (`ocr.rs`: top-down BGRA capture →
SoftwareBitmap → word boxes; zh-Hans-CN engine first, then en), the frame is
click-through while it runs, and `mask.rs` shows a topmost per-pixel-alpha
layered window over the frame: a frozen native-resolution BGRA snapshot as the
base, then one of three presentations (`[mask] mode`): `box` leaves the source
text untouched, `box+mask` draws inverse-colour bands and text, `box+outside`
adds a preview bar outside the frame. Every
pixel is opaque, so the mask stays hit-testable and source scrolling cannot
misalign it. Click = that word on the
clipboard; press-drag rectangle = `text::join_words` paragraph (space between
Latin words, none inside CJK); Esc/right-click dismiss. `mask_test.ps1` drives
the whole cycle mechanically (known Latin+CJK window, posted
hotkey/click/drag/Esc/right-click).

P0.3 OCR quality: captures are upscaled before recognition (`[ocr] scale`,
default 4; GDI halftone) because native resolution turns 8-10 pt screen text
into noise. `[ocr] language` defaults to auto: the zh engine runs first, then
an en pass when Latin is present; `ocr::merge_lines` keeps lines with >=20%
CJK letters on zh and takes the en line for Latin ones (en drops CJK, zh
mangles long Latin). Rectangles are divided back by the scale. Measured on
10 pt Sarasa Mono SC: 4x + merge is near-exact; RapidOCR (AHK-ScreenOCR) is
exact at native res but ~1.6 s per CLI run and line-level only — that is the
P0.4 engine decision.

P0.3 mask presentation: `[mask] mode = "box" | "box+mask" | "box+outside"`
(default `box`). `box` shows untouched source pixels with an orange outline on
click/selection. `box+mask` re-renders words in the configured font on
inverse-colour bands (the dominant source colour is inverted for the band and
inverted again for text, so dark and light terminals both stay readable); its
selection outline uses the text colour. `box+outside` leaves the source pixels
untouched and opens a multi-line native `EDIT` bar, orange with black text,
using `[mask] font` (default Sarasa Term SC Nerd) at 12 px normal with fixed
pitch — the resize input box made multi-line. The OCR text is preloaded; every
normal editing key works. Ctrl+Left/Right (also with Shift) jumps to token
edges — word, symbol run such as `=`, or a line break — and stops at the word
end, never after trailing whitespace. The page keeps its own undo/redo history
(Ctrl+Z, Ctrl+Y or Ctrl+Shift+Z; consecutive typing within 700 ms is one step),
implements Ctrl+A, and leaves Ctrl+C/X/V, Ctrl+Backspace/Delete, Home/End and
Shift selection to the EDIT control. There is no scrollbar; the EDIT control
still scrolls with the wheel, arrows and PageUp/PageDown. Esc closes the result.
Landscape puts the page BELOW the frame, portrait to the RIGHT; when that side
is short (e.g. full-screen OCR) the topmost bar floats over the frame and shifts
up/left just enough to stay on the monitor. In `box+mask`, `[mask] fill = "lines"`
(default) paints one continuous band per OCR line segment (word gaps < 3/4 em
filled, tabs/columns separate); `"boxes"` restores the per-word look. In all
modes the glyphs keep their OCR positions, so Latin/CJK spacing is exact and
word hit-testing stays aligned. Selections copy with `\n` between OCR lines. The
`toggle` binding (default `win+shift+o`; Win+O is reserved by Windows) hides/
shows the existing mask and outside bar with the same HWNDs, no re-OCR.
`[mask] font` (default Sarasa Term SC Nerd, the WezTerm face) and `size` in
points (default 10 = WezTerm's `font_size`; "auto" = per-line source height)
apply to `box+mask`; the outside editor always follows `frame.font`. The old
`reader` mode is gone.

P0.4 status: `[ocr] engine = "auto" | "rapid" | "tesseract"` selects the
provider, hot-reloaded with the rest of the config. Auto runs RapidOCR first,
uses Tesseract's `eng` pass to replace predominantly Latin lines, and falls
back to Windows OCR (zh-Hans-CN then en-US) if RapidOCR cannot start. Explicit
`en` chooses Tesseract in auto; explicit `chi_sim`/`zh` or another PP-OCR code
chooses RapidOCR. Forced engines report an error instead of silently changing
provider. RapidOCR is a persistent `uv run` Python worker, launched with
`CREATE_NO_WINDOW`, using the embedded PEP-723 script
`dt-ocr/scripts/rapidocr_worker.py`; it returns native character boxes, grouped
into Latin words and individual CJK characters. Tesseract supplies TSV word
boxes. Temporary PNGs are deleted after each request. When one English row
matches several horizontally separated primary fragments (for example `87` and
`ScanTailor Advanced`), the merged result emits that row once instead of keeping
the fragments as duplicate lines. On the mixed sample,
RapidOCR takes ~6.6 s including its first model start and ~0.8–1.1 s afterwards;
Tesseract takes ~0.6–1.0 s. `mask_test.ps1` passes 25/25 including exact mixed
Latin+CJK selection — it forces `[mask] mode = "box+mask"` (the black backing only
exists there) and places a frame first, because frames start hidden;
`engine_routing_test.ps1` passes 8/8 with the same placement requirement.

P0.5 status: automatic OCR is OFF by default. `auto_ocr` (`ctrl+shift+alt+o`) is
the only way to arm it: with no frame it draws one, with a visible frame (any
mode) it OCRs immediately and starts a fresh generation. Pressing `ctrl+alt+s`
pauses the session, drops any in-flight result (`ocr::start` seq/`accept_result`),
dismisses the old mask and returns the frame to resize; `ctrl+shift+alt+o`
resumes. The same hotkey again stops automatic OCR and closes the frame; any
other hide also disarms it. Drawing, `frame_window`, moving and resizing never
arm it. `[ocr] auto_delay_ms` (default 1000, range 100–60000) is the debounce.
The 250 ms host timer samples the frame only while armed (config is polled every
fourth tick). The first sample establishes a 48×27 luma fingerprint baseline;
blank renders are ignored. The first material change (≥0.5% of samples beyond
the per-pixel delta) immediately dismisses the stale mask; every further change
restarts the settle timer, and OCR starts only after the source has been stable
for `auto_delay_ms`. Esc on a result hides it but leaves the armed session alone;
the next change OCRs again. Mask `Ctrl+C` copies the whole result and leaves the
armed session armed (it only closes the result view), returning the frame to
Move; the next source change triggers a fresh OCR. Sampling tries ordinary
`PrintWindow`, then `PW_RENDERFULLCONTENT` and finally the window DC, keeping the
first render with real contrast (plain `PrintWindow` returned a blank surface on
real apps here); `ocr_actions_test.ps1` covers the no-frame guard, outside
editor, arming/refreshing/pausing/switching off and quick OCR (22 checks),
`mask_copy_test.ps1` covers non-auto copy-and-hide plus auto copy staying armed
(10 of 11 mechanical checks; the next-change GUI case needs the test form to hold
foreground); `flameshot_bridge_test.ps1` covers the frame hidden during the
handoff, the region log, daemon start, frame stays exited and exit log (6 checks); and
`editor_test.ps1` covers the persistent green result border, the mono editor
font, exact preloaded text with no trailing heap garbage, the absence of a
scrollbar and pause/resume (15 checks); `outside_lines_test.ps1` covers
paragraph breaks and CRLF in the editor (2 checks); `edit_keys_test.ps1` covers
Ctrl+Z/Y, Ctrl+A and token-wise Ctrl+Left/Right including Shift+Ctrl across a
line break (19 checks).

P0.7 status: the result edge is LimeGreen (#32CD32) while a mask or outside
editor is open (the mask paints it too, because it covers the frame), and every
OCR trigger holds GreenYellow (#ADFF2F) while recognition runs. Mask `Ctrl+C`
copies the whole result and leaves the result view: normally the frame is put
away; with auto OCR armed the frame returns to Move and the session stays armed,
waiting for the next source change. `box+outside` is the native multi-line EDIT
described above.
`[notify] enabled` (default `false`, hot-reloaded) sends one tray balloon/toast
after a **manual** OCR result and after a successful quick-OCR copy; automatic
refreshes stay silent, and `NIIF_NOSOUND` keeps the notification quiet when
enabled. A missing tray icon or a shell refusal only logs a warning.
`notify_test.ps1` covers the manual result notification (2 checks). Image keys
(`Ctrl+C`, `Ctrl+Shift+S`, `Ctrl+S`, `Ctrl+P`) work from Move and every resize
substate; on Pinned only `Ctrl+C` works and keeps the pin. `frame_image_keys_test.ps1`
covers image copy and quick-save from resize plus pinned copy (8 checks).

## Design decisions (the part worth copying into other tools)

| Phase | Decision | Why |
|---|---|---|
| P0.1 | `dt-common` + `dt-ocr` crates | tools share config/keys/tray; recording and scaling arrive as their own crates |
| P0.2 | Edge drags use `GetCursorPos` and repaint each step | `WM_MOUSEMOVE` client coordinates move with the frame, so resize deltas fed back into themselves and collapsed it to 16 px |
| P0.2 | Work-area bounds default on; `[frame] work_area = false` uses the full monitor | follow the Windows default (exclude taskbar) with one hot-reloadable override shared by every sizing path |
| P0.2 | One process, one `WS_EX_APPWINDOW` frame per virtual desktop | a Shell application view is what makes desktop isolation work; per-process hotkeys avoid conflicts |
| P0.2 | Pin is one toggling key (`ctrl+shift+alt+t`), Move/Resize stay absolute commands | the user wanted one key to summon and release the frame; only `ctrl+alt+m/s` force a mode |
| P0.2 | A resident host window owns hotkeys, tray and timer; frame `WM_CLOSE` only hides | a destroyed window silently drops its `RegisterHotKey`/`Shell_NotifyIcon` entries, and frames are disposable |
| P0.3 | Overlay is one layered window with per-word hit-testing, click-through only while OCR runs | nothing is blocked during processing; only the rendered result takes input |
| P0.3 | Interim engine `Windows.Media.Ocr` (zh-Hans-CN first, then en) | word boxes with zero provisioning let the overlay be verified before P0.4 swaps the provider |
| P0.3 | Mask base = native BGRA snapshot from the OCR capture | frozen source pixels keep boxes aligned after scrolling; every layered pixel stays opaque and hit-testable |
| P0.3 | OCR captures upscaled 4x (GDI halftone) | native resolution turns 8-10 pt screen text into noise; clamped by `MaxImageDimension` + a 16 Mpx budget |
| P0.3 | `auto` = zh pass + en pass merged per line | zh mangles long Latin runs, en drops CJK; ≥20% CJK lines keep zh |
| P0.3 | `fill = "lines"`: one band per OCR line segment | connected look without reflow; glyphs keep OCR positions so spacing is exact and hit-testing aligned |
| P0.3 | `toggle` = `win+shift+o`, reuses the mask HWND | Win+O is reserved (RegisterHotKey 1409); toggling must not re-run OCR |
| P0.3 | Selection text = `join_words` per line + `\n` between lines | keeps line breaks, spaces between Latin words, none inside CJK |
| P0.3 | Mask text fixed size by default (`size`, points; `auto` = per-line median) | per-word sizing made words jump; 10 pt matches the user's WezTerm font |
| P0.3 | `mode = "box" | "box+mask" | "box+outside"` | source-faithful default, an inverse-colour visual mode, and an editable multi-line proofreading bar outside the frame |
| P0.4 | Persistent RapidOCR worker, native character boxes | avoids the ~1.6 s model reload per CLI call and gives the mask exact word hit-testing without guessing glyph widths |
| P0.4 | `auto` = RapidOCR + Tesseract Latin-line replacement, Windows OCR fallback | best mixed-script result; an unavailable external dependency never removes OCR entirely |
| P0.4 | External processes use `CREATE_NO_WINDOW`; worker stdin/stdout are UTF-8 | OCR never flashes a console, and CJK output is not corrupted by the Windows ANSI code page |
| P0.5 | Auto OCR is armed only by `ctrl+shift+alt+o`; `ctrl+alt+s` pauses and drops in-flight results | implicit always-on OCR popped masks while the user was only moving frames; resizing needs the result gone immediately |
| P0.5 | Auto sampling falls back PrintWindow → PW_RENDERFULLCONTENT → window DC | plain PrintWindow returns an all-black surface on some real apps, which silently disabled change detection |
| P0.7 | Result state keeps the frame/result edge green until dismissal | the mask covers the frame border, so a one-shot flash was invisible; the mask paints the same edge itself |
| P0.7 | Outside editor is a stock multi-line EDIT plus our undo/token keys | native editing/IME/selection stay intact; only undo history, token jumps, Ctrl+A and the mono font are ours |
| P0.7 | Notifications are opt-in and never fire on automatic refreshes | the feature is useful for a manual "OCR is ready" beat but becomes noise when auto OCR runs every second |
| P0.7 | Mask `Ctrl+C` copies all and keeps auto armed in Move | the result view is out of the way but the next source change still refreshes; only `ctrl+alt+s` pauses a session |
| P0.7 | Image keys are captured before the mode-specific key handling | Move and all resize substates behave the same; Pinned still accepts Ctrl+C while focused and keeps its pin |
| P1.2 | `Ctrl+S` is a save-file dialog, `Ctrl+Shift+S` is the directory quick-save | one key to name a file, one key for the no-dialog path; the dialog starts where captures normally go |
| P1.2 | `Ctrl+O` hands the capture to `[external] editor` via a temp PNG cleaned after a day | reuses a real image editor instead of growing one, and the temp folder cannot grow without bound |
| P1.6 | Recording is `ffmpeg -f gdigrab` over the frame rect, with the frame kept on screen as a red REC marker and the captured rect inset 2 px inside that border | the frame must not appear in its own video, but the user still needs a visible recording indicator |
| P1.6 | Audio is our own WASAPI loopback to WAV, muxed by ffmpeg at stop | these ffmpeg builds ship no `wasapi` input, and a separate WAV keeps the mux a stream copy |
| P1.4 | Window zoom v1 is a DWM thumbnail in a fullscreen topmost window | live and hardware-composited without a capture-and-present pipeline; shader upscaling can replace the presentation later |
| P1.5 | Flameshot handoff is experimental, frame-local `Ctrl+F`, disabled by an empty `[external] flameshot` | Flameshot's own Print/keys and behavior stay untouched, and its 7 px edge bug makes the handoff unreliable |
| P1.5 | Region is monitor-relative and inset from the screen origin when within 7 px | Flameshot pins a preset selection to (0,0) if its x or y is under 7 px, and interprets `--region` relative to the monitor that hosts its overlay |
| P0.6 | Releases and Scoop manifests use the `-ex` naming; the hash stays empty until the first release | assets and manifest autoupdate must agree; never call an unreleased package installable |

Every handoff must rebuild LAST and check exe mtime > all src mtimes.
