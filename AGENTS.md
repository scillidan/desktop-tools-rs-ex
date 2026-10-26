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
  via `C:\Users\User\.temp\opencode\rebuild_ex.ps1` (mirror of
  rebuild_daemon.ps1: taskkill → build → sync config next to exe → CIM
  launch → probe). The user never builds or restarts anything manually and
  verifies only the on-screen effect.

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

## Agent workflow

1. cq first (query before acting; propose non-obvious findings).
2. One ROADMAP phase per unit of work; the user reviews CHECKLIST results
   before the next phase starts.
3. Rebuild + relaunch automatically after every change; hand the user a
   short physical-verification list, never a build step.

## Layout

- `dt-common` — keys/layout/config primitives copied from desktop-tools-rs
  (repo folder `window-tools-rs`) with provenance comments; shared by all tools
- `dt-ocr` — OCR frame window, overlay mask, drag-select OCR, engines
- `dt-rec` (future) — recording (wcap model)
- `dt-scale` (future) — window scaling (Magpie model)

P0.1 status: workspace + dt-common (`keys`, `layout`, `config` exe-relative
helpers) + dt-ocr (empty window, `dt-ocr.toml` next to exe, `dt-ocr.log`).
kbdhook/tray pipeline intentionally NOT copied yet — they land with the P0.2
hotkeys/tray.

P0.2 status: dt-ocr frame window complete. `interaction.rs` is the tested,
explicit five-state model: Move (cyan), Pinned (blue), and yellow resize
substates Input / Drag / Tune. `frame.rs` only applies those transitions;
there are no independent pinned/spec booleans. ctrl+t is lock-only and
idempotent; ctrl+alt+m/s are global absolute mode commands that unlock Pin
and focus the frame. From outside resize, the first ctrl+alt+s lands on
Tune(width), later ones on Drag; inside resize it resets Input. Esc:
Input→Drag, Drag/Tune→Move; any defocus exits resize to Move. Capture never
changes mode. Move drags by its interior/arrows/hjkl; a border press from
Move/Input/Tune enters Drag and the same captured client-mouse drag resizes
(no WS_THICKFRAME or NC sizing dependence). Drag arrows move their matching
edge (Shift inward, Ctrl ±5); `i`→Input, `w`/`h`→Tune. Tune digits/arrows
keep the 250 ms geometry debounce. The 2 px border has exactly one mode
color; release forces UpdateWindow so readouts cannot remain stale.
The status strip is display-only (below/above/inside fallback), Consolas or
`frame.font`, inverse-video SizeSpec input with a block caret; readout tune
is keyboard-entered, not clicked. SizeSpec uses per-axis DimSpec against the
current monitor work area. `overlay.rs` = red draw band; `capture.rs` = GDI
→ clipboard CF_DIB + CF_BITMAP; `hotkeys.rs` = RegisterHotKey with the
ported vk map. `~\.temp\opencode\frame_state_test.ps1` verifies background
mode commands, Pin/Esc/defocus and real border geometry (mouse_event drag;
single-event SendInput did not deliver the button-down here). Every handoff
must rebuild LAST and check exe mtime > all src mtimes.
