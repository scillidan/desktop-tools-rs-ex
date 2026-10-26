# CHECKLIST.md — desktop-tools-rs-ex

Physical verification, per phase. A phase is done only when every box here
is confirmed by hand on the real machine. Agents: after implementing a
phase, run `rebuild_ex.ps1`, then fill in the concrete steps for the user —
never hand the user a build or restart step.

## P0.1 Scaffold + dt-common
- [ ] (agent fills after implementation)

## P0.2 OCR frame window
- [ ] Draw a frame by dragging; it lands where drawn and stays on top.
- [ ] `Ctrl+Alt+S` then `1600` / `100%` / `1/2` each resize the frame as
      expected (fraction/percent relative to the work area).
- [ ] `Ctrl+T` pins: frame no longer moves or eats clicks; again unpins.
- [ ] Capture key: the exact pixels under the frame arrive in the
      clipboard/file (paste into an image viewer and compare).

## P0.3 Overlay mask
- [ ] While OCR runs, everything under the frame stays clickable.
- [ ] After OCR: every recognized word shows as clickable text with a
      backing box hiding the original text.
- [ ] Click one word → clipboard holds exactly that word.
- [ ] Drag across several words → clipboard holds one paragraph; Latin
      words are space-joined, CJK runs have no spaces.
- [ ] Right-click or Esc dismisses the overlay; the frame is idle again.

## P0.4 OCR engines + language routing
- [ ] Chinese UI text under `auto` OCRs correctly (RapidOCR path).
- [ ] English page under `auto` OCRs correctly (Tesseract path).
- [ ] Forcing the other engine explicitly still produces text.
- [ ] Language detection matches the AHK-ScreenOCR behavior the user is
      used to (spot-check the same screens as the old tool).

## P0.5 Auto-refresh triggers
- [ ] Text inside the frame changes → re-OCR happens by itself.
- [ ] Static frame → no re-OCR (no flicker, no CPU spin).

## P1.x
- [ ] (filled when each P1 phase starts)
