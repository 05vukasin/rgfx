# 032 — Full-screen image preview with options bar (screen parity with 3D)

- **Crate:** `rgfx-cli`
- **Depends on:** 021 (image viewer), 030 (inline path), 007 (frame engine), 006 (color)
- **Branch:** `task/032-fullscreen-image-preview`
- **Status legend:** ⬜ NOT STARTED

## Motivation (user report)
Previewing an image should "fix the screen" the way the 3D viewer does — take over the
terminal (alternate screen), show the image centered/fit, and offer **options along the bottom**
— and only behave like `cat` (print inline into scrollback, then return) when an explicit flag is
added. Today (task 030) the image default is the inline/`cat` behavior and full-screen is behind
`--interactive`; this task **flips the default** and adds the options bar.

> ⚠️ Decision to confirm with the maintainer before implementing: make full-screen the DEFAULT
> for `rgfx image.png` and move the inline (`cat`-like) behavior behind a flag. (See CLI below.)

## Scope / deliverables
1. **Default = full-screen interactive preview** for still images: enter the alternate screen via
   the existing `Session`, render the image fit-to-terminal (reusing the framebuffer + the diffing
   `FrameEngine`), and restore the terminal on every exit path (already guaranteed by `Session`).
   Re-render on resize.
2. **Options / status bar** pinned to the bottom (toggle with `F`, like the 3D viewer), showing:
   file name, source dimensions, current renderer, dither mode, zoom %, and a one-line key help.
3. **Interactive controls**: `R` cycle renderer (braille/ascii/blocks), `D` cycle dither,
   `I` invert, `C` toggle color, `+`/`-` zoom, arrows/drag to pan when zoomed, `Q`/`Esc` quit.
   (Mouse: wheel = zoom, drag = pan — reuse the mouse plumbing added in task 031.)
4. **`cat`-like escape hatch**: a flag — proposed `--inline` (alias `--cat`) — prints the encoded
   image inline at terminal width and returns immediately (the current task-030 `run_inline`).
   `--output <file>` and `-`/stdin remain non-interactive. Retire or repurpose `--interactive`
   (now the default) — keep it as a no-op alias for one release with a deprecation note.
5. Update `--help`, the README controls section, and `CHANGELOG`.

## Acceptance criteria
- [ ] `rgfx image.png` opens a full-screen preview with a bottom options bar and restores the terminal on quit.
- [ ] `rgfx image.png --inline` (and `--cat`) prints inline and returns, matching prior behavior; verified under a PTY (lines ≤ rows, exit 0).
- [ ] Renderer/dither/invert/color/zoom controls change the output live; resize re-renders.
- [ ] `--output` and `-` stay non-interactive. Clippy `-D warnings` clean.

## Tests required
- [ ] Pure controls state machine (renderer/dither/invert/zoom transitions) headlessly.
- [ ] Options-bar text composition (fields present, truncated to width) — snapshot.
- [ ] `--inline` path render-to-string at a fixed width (reuse the 030 regression style).
- [ ] Real-PTY check: full-screen enters/restores; `--inline` prints and exits.

## Out of scope
GIF/video chrome (task 033). Color-quantization changes (already in 006).

## Completion
- [ ] Implemented · [ ] Gate green + PTY check · [ ] PR opened · [ ] Merged

**Status:** ✅ DONE (merged)
