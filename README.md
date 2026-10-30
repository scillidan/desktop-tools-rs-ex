# desktop-tools-rs-ex

Extended desktop tools (the heavy/experimental siblings of
[desktop-tools-rs](../desktop-tools-rs)): OCR region frame with overlay mask,
drag-select screen OCR, window/desktop recording, flameshot-window capture,
window scaling.

Status: P0.1–P0.4 done (frame, overlay mask, reader, OCR engines); see
[ROADMAP.md](ROADMAP.md) for what is left, and [AGENTS.md](AGENTS.md) for the
conventions, the interaction model and the design decisions.

Reference implementations:

- Text Grab — https://github.com/TheJoeFin/Text-Grab (OCR frame + overlay)
- Screen OCR (AHK v1, local) — `C:\Users\User\Share\projs\AHK-ScreenOCR`
- wcap — https://github.com/mmozeiko/wcap (fast recording, ffmpeg encoding)
- FlameshotWindow (AHK v1, local) — `C:\Users\User\Share\projs\AHK-FlameshotWindow`
- Magpie — https://github.com/Blinue/Magpie (window scaling; the link in the
  original request pointed at Text-Grab by mistake)
