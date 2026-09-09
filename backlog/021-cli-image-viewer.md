# 021 — CLI image viewer

- **Crate:** `rgfx-cli`
- **Depends on:** 020, 008, 009, 004, 005 (006 for color if available)
- **Blocks:** —
- **Branch:** `task/021-cli-image-viewer`
- **Skills:** `finish-task`

## Goal
Wire the still-image path end to end: `rgfx image.png` decodes, builds a framebuffer sized to
the terminal, encodes with the chosen renderer, and prints it — responsive to resize, with
`--width`, `--renderer`, `--color`, dithering options, and `--output` to save text.

## Scope / deliverables
1. Image viewer implementing the dispatch trait from 020: load via `rgfx-image` (008), apply
   preprocessing (009: dither/gamma/contrast per flags/config), size the framebuffer from the
   current `Viewport` (respecting `--width` override and renderer subpixel ratio), encode via
   the selected `TerminalEncoder` (004/005, color via 006 when `--color`).
2. Responsive: on `Resize` event, recompute render size and re-render. Static image otherwise
   idles (event-driven, no busy redraw).
3. `--output <file>`: write the encoded text to a file instead of (or in addition to) the screen
   — non-interactive, no alt screen in that mode.
4. `auto` renderer selection heuristic (e.g. braille default; blocks when `--color`).

## Contracts / API
Compose existing crates only; no new rendering logic in the CLI beyond glue + layout. Respect
`CLAUDE.md` boundaries.

## Acceptance criteria
- [ ] `rgfx some.png` renders without panics; quitting restores the terminal.
- [ ] `--width 30` vs `--width 100` produce correspondingly sized output.
- [ ] `--output out.txt` writes valid Braille/ASCII text (assert file contents in a test via the non-interactive path).
- [ ] Resize handling recomputes size (unit-test the size-calc glue). Clippy `-D warnings`.

## Tests required
- [ ] Non-interactive render-to-string of a tiny embedded PNG at a fixed width → snapshot.
- [ ] Renderer + width + color flag combinations produce expected output dimensions.
- [ ] `--output` writes expected bytes.

## Out of scope
3D (022), gif/video (023), stdin (025).

## Completion
- [ ] Implemented
- [ ] Finish gate green
- [ ] Manual check: real PNG/JPEG in a terminal looks correct vs reference tools
- [ ] PR opened: <!-- url -->
- [ ] Merged to `main`

**Status:** ✅ DONE (merged)
