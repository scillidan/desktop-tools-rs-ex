# NEXT.md — continuation prompt for a new machine

Written 2026-10-27. Repo: `desktop-tools-rs-ex` (crate `dt-ocr`). This file is
the handoff for a fresh agent session on a new PC. The repo is self-contained;
only the PowerShell verification harnesses used to live outside it (see §6.3).

---

## Paste this into the new session

> You are continuing `desktop-tools-rs-ex`, the Rust workspace for desktop
> tools (OCR sibling of `desktop-tools-rs`). Work in this repo. Start by
> reading `AGENTS.md` (conventions and machine rules), `ROADMAP.md`,
> `PLAN.md`, `CHECKLIST.md`, then this file.
>
> Environment: Windows, Rust stable (1.99.0 on the old machine), PowerShell
> 5.1. The app is `dt-ocr`: a persistent always-on-top OCR frame.
>
> Current state: P0.1–P0.3 are done, including the overlay mask, OCR quality
> work and the mask reader mode (details below). Before changing anything:
> 1. `cargo test` (expect 67 passing) and `cargo clippy --all-targets`.
> 2. Rebuild/relaunch with the script from §6.1, then run the freshness check
>    from §6.2.
> 3. Recreate the harnesses in §6.3 if `%USERPROFILE%\.temp\opencode\*.ps1`
>    did not come along.
> 4. Ask me which item in §7 to take next. Do not start P0.4 (RapidOCR)
>    without an explicit decision — it changes the interaction model.
>
> Handoff rules: rebuild LAST, verify the exe is newer than every source file,
> update `CHECKLIST.md`, and give me a short physical-verification list.

---

## 1. What the app does today

- One global-hotkey-driven frame (`dt-ocr.frame`) marks a screen region.
- `ctrl+enter` captures the pixels (clipboard: CF_DIB + CF_BITMAP) and runs
  Windows OCR on a worker thread; the frame is click-through while it runs.
- The OCR result is shown in a topmost layered window (`dt-ocr.mask`):
  - **overlay** (default): black backing + white re-rendered text at the OCR
    positions. `fill = "lines"` connects word gaps into line bands;
    `fill = "boxes"` is per-word. Click a word = copy it; drag = copy the
    selection (Latin spaces, no CJK spaces, `\n` between OCR lines).
  - **reader**: opaque black page, the text reflowed at the fixed size,
    paragraphs separated by a blank line; a click copies everything.
- `win+shift+o` (`toggle`) hides/shows the existing mask without re-OCR.
- Esc / right-click dismiss. Re-capturing dismisses the old mask first.
- Frame modes/hotkeys: draw `ctrl+alt+d`, resize `ctrl+alt+s`, pin `ctrl+t`,
  capture `ctrl+enter`, move `ctrl+alt+m` (see `AGENTS.md` for the state
  machine; `interaction.rs` is the tested five-state model).

## 2. Code map (`dt-ocr/src`)

| file | owns |
|---|---|
| `main.rs` | config load, hotkey registration, message loop, dispatch, log |
| `frame.rs` | frame window, modes, SizeSpec input/tune, status strip, `WM_OCR_DONE` |
| `interaction.rs` | pure state machine (Move/Pinned/Input/Drag/Tune) |
| `overlay.rs` | drag-a-rect overlay |
| `capture.rs` | GDI BitBlt, clipboard image, `copy_text` |
| `hotkeys.rs` | `RegisterHotKey`, ids = `Action::ALL` order |
| `ocr.rs` | capture → 4× halftone upscale → Windows OCR → zh/en line merge → `Word { text, rect, line }` |
| `mask.rs` | layered mask, overlay/reader rendering, toggle, selection and copy |
| `text.rs` | `join_words`, `is_cjk`, `is_cjk_letter` |

## 3. Config defaults (root `dt-ocr.toml`, copied next to the exe on rebuild)

```toml
[frame]
width = 800
height = 600
show_hint = true

[ocr]
scale = 4              # 1..=4, GDI halftone upscale before OCR
language = "auto"      # auto = zh pass + en pass merged per line

[mask]
fill = "lines"         # lines | boxes (overlay only)
layout = "overlay"     # overlay | reader
font = "Sarasa Term SC Nerd"  # the dotfiles WezTerm face
size = 10              # points; "auto" = per-line source height

[bindings]
draw = "ctrl+alt+d"
resize = "ctrl+alt+s"
pin = "ctrl+t"
capture = "ctrl+enter"
move = "ctrl+alt+m"
toggle = "win+shift+o"
```

Hot-reload: the app polls the exe-side `dt-ocr.toml` every second. Runtime files
live next to the exe: `target\release\dt-ocr.toml` and `dt-ocr.log`.

## 4. New-machine setup

1. Rust stable; `cargo build --release`. No other runtime dependency (there is
   no Python/uv integration yet — RapidOCR is not wired in).
2. Windows OCR language packs: Settings → Time & language → Language →
   English (US) and Chinese (Simplified), each with **Optical character
   recognition**. Without zh, `auto` falls back to en and drops CJK.
3. Fonts: `Sarasa Term SC Nerd` (mask default) and `Microsoft YaHei` (reader/
   fallback). If the Sarasa face is missing, GDI substitutes; set
   `[mask] font = "Consolas"` to be explicit.
4. The old machine's repo path was `C:\Users\User\Share\projs\desktop-tools-rs-ex`;
   adjust paths in scripts if the user name differs.

## 5. Hard-won constraints (do not rediscover these)

1. **Never run bare `find`** — it is `find.exe` on Windows. Use `rg`/`fff`.
2. Debug builds are console apps; clicking the console suspends the app.
   Test release builds only.
3. The opencode shell wrapper kills children on exit; launch daemons with
   `Invoke-CimMethod Win32_Process Create`, not `Start-Process`.
4. `RegisterHotKey` on this machine: **Win+O is reserved (error 1409)**; the
   toggle is `win+shift+o`. Register against the frame HWND (attribution).
5. Layered windows: alpha 0 pixels are click-through even with per-pixel
   alpha. The mask uses the capture's native BGRA snapshot as its base and
   makes every pixel opaque, so the region stays hit-testable and frozen.
6. Windows OCR is unusable at native resolution on 8–10 pt text
   (`The qu1Ck 0r0 n fOX`). It runs at 4× (GDI halftone), clamped by
   `OcrEngine.MaxImageDimension` (10000 here) and a 16 Mpx budget.
7. The zh model mangles long Latin runs; the en model drops CJK. `auto` runs
   both and merges per line: lines with ≥20 % CJK letters keep zh, Latin lines
   take en (an en line is consumed at most once).
8. PowerShell 5.1 reads BOM-less `.ps1` as ANSI/GBK: build CJK strings from
   code points or write UTF-8 with BOM. Writing TOML from PS needs BOM-less
   UTF-8 (`UTF8Encoding($false)`).
9. Injected keystrokes cannot trigger registered hotkeys; drive message-level
   behavior with `PostMessage WM_HOTKEY` (ids 1..6 = draw, resize, pin,
   capture, move, toggle). Real border drags need `mouse_event`; posted
   messages occasionally flake (rerun).
10. `INVALIDATE` alone does not repaint under hit-test traffic; the drag path
    calls `UpdateWindow` directly.
11. `SystemParametersInfoForDpi` fails with E_INVALIDARG here; DPI uses
    `GetDpiForWindow`. Mask `size` is points: `em_px = pt * dpi / 72`.
12. Rebuild LAST: `dt-ocr.exe` mtime must be newer than every `src\*.rs`
    (the old machine shipped a stale exe once).

## 6. Verification

### 6.1 Rebuild + relaunch

Save as `tools/rebuild.ps1` (repo-relative). It kills, builds, syncs the config
template next to the exe, launches via CIM, checks the process and probes that
`ctrl+alt+s` is held by someone else (1409 = dt-ocr owns it).

```powershell
param([switch]$Clean)
$repo = Split-Path -Parent $PSScriptRoot
$exe  = Join-Path $repo 'target\release\dt-ocr.exe'
taskkill /F /IM dt-ocr.exe 2>$null | Out-Null
Start-Sleep -Milliseconds 800
Push-Location $repo
if ($Clean) { cargo clean }
cargo build --release
if ($LASTEXITCODE -ne 0) { Pop-Location; Write-Host 'BUILD FAILED'; exit 1 }
Pop-Location
Copy-Item (Join-Path $repo 'dt-ocr.toml') (Join-Path $repo 'target\release\dt-ocr.toml') -Force
$r = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = "`"$exe`"" }
Write-Host "start ret=$($r.ReturnValue) pid=$($r.ProcessId)"
Start-Sleep -Seconds 1
$alive = Get-CimInstance -ClassName Win32_Process -Filter "Name='dt-ocr.exe'"
if (-not $alive) { 'NOT RUNNING (BAD)'; exit 1 }
"alive: pid=$($alive.ProcessId)"
$sig = @'
[DllImport("user32.dll", SetLastError = true)]
public static extern bool RegisterHotKey(System.IntPtr hWnd, int id, uint fsModifiers, uint vk);
[DllImport("user32.dll", SetLastError = true)]
public static extern bool UnregisterHotKey(System.IntPtr hWnd, int id);
'@
$api = Add-Type -MemberDefinition $sig -Name User32Probe -Namespace DtOcr -PassThru
if ($api::RegisterHotKey([IntPtr]::Zero, 99, 0x0003, 0x53)) {
    $api::UnregisterHotKey([IntPtr]::Zero, 99) | Out-Null
    Write-Warning "ctrl+alt+s still FREE — dt-ocr hotkeys not registered?"
    exit 1
}
"hotkey probe: ctrl+alt+s held (pid $($alive.ProcessId))"
```

### 6.2 Freshness check (`tools/check_fresh.ps1`)

```powershell
$repo = Split-Path -Parent $PSScriptRoot
$exe = Get-Item (Join-Path $repo 'target\release\dt-ocr.exe')
$src = Get-ChildItem -Path (Join-Path $repo 'dt-ocr\src'), (Join-Path $repo 'dt-common\src') -Recurse -File |
  Sort-Object LastWriteTime -Descending | Select-Object -First 1
if ($exe.LastWriteTime -gt $src.LastWriteTime) { 'FRESH: exe newer than all sources' } else { 'STALE'; exit 1 }
Get-Content -LiteralPath (Join-Path $repo 'target\release\dt-ocr.log') -Tail 3 -Encoding UTF8
```

### 6.3 Harnesses (were in `%USERPROFILE%\.temp\opencode\` on the old machine)

Copy them over if they survived; otherwise recreate from these specs. All
drive the app at message level against a known WinForms text window placed
under the frame at (911,420) 800×600 (the default frame position).

**`mask_test.ps1`** — 27 checks, overlay:
- show a form with `"Quick brown\r\n你好世界"` (36 pt, topmost) under the frame;
- post capture (WM_HOTKEY 4); poll the frame exstyle for `WS_EX_TRANSPARENT`
  while OCR runs (busy loop, ~150 ms window);
- wait for `dt-ocr.mask`; check `WindowFromPoint` in a gap and outside the
  frame; screenshot: word box black, glyph pixels light, gap frozen, line band
  continuous past the first word; changing the source must not change the mask;
- posted click → clipboard = first word; posted drag across both lines →
  `Quick brown\n你好世界`;
- WM_HOTKEY 6 twice → mask hidden/shown with the same HWND (no re-OCR);
- Esc destroys; right-click on a fresh mask destroys; pin survives capture.

**`reader_test.ps1`** — 5 checks: rewrite the exe-side `[mask] layout =
"reader"` (hot-reload), capture, check the black page, light text pixels, and
that a click copies `Quick brown 你好世界`; restore `overlay`.

**`ocr_small_test.ps1 [-Scale N] [-Language L]`** — rewrite `[ocr] scale` /
`language` in the exe-side TOML (hot-reload), show a 10 pt Sarasa Mono SC
Latin+CJK sample, capture, print the `ocr:` preview lines from the log.

Log lines to expect: `ocr: capture WxH (scale 4, max 10000px)`,
`ocr: zh-Hans-CN -> N lines, M words`, `ocr: en-US -> ...`,
`ocr: merged ...`, `ocr: text "..."`, `mask: overlay:lines, N words, font
Sarasa Term SC Nerd+13px ...`.

## 7. Open work (ask before starting)

1. **Frozen snapshot base** (implemented 2026-10-27): `Outcome` carries the
   native full-frame BGRA capture; OCR uses an upscale of its inset, and the
   mask composites over the frozen pixels. Physical verification pending.
2. **Token-range selection** (not implemented): replace rectangle intersection
   with anchor→head token ranges; shift/ctrl extend, double-click a paragraph,
   ctrl+A; keep the current per-line copy rules.
3. **Yellow→cyan feedback** (not implemented): press/selection = yellow
   outline, after a successful copy flash cyan 2× (120/80/120 ms).
4. **P0.4 engine**: RapidOCR (the AHK-ScreenOCR pipeline) measured 4/4 lines
   correct at native resolution, ~1.6 s per `uv run` CLI call, line boxes only.
   Decision needed: persistent worker + word splitting, or keep Windows OCR.
5. **Reader limits**: no scrolling (text clips), click copies everything, no
   per-word selection; paragraph split = vertical gap ≥ one line height.

## 8. Handoff checklist for every change

- `cargo fmt`, `cargo test` (currently 67 passing, 1 ignored), `cargo clippy --all-targets`.
- Rebuild/relaunch with §6.1 (release, CIM launch, probe).
- §6.2 freshness check: exe newer than all sources.
- Update `CHECKLIST.md` (mechanical + physical items), `AGENTS.md` status,
  `PLAN.md` decision log; physical verification list in the reply.
