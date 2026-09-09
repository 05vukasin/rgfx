# 004 — Braille encoder

- **Crate:** `rgfx-terminal`
- **Depends on:** 003
- **Blocks:** 021, 022, 026, 028, 029
- **Branch:** `task/004-braille-encoder`
- **Skills:** `rgfx-architecture`, `finish-task`

## Goal
Implement the flagship `TerminalEncoder`: a Braille encoder that maps a `Framebuffer` to a
`TerminalFrame` of U+2800-range glyphs, one cell per 2×4 pixel block. This is the primary
renderer and the visual benchmark of the project.

## Scope / deliverables
1. `BrailleEncoder` implementing `rgfx_core::TerminalEncoder`.
2. Correct 2×4 → 8-bit mask math per the `rgfx-architecture` skill (dot positions 1,2,3,7 in
   col0 and 4,5,6,8 in col1; base `U+2800`).
3. Options struct: `threshold`, `invert`, `gamma`, `contrast`, `edge_enhance` (dithering lives
   in the image crate but the encoder must accept an already-dithered/boolean input cleanly).
4. Grayscale decision per subpixel from `Framebuffer::luma` vs threshold (after gamma/contrast).
5. Respect the requested `Viewport`: the encoder consumes a framebuffer sized `cols*2 × rows*4`.

## Contracts / API
`encode(&self, frame: &Framebuffer, viewport: Viewport) -> TerminalFrame`. Do not read stdin or
write stdout. Do not depend on any loader/image crate.

## Acceptance criteria
- [ ] A fully-lit 2×4 block encodes to `⣿` (U+28FF); empty block to `⠀` (U+2800).
- [ ] Individual dot positions produce the exact expected code points (golden test per dot).
- [ ] `invert` flips lit/unlit; `threshold` boundary behaves as documented.
- [ ] Output line count == viewport rows; each line char count == viewport cols.
- [ ] Clippy `-D warnings`; documented.

## Tests required
- [ ] Snapshot: a known small framebuffer (e.g. a diagonal / a circle) → exact Braille string.
- [ ] Per-dot mask table test (all 8 dots individually → correct glyph).
- [ ] Threshold + invert combinations on a gradient.

## Out of scope
ANSI color (006) — but structure the encoder so 006 can add per-cell color without a rewrite.

## Completion
- [ ] Implemented
- [ ] Finish gate green
- [ ] Visual spot-check vs https://lachlanarthur.github.io/Braille-ASCII-Art/
- [ ] PR opened: <!-- url -->
- [ ] Merged to `main`

**Status:** ✅ DONE (merged)
