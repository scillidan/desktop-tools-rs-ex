# EXPERIMENT.md — using Neovim as an external editor

Status: explored in `dt-ocr` as `Ctrl+E` in the `box+outside` result editor, then
removed from the product code. This file keeps the reusable business logic so a
future tool (or a future dt-ocr phase) can implement it deliberately instead of
re-discovering it.

The question is not "how do I start nvim". It is "what must an application own
around an external editor so the user's buffer is never lost, duplicated, or
silently stale".

## 1. Choose the integration level first

| Level | What it is | Cost | Use when |
|---|---|---|---|
| A. External handoff | Write the text to a temp file, launch the user's editor in its own terminal window, read the file back on exit | Low; one process + one file + window placement | The app has a small text surface (OCR result, commit message, config snippet) and the user already lives in Neovim |
| B. Embedded Neovim UI | `nvim --embed`, RPC `nvim_ui_attach`, render the grid yourself | Very high: IME, fonts, grid, scrolling, clipboard, selection, input method, window composition | The editor is the core product (Firenvim, vscode-neovim) |
| C. Reimplement Vim keys | A modal layer on the existing native control | Medium, but never the real editor: no `init.lua`, plugins, LSP, macros | The app only needs `hjkl`, `dd`, `/` and is not an editing environment |

This note covers **Level A only**. Level B is a separate project, not a feature.

## 2. The external-handoff contract

An application that offers "edit in Neovim" must own all of these:

1. **Scope** — decide what is handed over: the whole buffer, the current
   selection, or a read-only copy. Record the choice; do not infer it from the
   focused control at exit time.
2. **Materialisation** — write a real file so Neovim can use filetype plugins,
   `:w`, modelines, LSP and swap files. A pipe or a fake buffer loses too much.
3. **Process launch** — start a terminal + editor with an explicit argv, never a
   shell string. Keep the child handle so the app can know when it exited.
4. **Window placement** — place the editor window once, next to the invoking
   surface, inside the monitor work area. Topmost or not is a product decision.
5. **Source state** — disable or freeze the native editor while the external one
   runs. Two editors writing the same text is data loss.
6. **Completion** — wait for the editor process to exit, then read the file back.
   Handle cancel, crash, missing executable and app shutdown.
7. **Apply + cleanup** — install the new text, reset undo/history to the new
   baseline, delete the temp file and any session state.

## 3. Buffer materialisation

- **Encoding**: UTF-8, always. Normalise the app's internal text to LF on the
  way out; convert back to the platform control's convention on the way in
  (Win32 multiline `EDIT` needs CRLF).
- **Path**: `%TEMP%/<app>-<pid>-<session>.txt` (or the platform temp dir).
  Unique per session; never a fixed filename. A stable extension helps
  filetype detection; `.txt` is the safe default.
- **Metadata**: keep the original text and a hash/length in memory. The hash is
  the "unchanged" check and the conflict check.
- **Permissions**: the temp file is user-private. Do not put user text in a
  world-readable location; do not pass it through a shell.
- **Lifecycle**: create → write → flush → launch → wait → read → delete. Delete
  on every path, including cancel and failure, unless the user asked to keep it.
- **Neovim details**: Neovim may create `.swp`, `.swo`, `.un~` and backup files
  next to the buffer. Use a dedicated temp directory per session if those files
  must not leak; otherwise delete the whole session directory, not just the
  `.txt`.
- **Limits**: cap the size (e.g. 1–10 MiB) and refuse rather than hang the UI.
  Binary content is not an editor buffer.

## 4. Process launch

Minimum viable command shape:

```text
wezterm start --always-new-process --class <unique-class> \
    --position <x>,<y> --cwd <temp-dir> -- nvim <file>
```

Rules:

- **Argv, not shell**: no `cmd /c`, no string concatenation, no quoting bugs.
- **Fresh process**: for terminal hosts that reuse a running mux, force a new
  GUI instance when the placement matters (`--always-new-process` in WezTerm).
- **Unique window class**: lets the app find *its* window, never the user's
  existing terminal (`--class` in WezTerm).
- **Hidden console**: Windows child processes need `CREATE_NO_WINDOW` if the
  launcher is not already a GUI process.
- **Environment**: inherit the user's environment so their real config, PATH,
  LSP and plugins work. Do not set `NVIM_APPNAME` or `--clean` unless the
  product intentionally wants an isolated editor.
- **Exit code**: a non-zero exit is not automatically an error — the user may
  have `:cq`, or the terminal may return the shell status. Read the file; the
  file is the source of truth.
- **Single session**: one handoff at a time per surface. A second trigger should
  focus the running session or be ignored, not spawn a second editor.

## 5. Window placement

Generic algorithm, independent of Win32:

```text
owner   = rect of the invoking surface (frame / panel / dialog)
work    = usable monitor area (exclude taskbar)
width   = clamp(owner.width, min_width, work.width)
height  = clamp(work.height / 3, min_height, max_height, work.height)
side    = preferred side (usually below for landscape, right for portrait)
if room on preferred side:
    place against that side
else:
    float against the opposite work-area edge
```

Decisions to make explicitly:

- **One-shot vs follow**: place once at launch (simple, predictable) or follow
  the owner while it moves (needs a timer or owner `WM_MOVE`/`WM_SIZE` hook).
  Following a moving window is visually nice but can fight the user's own drag.
- **Topmost**: making the editor `HWND_TOPMOST` keeps it above the invoking app;
  it also keeps it above everything else, which may be wrong when the user
  switches tasks. Make it a policy, not an accident.
- **DPI**: use physical pixels and the owner monitor's work area; never assume
  96 DPI or the primary monitor.
- **Minimum size**: respect a usable terminal size (columns/rows), not just
  pixels.
- **Focus**: the terminal should take focus on launch; return focus to the
  source control when it exits.

## 6. State machine

```text
Idle
  └─ trigger ─→ Preparing   (write temp file, snapshot original)
                  └─ launch ok ─→ Running   (editor process alive)
                  │                 ├─ user exits ─→ Reading
                  │                 └─ timeout/crash ─→ Aborted
                  └─ launch fail ─→ Aborted (keep original text)
Reading
  ├─ file readable, changed ─→ Applied (install text, reset undo baseline)
  ├─ file readable, same ─→ Applied (no-op)
  └─ file missing/unreadable ─→ Aborted (keep original text)
Applied / Aborted
  └─ cleanup ─→ Idle
```

Every transition needs a log line: session id, temp path, process id, exit
reason, bytes/chars read back, and whether the text was applied. Without those
lines the feature is impossible to support from a user report.

## 7. Read-back rules

- Read the file **after** the editor process exits, not when the window closes
  (terminal hosts may keep the process alive after the window is hidden).
- Normalise encoding to UTF-8; if decoding fails, keep the original text and
  report it.
- Convert LF → control-specific newlines (CRLF for Win32 `EDIT`).
- Compare with the original. Identical content is a successful no-op.
- If the file is missing, keep the original. Never replace a buffer with empty
  text because a path disappeared.
- Reset the native undo/redo history to the new baseline; the external edit is a
  new document state, not an undoable step inside the old history.
- Put the caret at a predictable place (start or the previous caret if tracked).

## 8. Failure matrix

| Failure | User-visible result | App behaviour |
|---|---|---|
| `nvim` not on PATH | Nothing opens | Keep text, log, show a one-line error |
| Terminal host missing | Nothing opens | Same |
| Temp file cannot be created | Nothing opens | Keep text, log |
| Editor exits non-zero | Editor closes | Read the file; apply if valid |
| User kills the window | Session ends | Read the file; apply if valid |
| File unreadable after exit | Editor closes | Keep original text |
| App exits while editor runs | Editor may stay open | Best effort: leave the file, log; a crash-cleanup pass can remove stale sessions |
| Second trigger while running | Existing editor focuses | Do not spawn a second session |
| Source control destroyed while running | Editor may stay open | Drop the completion message safely; never post to a dead window |

## 9. Security and data safety

- No shell interpolation of user text or paths.
- Temp files are user-private and deleted after use.
- The app never overwrites the buffer with a partial read.
- The app never treats "editor exited" as "user saved"; only file content is
  authoritative.
- If the app has an autosave or background refresh, it must be paused while the
  external editor owns the buffer.

## 10. Minimal API shape

```rust
struct OpenRequest {
    owner: WindowId,
    text: String,
    title_hint: String,
    read_only: bool,
}

enum SessionState {
    Preparing,
    Running { pid: u32 },
    Reading,
    Applied { changed: bool },
    Aborted { reason: String },
}

trait ExternalEditor {
    fn open(&self, req: OpenRequest) -> Result<SessionId>;
    fn poll(&self, id: SessionId) -> SessionState;
    fn cancel(&self, id: SessionId);
}
```

The caller only needs three things: a way to start, a way to be told it finished,
and the guarantee that the original text survives every failure.

## 11. Test checklist

Mechanical, without synthetic keyboard input:

1. Launch creates a temp file with exactly the original text (bytes).
2. The editor window class is the app's unique class, not the user's terminal.
3. The window rect is inside the work area, on the preferred side, with the
   minimum size respected.
4. The source control is disabled while the session runs.
5. Closing the editor re-enables the source control.
6. An unchanged file leaves the text identical and resets undo.
7. A modified file is applied with the right newline convention.
8. A missing/unreadable file keeps the original text.
9. A missing editor executable keeps the original text and logs a warning.
10. Two triggers in a row create at most one editor window.
11. No temp file or session directory remains after success, cancel or failure.
12. The app can be closed while the editor runs without a crash or a message to
    a dead window.

Manual checks: real `:wq`, real `:q!`, editing with the user's `init.lua`, IME
input, and a second monitor with a non-primary DPI.

## 12. Neovim-specific notes

- Pass the file as a normal argument: `nvim <file>`. Do not use `-c` for content.
- A dedicated `--cmd`/`-u` is only for an intentionally isolated editor; the
  point of Level A is the user's real config.
- `:wq` writes the file; `:q!` leaves it unchanged; `:cq` exits non-zero. All
  three are normal outcomes.
- Swap files can be left behind by a kill. Delete the session directory, not
  only the buffer.
- Level B (embedded) needs `nvim --embed` plus `nvim_ui_attach` and a grid
  renderer. Do not mix it with Level A: they have different input, IME and
  clipboard models.

## 13. Why it was removed from dt-ocr

The experiment proved the mechanic: WezTerm opened with a unique class, docked
below the frame, stayed topmost, disabled the native editor, and handed an
unchanged buffer back. It was still a second product surface inside an OCR
tool: a terminal window, a second editing model, and a process lifetime that
does not belong to the OCR result loop.

The product keeps the native `box+outside` editor. A future tool that is an
editor first can implement this note intentionally.
