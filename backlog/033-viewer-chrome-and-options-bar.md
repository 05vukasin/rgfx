# 033 — Unified viewer chrome & options bar (image · gif · video)

- **Crate:** `rgfx-cli` (small shared helper; possibly a `viewer_chrome` module)
- **Depends on:** 032 (image full-screen), 023 (gif/video playback), 022 (3D viewer)
- **Branch:** `task/033-viewer-chrome-and-options-bar`

## Motivation
"It should have additional options down there as well." Every interactive viewer should present a
consistent bottom **status/options bar** and a consistent control scheme — the 3D viewer has one;
the image (task 032) and gif/video viewers should match, so the whole tool feels like one app.

## Scope / deliverables
1. **Extract a shared status-bar helper** from the 3D viewer (`mesh_viewer`) into a small
   `viewer_chrome` module: given a left-aligned info segment list and a key-help line, compose the
   bottom two rows, truncated to the viewport width, with a consistent style. Reuse it in the 3D,
   image (032), and gif/video viewers.
2. **GIF/video options bar**: file name, frame index / total, fps, loop state, renderer, elapsed /
   duration (video). Controls surfaced: `Space` pause, `←/→` seek (video), `R` restart,
   `+/-` fps, renderer cycle, `F` toggle bar, `Q` quit — matching the existing playback logic.
3. **Consistent key map** across viewers where it makes sense (`F` toggles the bar, `Q`/`Esc`
   quits, `R` reset/restart) — documented in one place and in the README.
4. Ensure the bar never corrupts the render: it is drawn as overlay cells within the viewport and
   accounted for in the render height (no scroll), consistent with the 3D viewer.

## Acceptance criteria
- [ ] All three raster/animated viewers show a bottom options bar with the same look, toggle (`F`), and quit keys.
- [ ] GIF/video bar shows live frame/fps/loop/position and reflects pause/seek.
- [ ] The bar is width-truncated and never causes a scroll or misalignment (PTY check).
- [ ] Shared helper is unit-tested; clippy `-D warnings` clean.

## Tests required
- [ ] `viewer_chrome` composition: segments + help line → expected two-row layout at several widths.
- [ ] GIF/video status fields update with playback state (mock source/clock).

## Out of scope
New playback features; only surfacing existing state + shared chrome.

## Completion
- [ ] Implemented · [ ] Gate green + PTY check · [ ] PR opened · [ ] Merged

**Status:** ⬜ NOT STARTED
