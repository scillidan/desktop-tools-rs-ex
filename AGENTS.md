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

## Layout (planned)

- `dt-common` — copied keys/layout/config/tray primitives, shared by all tools
- `dt-ocr` — OCR frame window, overlay mask, drag-select OCR, engines
- `dt-rec` (future) — recording (wcap model)
- `dt-scale` (future) — window scaling (Magpie model)
