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
- `RegisterHotKey` ids are 1-based indexes into `Action::ALL` (1 draw,
  2 resize, 3 pin, 4 capture, 5 frame_window, 6 pin_shot, 7 move, 8 toggle);
  `PostMessage WM_HOTKEY`
  drives message-level behavior in harnesses.
- `InvalidateRect` alone does not repaint under hit-test traffic; the drag
  path calls `UpdateWindow` directly. `SystemParametersInfoForDpi` fails with
  E_INVALIDARG here — DPI comes from `GetDpiForWindow`; mask `size` is points
  (`em_px = pt * dpi / 72`).

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
RegisterHotKey; `ocr.rs` capture → upscale → Windows OCR → line merge;
`mask.rs` layered mask, overlay/reader rendering, selection, toggle; `text.rs`
`join_words`/CJK rules; `desktops.rs` one frame per virtual desktop; `tray.rs`
Shell_NotifyIconW; `icon.rs`/`build.rs` procedural icon.

## Capture sinks

`capture` (ctrl+enter) grabs the frame region for OCR and the clipboard.

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
or it would pin the wrong thing. Ctrl+P on a focused Move frame is the explicit
form of the same pin action. Ctrl+Shift+S saves that rect straight to
`[capture] dir`; Ctrl+S opens a folder picker (`SHBrowseForFolderW`) and saves
there; both close the frame. Ctrl+C copies it and closes the frame. Only those
save keys write PNG.

A pinned shot is a borderless layered tool window: drag inside to move
(`HTCAPTION`), and drag an image edge/corner through our own mouse capture to
resize with the source aspect (`pin::lock_aspect`). Hovering an edge paints a
cyan grip and `WM_SETCURSOR` gives the matching sizing cursor. `[pin] border`
selects the edge: `hatch` (default, 6 px translucent cyan diagonal shadow with a
1 px outline), `line` (1 px, `line-color` accepts names or `#RRGGBB`) or `none`.
Do not bring back `WS_THICKFRAME`: its non-client border ate image pixels while
shrinking, and a layered window was not invalidated during enlargement, leaving
ghosts. Esc/right-click closes; creation fits the image into the work area.
Existing pins keep the style they were created with. No per-desktop pinning yet.

`flash_capture` flashes the capture border cyan (RGB(0,255,255), timer 4,
180 ms). Esc on a focused Move frame puts it away; both `place()` callers
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
  (Simplified), each with the OCR feature; without zh, `auto` falls back to
  en and drops CJK.
- Fonts: `Sarasa Term SC Nerd` (mask default, the WezTerm face) and
  `Microsoft YaHei` (reader/fallback).

## Resident host

The process is a daemon; frames are disposable. `host.rs` owns a hidden
`WS_EX_TOOLWINDOW` STATIC-style window that carries every `RegisterHotKey`, the
tray icon and the 1 s config timer, because a destroyed window silently takes
its hotkey and tray registrations with it. Its tray callback must be handled in
the host window *procedure*: the shell sends (`SendNotifyMessage`) rather than
posts it, so it never appears in the `GetMessage` loop — handling it there made
right-click do nothing. `WM_CLOSE` on a frame hides it
(mask dismissed) instead of destroying it; the next command calls
`frame::ensure_visible` and the draw overlay re-shows it via `place`. Only the
tray Quit menu (or WM_QUIT) ends the process. Keep the popup owned by our own
window — a foreign foreground owner makes `TrackPopupMenu` return 0 without
showing anything; desktop-tools-rs `pick_sizer` is the reference.

P0.1 status: workspace + dt-common (`keys`, `layout`, `config` exe-relative
helpers) + dt-ocr (empty window, `dt-ocr.toml` next to exe, `dt-ocr.log`).
The kbdhook pipeline is intentionally NOT copied yet; P0.2 owns the hotkeys
and a minimal native tray.

P0.2 status: dt-ocr frame window complete. `interaction.rs` is the tested,
explicit five-state model: Move (cyan), Pinned (blue), and yellow resize
substates Input / Drag / Tune. `frame.rs` only applies those transitions;
there are no independent pinned/spec booleans. pin (`ctrl+shift+alt+t`)
toggles Pinned/Move from every state; ctrl+alt+m/s are global absolute mode
commands that unlock Pin and focus the frame. From outside resize, the
first ctrl+alt+s lands on Tune(width), later ones on Drag; inside resize it
resets Input. Esc:
Input→Drag, Drag/Tune→Move; any defocus exits resize to Move. Capture never
changes mode. Move drags by its interior/arrows/hjkl; a border press from
Move/Input/Tune enters Drag and the same captured mouse drag resizes
(no WS_THICKFRAME or NC sizing dependence). Mouse geometry uses GetCursorPos:
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
and merges duplicates after a desktop closes. `tray.rs` uses
Shell_NotifyIconW with exe icon resource 1 and a right-click Quit menu; the
shell sends its callback to the host window, which handles it in its own
window procedure (see Resident host).
`overlay.rs` = red draw band; `capture.rs` = GDI
→ clipboard CF_DIB + CF_BITMAP; `hotkeys.rs` = RegisterHotKey with the
ported vk map. `~\.temp\opencode\frame_state_test.ps1` verifies background
mode commands, Pin/Esc/defocus and real border geometry (mouse_event drag;
single-event SendInput did not deliver the button-down here).

P0.3 status: overlay mask complete. Capture keeps the image capture and starts
Windows.Media.Ocr on a worker thread (`ocr.rs`: top-down BGRA capture →
SoftwareBitmap → word boxes; zh-Hans-CN engine first, then en), the frame is
click-through while it runs, and `mask.rs` shows a topmost per-pixel-alpha
layered window over the frame: a frozen native-resolution BGRA snapshot as
the base, a black backing box per word with the text re-rendered white. Every
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

P0.3 mask fill: `[mask] fill = "lines"` (default) paints one continuous black
band per OCR line segment (word gaps < 3/4 em filled, tabs/columns separate);
the glyphs keep their OCR positions, so Latin/CJK spacing is exact and word
hit-testing stays aligned. `"boxes"` restores the per-word look. Selections
copy with `\n` between OCR lines. The new `toggle` binding (default
`win+shift+o`; Win+O is reserved by Windows) hides/shows the existing mask
with the same HWND, no re-OCR. `[mask]` also carries `font` (default Sarasa
Term SC Nerd, the WezTerm face), `size` in points (default 10 = WezTerm's
`font_size`; "auto" = per-line source height) and `layout = "overlay" |
"reader"`. Reader paints an opaque page and reflows the text at the fixed
size, paragraphs separated by a blank line; a click copies the whole text.

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
| P0.3 | `layout = "reader"`: opaque page, reflowed text, click copies all | a readable mode separate from the position-faithful overlay |
| P0.4 | RapidOCR measured: 4/4 lines exact at native res, ~1.6 s per CLI run, line boxes only | best quality by far; needs a persistent worker plus a word-splitting plan |
| P0.4 | `auto` routes by a language table; explicit keys force an engine | the smallest rule set that answers multi-engine routing |
| P0.6 | Releases and Scoop manifests use the `-ex` naming; the hash stays empty until the first release | assets and manifest autoupdate must agree; never call an unreleased package installable |

Every handoff must rebuild LAST and check exe mtime > all src mtimes.
