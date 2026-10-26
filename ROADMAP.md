# ROADMAP.md — desktop-tools-rs-ex

One phase per prompt round; the user physically verifies the CHECKLIST items
before the next phase starts. Phases are ordered so each is independently
useful.

## P0 — Text Grab core (OCR frame + overlay)

### P0.1 Scaffold + dt-common
- Cargo workspace: `dt-common` (shared keys/layout/config/tray, copied from
  desktop-tools-rs with provenance comments), `dt-ocr`.
- TOML config next to exe, seeded template at repo root; logging; anyhow
  convention. `rebuild_ex.ps1` (kill → build → sync config → CIM launch →
  probe) in `C:\Users\User\.temp\opencode\`.
- Done when: empty dt-ocr window launches via one script, config found.

### P0.2 OCR frame window
- Borderless, always-on-top frame marking a fixed OCR region. Create by
  drawing a rect (mouse drag) or sizing a default frame.
- `Ctrl+Alt+S` resizes the frame by a SizeSpec — `1600` (width px),
  `100%` (percent of work area), `1/2` (fraction of work area); parser
  reuses desktop-tools-rs `layout::SizeSpec`.
- `Ctrl+T` locks the frame (pin = stays put, non-interactive except hotkeys;
  lock-only, not a toggle). `Ctrl+Alt+M`/`Ctrl+Alt+S` unlock and select
  Move/resize globally. Frame may be dragged/moved when unpinned.
- `Ctrl+Enter` (or configured key) captures the region under the frame to
  clipboard/file.
- Reference: Text-Grab's "Grab Frame".
- Done when: frame can be drawn, resized by all three SizeSpec forms,
  pinned, and its region captured.

### P0.3 Overlay mask
- During OCR: the frame region is captured in the background and the frame
  stays **click-through** (nothing under it is blocked).
- After OCR: render a transparent-background overlay inside the frame where
  each OCR word is drawn as a text element with an opaque black (or themed)
  backing box covering the original text underneath.
  - Click a word → copy that word to clipboard.
  - Press-drag across words → batch-select; release → join selected words
    into one paragraph on the clipboard: insert a space between two Latin
    words, no space for CJK (pure function, heavily unit-tested).
  - Right-click or Esc → dismiss the overlay (frame returns to idle).
  - While the overlay is up it owns the clicks inside the frame region;
    everything outside stays reachable.
- Done when: full click/copy/drag-paragraph/exit cycle works over a real
  screenshot with mixed Latin+CJK text.

### P0.4 OCR engines + language routing
- RapidOCR (ONNX runtime) as primary engine for CJK and mixed text.
- Tesseract as secondary for Latin-script languages (e.g. en).
- `ocr.engine = "auto" | "rapid" | "tesseract"`. Open question from the
  user — interaction design for multi-engine; proposal: `auto` routes by a
  language table (per-target or detected-script), explicit keys force an
  engine. Port the language-detection interaction from
  `C:\Users\User\Share\projs\AHK-ScreenOCR` (AHK v1).
- Native deps (onnxruntime DLLs, tessdata) provisioned via Scoop; build
  script documents what's required. Verify with context7 before pinning
  crate versions.
- Done when: a frame over a Chinese UI and over an English page both OCR
  correctly under `auto`, and forcing the other engine still works.

### P0.5 Auto-refresh triggers
- Manual key (existing) plus automatic re-OCR: fixed interval and/or
  on-change (frame region hash/pixel-diff above threshold) — the
  text/graphic-game use case.
- `ocr.auto = false | { every_ms, on_change }`, hot-reloadable.
- Done when: changing text inside the frame re-OCRs by itself, a static
  frame does not.

## P1 — Parity with existing AHK tools + more

### P1.1 Drag-select screen OCR
- Port `AHK-ScreenOCR`: draw a rect anywhere → OCR → result (copy/show).
  Shares engines + paragraph-join logic with dt-ocr.

### P1.2 FlameshotWindow
- Port `AHK-FlameshotWindow`: detect/highlight the window under the cursor,
  then invoke `flameshot gui` preset to the window's rect.

### P1.3 Recording (wcap model)
- v1: locate `ffmpeg.exe` (Scoop) + `wcap.exe` if distributable, wrap with
  config (quality, fps, output). v2: own capture (Desktop Duplication API)
  piping raw frames to ffmpeg. Decide at P1.3 start with context7 + wcap
  source reading.

### P1.4 Window scaling (Magpie model)
- Research spike first (Magpie = D3D11 capture + shaders). v1 may be a
  simple integer-ratio fullscreen stretch; lecture/presentation scenario is
  the target. Decide approach at P1.4 start.

## P2 — Consolidation
- Extract dt-common back into the keytools-rs/kit plan (memory: kit is the
  shared tray/hotkey/config library); revisit after desktop-tools-rs's
  folder rename lands.
