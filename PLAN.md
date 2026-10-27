# PLAN.md — how to drive desktop-tools-rs-ex with an agent

The project is executed in ROADMAP phases, one phase per prompt round. This
file is the operator's manual: what to paste, what the agent must do, and
how decisions get made.

## Golden rules

1. **One phase at a time.** An agent must not start phase N+1 until the
   user has confirmed phase N's CHECKLIST items on the real machine.
2. **The user never builds or restarts.** After any change the agent runs
   `rebuild_ex.ps1` itself and only then hands over a verification list.
3. **cq before acting** (per the global AGENTS.md), context7 for library
   docs, tdd-style unit tests for pure logic (SizeSpec forms, the
   Latin/CJK paragraph join, region geometry, change detection).
4. **Reference desktop-tools-rs by convention, not dependency.** Copy code
   into `dt-common` with a provenance comment; do not add it as a cargo
   dependency (its folder will be renamed; kit-extraction is a later plan).

## Prompt template (paste one per phase)

```
继续 desktop-tools-rs-ex 的 <P0.x 名称>(先读 AGENTS.md / ROADMAP.md /
PLAN.md / CHECKLIST.md 的对应节)。要求:
- 只实现这一个 phase 的验收标准,不提前做下一 phase;
- 纯逻辑(解析、拼接规则、几何、diff)先写单测再实现;
- 涉及第三方库先用 context7 查当前文档再选 crate;
- 完成后跑 rebuild_ex.ps1 自动重启,更新 CHECKLIST.md,
  给我物理验证清单。
```

After physically verifying, reply with results (and any misfires), then
paste the template again with the next phase.

## Skills and tools

| Tool / skill | When |
|---|---|
| `cq` | Always first per global AGENTS.md: query before acting, propose non-obvious findings immediately (the VIBE√ gate applies). |
| `context7` / context7-mcp | Any library question: onnxruntime/rapidocr bindings, tesseract bindings, ffmpeg CLI, `windows` crate APIs (DDA, layered windows). |
| `tdd` | Pure logic phases: SizeSpec presets, paragraph join, pixel-diff. |
| `codemark` | Bookmark critical spots across sessions (overlay hit-test, engine router). |
| OpenSpec or similar | **Not used.** Single-agent + single-user phase review is covered by ROADMAP/PLAN/CHECKLIST; a spec framework adds ceremony without adding verification. Revisit only if multiple agents start working phases in parallel. |

## Decision log (fill as phases complete)

| Phase | Decision | Why |
|---|---|---|
| P0.1 | Workspace shape: `dt-common` + `dt-ocr` crates | Tools share config/keys/tray; recording/scaling arrive later as own crates |
| P0.3 | Overlay is one layered window with per-word hit-testing (Text-Grab model), click-through only while OCR runs | Spec requirement: no blocking during processing; blocking only after render |
| P0.3 | Interim engine = `Windows.Media.Ocr` (zh-Hans-CN first, then en) | Word boxes with zero provisioning let the overlay be verified before P0.4; P0.4 swaps the provider behind `ocr::Word` |
| P0.3 | Mask gaps use alpha 1, not 0 | Layered alpha-0 pixels pass mouse input through; the mask must own the whole frame region after render |
| P0.3 | OCR captures are upscaled 4x with GDI halftone before recognition | Native-resolution Windows OCR turns 8-10 pt screen text into noise; 4x fixed digits and CJK. Clamped by `MaxImageDimension` + a 16 Mpx budget |
| P0.3 | Auto language = zh pass + en pass merged per line | The zh model mangles long Latin runs, the en model drops CJK; merge keeps ≥20%-CJK lines on zh and takes en for Latin lines |
| P0.3 | Mask fill = continuous band per OCR line segment (`[mask] fill = "lines"`, default) | "Connected text" look without reflow: glyphs keep OCR positions, so spacing is exact by construction and word hit-testing stays aligned; gaps > 3/4 em (tabs/columns) stay separate |
| P0.3 | `toggle` binding = `win+shift+o`, hides/shows the existing mask | Win+O is reserved by Windows (RegisterHotKey 1409); toggling must not re-run OCR, so the HWND and words are reused |
| P0.3 | Selection text = `join_words` inside a line + `\n` between OCR lines | Preserves line breaks, spaces between Latin words, no spaces inside CJK |
| P0.4 | RapidOCR measured (AHK-ScreenOCR CLI): 4/4 lines at native res, ~1.6 s/run, line boxes only | Best quality by far; needs a persistent worker for latency and a word-splitting plan to keep word-level clicks |
| P0.4 | `auto` engine = language-table routing; explicit keys force engine | Answers the user's multi-engine interaction question with the smallest rule set; revisit after real use |
| P1.3 | pending | choose at phase start: wrap wcap.exe vs own DDA capture |
| P1.4 | pending | Magpie is D3D11+shader heavy; v1 may be simple ratio stretch |

## Verification protocol (mirrors desktop-tools-rs)

- Automated: `cargo test`, `cargo clippy --all-targets`, `cargo fmt
  --check`, then `rebuild_ex.ps1` (kill → build → sync config → CIM
  launch → RegisterHotKey probe / process check).
- Human: the CHECKLIST items for that phase, pressed/clicked by hand on the
  real machine. Injected input cannot test hotkeys on this machine (alt+shift
  combos are swallowed by the input stack); physical verification is the only
  ground truth.
- A phase is DONE when its checklist is human-confirmed, not when tests pass.
