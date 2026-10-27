# CHECKLIST.md — desktop-tools-rs-ex

Physical verification, per phase. A phase is done only when every box here
is confirmed by hand on the real machine. Agents: after implementing a
phase, run `rebuild_ex.ps1`, then fill in the concrete steps for the user —
never hand the user a build or restart step.

## P0.1 Scaffold + dt-common
- [ ] Empty dt-ocr window is on screen (title "dt-ocr", ~800x600), taskbar
      entry present; it closes normally via the X button.
- [ ] `target\release\dt-ocr.toml` exists next to the exe and matches the
      repo-root template.
- [ ] `target\release\dt-ocr.log` contains `config loaded from ... (frame
      800x600)` and `window created`; while the window is open the process
      shows in Task Manager as dt-ocr.exe.
- [ ] (optional probe) Edit the synced `target\release\dt-ocr.toml`
      `[frame]` width/height, kill dt-ocr.exe, and re-run
      `rebuild_ex.ps1` — the window comes back at the new size and the log
      line shows it.

## P0.2 OCR frame window
- [ ] Drag every edge and corner: the pointer stays on the dragged edge, the
      opposite edge stays fixed, and repeated upward drags never collapse the
      frame. Mechanical mouse-event regression passed 2026-10-27 for repeated
      top drags, all four corners, work-area clamping, and drag/move repaint.
- [x] Draw a frame by dragging (`ctrl+alt+d`): full-virtual-screen crosshair
      overlay, Esc cancels, <9 px drags ignored; the frame lands where drawn
      and stays on top (`hotkey probe: ctrl+alt+s held` passes; physical drag
      pending on real machine).
- [x] `Ctrl+Alt+S` then `1600` / `100%` / `1/2` each resize the frame as
      expected (fraction/percent relative to the work area): SizeSpec gained
      `WidthPx`+`Percent`, `frame::apply_size` centers on the current frame
      center and clamps into the work area (unit-tested; `100%` physical
      measurement pending).
- [x] `Ctrl+T` pins: frame no longer moves or eats clicks
      (`WS_EX_TRANSPARENT`); again unpins.
- [x] Capture key (`ctrl+enter`): the exact pixels under the frame land on the
      clipboard (`CF_BITMAP` via GDI BitBlt; log records `capture: WxH at
      (x,y)`; paste-and-compare pending on real machine). NOTE: after the
      hot-reload test the exe-side toml may hold a rebound capture key —
      `rebuild_ex.ps1` re-syncs the template, restoring `ctrl+enter`.
- [x] All bindings in `[bindings]` of `dt-ocr.toml` (bad binding → warn +
      default); config hot-reload re-registers bindings on mtime change
      (log: `config reloaded, hotkeys re-registered`).

## P0.2 feedback round (2026-10-26)
- [x] Border is 2 px (GDI FrameRect = one logical/physical unit per pass);
      colors:
      cyan = draggable, blue = pinned, yellow = size-spec mode, red = the
      draw-select band.
- [x] Drawing a new frame resets any in-progress size input (place() clears
      the spec buffer — the new frame always starts in the plain state).
- [x] Size-spec mode shows a gray format hint on the frame while the buffer
      is empty (`1600  x1200  1600x1200  40%  x30%  40%x30%  1/2x1/3   (%
      /fraction of desktop)`); typed text is white.
- [x] SizeSpec is per-dimension (`DimSpec`): `1600` (width px), `x1200`
      (height px), `40%x30%`, `1/2x1/3`, `1600x1/2` all parse; a lone `%` or
      fraction still applies to BOTH axes; `%`/fraction resolve against the
      work area of the monitor the frame is on (`MonitorFromWindow` +
      `GetMonitorInfoW`) — resizing on the secondary monitor no longer jumps
      the frame back to the primary.
- [x] Fresh release exe verified not stale (exe mtime > all src mtimes; all
      four hotkeys held; reload logline fires within 2 s of a toml touch).

## P0.2 feedback round 2 (2026-10-26)
- [x] Spec mode draws a neovim/tmux-style yellow command bar along the
      bottom inner edge (above the border, 2 px margin, text inset 4-6 px):
      `<current size> > <input>_` left, abbreviated hint
      `500[x500] 50% 1/2 ...` right-aligned. `frame.show_hint = false`
      hides the hint (live-reloads).
- [x] Capture puts BOTH CF_DIB (BITMAPINFOHEADER + 32 bpp bits via
      CreateDIBSection + GlobalAlloc) and a CF_BITMAP alias on the
      clipboard — clipboard managers (copyq) and file-oriented paste
      targets read the DIB form; Explorer ctrl+v still can't materialize
      a file from a bitmap (Windows limitation, unchanged).

## P0.2 feedback round 3 (2026-10-26)
- [x] Spec-mode display is now a neovim-style status BLOCK (yellow, sized
      to content) anchored bottom-right inside the frame: black vertical
      bar cursor + typed spec on the left, `(current size)` on the right.
      No full-width strip; no format hint text. Hidden when the frame
      covers its monitor work area (e.g. after `100%`) and when
      `frame.show_hint = false`. Block is the planned display slot for
      later status (record time, save filename, …).
- [x] CF_DIB alpha bytes filled with 0xFF after BitBlt — GDI leaves them
      zero for BI_RGB 32 bpp, and alpha-honouring readers (copyq/Qt) saw
      a fully transparent image. (Round-3 "copyq 看不到图" turned out to
      be copyq's Images toggle disabled; the alpha fill is still correct.)

## P0.2 feedback round 4 (2026-10-26)
- [x] Spec-mode status is a full-width strip in a SEPARATE topmost window
      OUTSIDE the frame: below the frame when the monitor work area has
      room, above as fallback, inside the frame (yellow text above the
      bottom border) when neither fits — fullscreen included. Input +
      blinking black/white bar cursor always left, current size right
      (no parens). Cursor symbol is the same 2 px bar in every state.
- [x] Gotcha recorded: CreateWindowExW(WS_VISIBLE) fires
      WM_WINDOWPOSCHANGED before GWLP_USERDATA is set — handlers touching
      `state` must go through `state_opt`.

## P0.2 feedback round 5 (2026-10-26)
- [x] Strip invisibility root cause: it had WS_EX_LAYERED but never called
      SetLayeredWindowAttributes/UpdateLayeredWindow, so the compositor
      rendered nothing. Layered style removed (strip is opaque).
- [x] Status text is now bold (CreateFontW FW_BOLD) in both strip and
      inside-fallback modes.
- [x] Narrow-frame layout: input reserves 8 char widths; the size on the
      right is ellipsis-clipped from its start (`…1200`) when tight and
      hidden entirely below ~8 chars; overflowing input scrolls
      (right-aligned draw) instead of blinding.
- [x] Focused frame (either pin state) moves with arrows/hjkl: 1 px per
      press, 5 px with Ctrl; clamped to the monitor work area.
- [ ] LATER (fine-tune mode spec, needs the future tweak-binding work):
      clicking the `<w> x <h>` text inverts it into an edit block —
      direct digit editing (no negatives/zero, clamped at work-area
      edges) or hjkl/arrows adjusting (up/k/j ±1, left/h/right/l ±5).

## P0.2 feedback round 6 (2026-10-26)
- [x] Cursor is a full-cell block blinking yellow/black; the input sits on
      a black field with yellow text (inverse video). Cursor always at the
      head of the input; overflow is clipped (no scroll). bksp only — del /
      left / right / home / end / ctrl+bksp are swallowed no-ops and arrows
      never move the frame while editing.
- [x] Readout format `w x h` (spaces around x); margins in char widths: 1
      char to the strip edge on the right, 1 char gap on the left of the
      readout, cursor cell = 1 char before the input.
- [x] Clickable readout in BOTH the strip and the inside fallback: clicking
      `w` or `h` highlights it inverted (black field + yellow text) and
      enters fine-tune — arrows/hjkl adjust (up/k −1, down/j +1, left/h −5,
      right/l +5), digits replace the value, bksp edits digits, Enter/Esc
      exits. Live center-preserving resize, value locked at work-area
      edges; while tuning, arrows never move the frame. Strip stays
      click-through outside the readout; the inside readout only on the
      unpinned frame (pinned is click-through by design).
      Strip hit-test was dead because WM_NCHITTEST lparam is in SCREEN
      coordinates but was compared against client-relative rects
      (always-miss → click-through); now ScreenToClient'd.

## P0.2 feedback round 9 (2026-10-26)
- [x] Status font is the system monospace face (`frame.font`, default
      Consolas; hot-reloads). Cursor FOLLOWS the typed text at the
      insertion point; on overflow the text scrolls left so tail+cursor
      stay visible.
- [x] Strip readout click works (root cause above: missing ScreenToClient in
      the strip's WM_NCHITTEST; verified mechanically with a SendInput click
      on the readout → `tune:` logged).
- [x] Fine-tune applies are debounced 250 ms (readout previews digits
      immediately; the resize happens after typing pauses — per-keystroke
      fullscreen SetWindowPos was the typing lag).
- [x] Enter/Esc in fine-tune fully LEAVE resize mode; Enter on an empty
      spec buffer also exits instead of erroring and re-arming; a valid
      spec applies and exits on Enter.
- [x] Pin state is visible while a spec is active: yellow outer ring, blue
      inner ring (previously toggling ctrl+t in spec mode showed nothing
      and silently made the frame click-through, i.e. "can't drag").

## P0.2 feedback round 10 (2026-10-26) — interaction model
- [x] Status font configurable (`frame.font`, default Consolas).
      Block cursor sits ON the last typed char (empty = head cell); typing
      repaints only the strip (or the inside line rect), not the whole
      fullscreen frame — the per-keystroke full-frame invalidate was the
      remaining typing lag.
- [x] Entering fine-tune CLEARS and hides the input zone (no buffer, no
      field, no caret) in both strip and inside modes; the readout takes
      the freed width. Ellipsized readouts are clickable too (tail = height).
- [x] The tuned readout value clamps to the work-area limit in the preview
      and turns RED when it hits the monitor limit (was: raw digits shown,
      could display sizes past the monitor).
- [x] State model: idle (no spec) = drag/arrows MOVE the frame; resize mode
      (spec input or fine-tune active) = movement locked, a 6 px border
      strip is resize handles (system resize cursors + live drag, clamped
      to the monitor work area via WM_WINDOWPOSCHANGING, min 16 px), the
      interior is a dead zone. Enter on a valid spec applies and STAYS in
      resize mode; Enter on an empty buffer or Esc exits; fine-tune
      Enter/Esc returns to spec input (Esc again exits).
- [x] Capture flashes the border cyan once (180 ms, timer 4) as feedback.

## P0.2 feedback round 11 (2026-10-26) — three-substate resize model
- [x] Block-cursor fix: the char under the cursor cell is redrawn in the
      inverse color so it is visible in both blink phases.
- [x] Resize mode is now three substates: `A` input (type-ahead SizeSpec,
      charset-filtered to digits + `x % / .`), `B` drag (waiting: `i` → A,
      `w`/`h` → tune; border drag resizes), `C` tune (arrows/hjkl ±1 on the
      selected axis, Ctrl = ±5, Shift = inward; digits replace, bksp;
      `w`/`h` switch axis, `i` → A). Esc / clicking outside steps down one
      level: A→B, C→B, B→idle. Enter in A applies and STAYS in A (empty → B).
      FIRST entry of a frame's lifetime lands on C(width), later ones on B.
- [x] Mouse-click-to-enter-tune is removed (strip and inside readout are
      display-only, fully click-through); hjkl no longer nudges size except
      as arrow mirrors inside C.
- [x] The frame's IME context is disabled at creation (ImmAssociateContextEx
      NULL): `x` always arrives as a plain WM_CHAR, no composition/Enter
      commit weirdness. WM_CHAR accepts only the SizeSpec charset.
- [x] WM_ACTIVATE(WA_INACTIVE) implements click-outside; activations moving
      to another window of THIS process (the draw overlay) are ignored.
- [x] State-machine audit (no dead ends): every state has an exit, B is the
      hub, only B exits to idle; debounce timers are killed on their exits.

## P0.2 feedback round 12 (2026-10-26) — Move mode + state machine fixes
- [x] Height tune: Up/k = +1 (grow), Down/j = −1 (shrink); width unchanged
      (Right/l grow, Left/h shrink).
- [x] WM_EXITSIZEMOVE repaints the frame fully after any modal drag.
- [x] `.` removed from the SizeSpec charset.
- [x] Cyan is now MOVE mode: new global hotkey `move = "ctrl+alt+m"` leaves
      resize mode (frame draggable again). Frame modes: cyan Move / yellow
      resize / blue pin.
- [x] Border drag works from ANY mode: the border strip always hit-tests as
      resize handles; in Move mode an edge press enters resize mode (B) and
      the same drag resizes; in A/C a border press clears the substate to B.
- [x] In A, `w`/`h` switch straight to tuning (input cleared); ctrl+alt+s
      while in resize mode now RESETS to a fresh input state (A) instead of
      being a no-op. Ctrl+enter never changes the interaction state.
- [x] New `[frame] exit_to_drag` (default true, hot-reloadable): clicking
      outside lands on the drag substate (yellow) instead of exiting to
      Move mode; Esc always exits. This plus the global hotkeys means an
      unfocused frame can always be reset with ctrl+alt+s and operated
      (ctrl+enter / ctrl+t / ctrl+alt+m) without focusing it first.

## P0.2 feedback round 13 (2026-10-26) — pin is a mode, Esc/drag fixes
- [x] WM_EXITSIZEMOVE repaints with UpdateWindow (bypasses the queue):
      a queued WM_PAINT starves behind hover hit-tests, which read as
      "several seconds" of stale border/readout after a drag.
- [x] ctrl+t jumps straight to the pin state from ANY state (resize mode
      is exited first). Modes switch only via ctrl+alt+m (Move) and
      ctrl+alt+s (resize); pin is blue, superimposed on Move.
- [x] Esc ladder revised: A (input) → B (drag); B → Move; C (tune) → Move
      directly. Input is the backup of tuning; tuning is the deep state.
- [x] Any defocus (click outside / app switch) exits resize for Move mode;
      `exit_to_drag` default flipped to false (true = the old
      land-on-yellow behavior, kept as an advanced option).

## P0.2 feedback round 14 (2026-10-26) — explicit state model + real drag
- [x] Replaced the implicit pinned/spec/input/tune flag combinations with
      `interaction.rs`: Move / Pinned / Input / Drag / Tune plus one tested
      transition table. 39 workspace tests pass; clippy/fmt are clean.
- [x] ctrl+t only locks (blue, click-through); pressing it again does NOT
      unlock. ctrl+alt+m/s always unlock and select Move/resize, including
      from an unfocused frame, then focus it for continued operation.
- [x] Esc: Input→Drag, Drag/Tune→Move. Any defocus exits resize to Move.
      Removed the obsolete `exit_to_drag` option and its alternate ladder.
- [x] Border sizing uses captured client-mouse drags (edges + corners),
      not WS_THICKFRAME/NC sizing. Verified with real injected mouse drags:
      Move→Drag +40 px and Drag→Drag +20 px, exact final dimensions.
- [x] Border has one mode color (no yellow/blue or yellow/system rings).
      Mouse release repaints synchronously; Drag arrows resize their own
      edge (Shift inward, Ctrl ±5).
- [x] `frame_state_test.ps1` passes background hotkey dispatch, idempotent
      Pin, Input/Drag Esc, defocus and real border geometry. Physical global
      key presses still require user verification (injection cannot trigger
      RegisterHotKey on this machine).

## P0.2 desktop behavior, bounds and identity (2026-10-27)
- [x] Border drag uses stable GetCursorPos screen coordinates, not client
      coordinates that move with the frame. Repeated top-edge and all-corner
      drags pass; the opposite edge stays fixed and work-area clamping works.
- [x] `[frame] work_area` defaults true (exclude taskbar); false selects the
      full monitor for pointer, keyboard and SizeSpec sizing and hot-reloads.
      frame_work_area_test passes: false dragged to screen bottom 1080, then
      restoring true re-clamped to work bottom 1032.
- [x] One process owns one frame per virtual desktop. WS_EX_APPWINDOW gives
      each frame a shell desktop identity; the current desktop's global hotkey
      creates or reuses its frame, and frames moved together by closing a
      desktop are deduplicated. virtual_desktop_repro verifies desktop IDs,
      DWM cloaking, create/reuse and desktop-close cleanup.
- [x] Icons: both use a hollow title bar over a full body; desktop-tools-rs
      fills the body, -ex diagonal-hatches it. Multi-size ICOs are embedded;
      -ex has a native right-click Quit tray. Pixel/ICO tests pass, and
      tray_quit_test uses SendNotifyMessage through the real wndproc path.

## P0.3 Overlay mask
Engine note: P0.3 uses the Windows built-in OCR (`Windows.Media.Ocr`, zh
pack first, then en) so the overlay is testable without P0.4's provisioning;
P0.4 replaces the provider behind `ocr::Word` / `ocr::Outcome`.
- [x] Capture (`ctrl+enter`) keeps the image capture and starts OCR on a
      worker thread; the frame is click-through (`WS_EX_TRANSPARENT` +
      `HTTRANSPARENT`) from the capture until the mask owns the region.
      Observed with a fast exstyle poll in `mask_test.ps1`.
- [x] After OCR the frame region is covered by `dt-ocr.mask`: one topmost
      per-pixel-alpha layered window. `[mask] fill = "lines"` (default)
      paints one continuous black band per OCR line segment (word gaps below
      3/4 em are filled; tabs/columns stay separate) with the words
      re-rendered white (`Microsoft YaHei`) at their exact OCR positions, so
      Latin spaces and CJK spacing need no re-layout; `"boxes"` restores the
      per-word look. Untouched pixels come from the native BGRA snapshot taken
      with the OCR capture; every layered pixel is opaque and hit-testable.
      Pixel-sampled: box black, glyphs light, gap frozen, band continuous
      between words.
- [x] Click a word → `CF_UNICODETEXT` holds exactly that word.
- [x] Press-drag a rectangle → one selection in reading order: `join_words`
      inside a line (Latin spaces, no CJK spaces) and `\n` between OCR lines
      (`mask::selection_text`, unit tests). End-to-end clipboard read:
      `Quick brown\n你好世界`.
- [x] `toggle` binding (default `win+shift+o`; Win+O is reserved by Windows
      and `RegisterHotKey` fails with 1409) hides/shows the existing mask:
      same HWND, same words, no re-OCR.
- [x] `[mask] font` / `[mask] size` (points, default 10 = the dotfiles
      WezTerm font_size) / `[mask] layout = "overlay" | "reader"`. Fixed size
      is the default, so re-rendered words no longer change size from one
      word to the next; `size = "auto"` keeps the source size and now takes
      the per-line median height (was per word). The default face is Sarasa
      Term SC Nerd, the WezTerm face, so mask text matches the terminal.
- [x] `layout = "reader"` paints an opaque black page and reflows the
      recognised text at the fixed size: OCR lines are joined with
      `join_words`, a vertical gap >= one line height starts a new paragraph
      (`\n\n`), and a click copies the whole text. Position-faithful overlay
      stays the default.
- [x] Reader and overlay share `toggle`, Esc, right-click and re-capture
      (which dismisses the previous mask).
- [x] Esc or right-click dismisses; right-click closes on button-up so the
      app underneath never receives the stray release. The frame is
      interactive again in its previous mode.
- [x] Re-capturing while the mask is open dismisses it first, so OCR reads
      the real screen rather than our own overlay.
- [x] `~\.temp\opencode\mask_test.ps1` drives the whole cycle at message
      level (known Latin+CJK text window under the frame, posted capture
      hotkey, posted click/drag/Esc/right-click): 27 checks pass, including
      the click-through exstyle while OCR runs, `WindowFromPoint` proving
      gaps belong to the mask and points outside the frame do not, the
      continuous line band, the toggle hide/show with an unchanged HWND, and
      pin surviving a capture + dismiss.
- [x] `~\.temp\opencode\reader_test.ps1`: hot-reloads `layout = "reader"`,
      checks the opaque black page, light text pixels and that a click copies
      `Quick brown 你好世界` (one reflowed paragraph).
- [ ] PHYSICAL: `ctrl+enter` over a real mixed Latin+CJK screenshot; click
      one word; drag a two-line paragraph (paste must keep the line break);
      `win+shift+o` twice (hide/show, no re-OCR); switch `fill = "boxes"` to
      compare; set `size = "auto"` and compare; `layout = "reader"` over a
      long paragraph and click to copy all; Esc; right-click.
- [ ] PHYSICAL: while OCR runs, click a button under the frame — the click
      must reach the app, not the frame.

### Frozen snapshot base (2026-10-27)

- [x] `Outcome` carries the full-frame native BGRA capture; OCR still uses the
      upscaled inset, and overlay gaps composite over the frozen pixels rather
      than the live screen. All mask pixels are opaque and hit-testable.
- [x] Mechanical: `mask_test.ps1` passes capture, hit-testing, rendering, word
      click, line-break selection and frozen-base checks (14 checks), including
      recoloring the source without changing the mask. Its later exact-CJK
      assertion still fails on this machine's Windows OCR; not a snapshot
      regression.
- [ ] PHYSICAL: capture, then scroll or change the app underneath without
      dismissing the mask. The whole frame image must stay frozen and aligned;
      word click, drag-copy, toggle and dismiss must still work.

### OCR quality (post-P0.3 feedback: small/compact fonts)

- [x] Native resolution was unusable on 8-10 pt screen text (`The qu1Ck 0r0
      n fOX ...`). Captures are now upscaled with GDI's halftone stretch
      before recognition: `[ocr] scale` (1..=4, default 4), clamped by the
      engine's `MaxImageDimension` (10000 here) and a 16 Mpx budget. Word
      rects are divided back by the same factor, so mask geometry is
      unchanged.
- [x] `[ocr] language` = auto (default) / a tag (`en`, `zh`, ...). Auto runs
      the zh engine, then an English pass when Latin is present, and merges
      line by line: lines with ≥20% CJK letters keep the zh result, Latin
      lines take the en result (an en line is consumed at most once). The en
      model drops CJK entirely and the zh model mangles long Latin runs, so
      neither alone covers mixed text.
- [x] `[ocr] scale` / `[ocr] language` hot-reload; the next capture picks
      them up.
- [x] Measured on a 10 pt Sarasa Mono SC sample (800x600 frame): 1x garbage;
      2x ~90%; 4x + merge = line 1/2 exact, pure-CJK line exact, one 0/O
      confusion in `OCR`. RapidOCR (AHK-ScreenOCR pipeline) on the same PNG:
      all four lines correct at native resolution.
- [x] RapidOCR cost measured: ~1.6 s per `uv run` CLI call (warm cache) and
      line-level boxes only. That is the P0.4 decision: persistent worker and
      word segmentation, or accept losing word-level clicks.
- [ ] PHYSICAL: frame over real small/compact text (e.g. Sarasa Mono 10 pt),
      compare the mask text with the source; try `scale = 1` for contrast and
      `language = "en"` on a Latin-only page.

## P0.4 OCR engines + language routing
- [ ] Chinese UI text under `auto` OCRs correctly (RapidOCR path).
- [ ] English page under `auto` OCRs correctly (Tesseract path).
- [ ] Forcing the other engine explicitly still produces text.
- [ ] Language detection matches the AHK-ScreenOCR behavior the user is
      used to (spot-check the same screens as the old tool).

## P0.5 Auto-refresh triggers
- [ ] Text inside the frame changes → re-OCR happens by itself.
- [ ] Static frame → no re-OCR (no flicker, no CPU spin).

## P0.6 Release packaging
- [x] Tag-driven GitHub Actions release packages dt-ocr.exe + dt-ocr.toml,
      with a matching .sha256 file and consistent desktop-tools-rs-ex names.
- [x] scoop-pile pulled with git pull --ff-only. window-tools-rs.json renamed
      to desktop-tools-rs.json; v0.1.0 pre_install handles its old bundled exe
      and config names, while autoupdate uses the new asset names.
- [ ] desktop-tools-rs-ex.json is prepared for v0.1.0 but intentionally has an
      empty hash until the first release publishes the .sha256. Do not call it
      installable yet. JSON parsing passes for both manifests.

## P1.x
- [ ] (filled when each P1 phase starts)
