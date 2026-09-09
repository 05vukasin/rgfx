# 005 — ASCII & Unicode block encoders

- **Crate:** `rgfx-terminal`
- **Depends on:** 003
- **Blocks:** 021, 022
- **Branch:** `task/005-ascii-and-block-encoders`
- **Skills:** `rgfx-architecture`, `finish-task`

## Goal
Two more `TerminalEncoder` implementations that share the framebuffer pipeline: an ASCII
density encoder and a Unicode half-block encoder.

## Scope / deliverables
1. `AsciiEncoder`: maps cell luminance to a configurable density ramp (default
   `" .:-=+*#%@"`), one char per terminal cell (1×1 framebuffer sampling, or averaged block).
   Options: custom ramp, invert, gamma.
2. `BlockEncoder`: uses the upper/lower half-block trick (`▀` with fg=top pixel, bg=bottom
   pixel) to get 1×2 vertical resolution per cell, plus shading glyphs `█ ▌ ▐ ░ ▒ ▓`. This
   encoder benefits from color (006) but must produce sensible grayscale output without it.
3. Both consume `Framebuffer` at the appropriate sampling ratio for the requested `Viewport`.

## Contracts / API
Same `TerminalEncoder::encode` signature. No stdout, no loader deps. Structure `BlockEncoder`
so 006 can attach fg/bg color per cell.

## Acceptance criteria
- [ ] ASCII: black→first ramp char, white→last ramp char, monotonic mapping across a gradient.
- [ ] Blocks: a two-tone framebuffer produces the correct half-block glyph with the right
      top/bottom assignment.
- [ ] Output dimensions match the viewport; clippy `-D warnings`; documented.

## Tests required
- [ ] ASCII ramp mapping across luminance values (incl. custom ramp + invert).
- [ ] Half-block glyph selection for top/bottom lit combinations.
- [ ] Snapshot of a small known framebuffer for each encoder.

## Out of scope
Color layer (006); braille (004).

## Completion
- [ ] Implemented
- [ ] Finish gate green
- [ ] PR opened: <!-- url -->
- [ ] Merged to `main`

**Status:** ⬜ NOT STARTED
