# 009 — Dithering & tone controls

- **Crate:** `rgfx-image`
- **Depends on:** 008
- **Blocks:** 021
- **Branch:** `task/009-dithering-and-tone`
- **Skills:** `rgfx-architecture`, `finish-task`

## Goal
Add the image-quality stage that makes Braille/ASCII output look good: dithering algorithms and
tone adjustments applied to a `Framebuffer` (or during framebuffer construction).

## Scope / deliverables
1. Dithering on the luma/threshold path: `None`, `FloydSteinberg`, `Atkinson`, `Bayer` (ordered,
   configurable matrix size), `ThresholdOnly`.
2. Tone: `gamma`, `contrast`, `brightness`, and an optional unsharp `sharpen` pass.
3. A `Preprocess` options struct threaded through `to_framebuffer` (extend 008's `opts`).
4. Operate in-place on the framebuffer where possible (buffer reuse, no per-call alloc storms).

## Contracts / API
Pure framebuffer→framebuffer transforms in `rgfx-image`. No terminal/encoder deps. The encoder
(004) consumes the already-processed framebuffer.

## Acceptance criteria
- [ ] Floyd–Steinberg on a mid-gray flat field produces the classic ~50% checker distribution
      (assert lit-pixel ratio within tolerance).
- [ ] Bayer ordered dithering is deterministic for a given matrix (exact snapshot).
- [ ] Gamma/contrast are monotonic and clamp to [0,1].
- [ ] Clippy `-D warnings`; documented.

## Tests required
- [ ] Each dither algorithm on a known gradient → expected boolean pattern / lit ratio.
- [ ] Gamma/contrast/brightness math on sample values.
- [ ] Sharpen kernel on a step edge increases local contrast.

## Out of scope
Color quantization (that's terminal/006). Decoding/resize (008).

## Completion
- [ ] Implemented
- [ ] Finish gate green
- [ ] Visual spot-check vs the reference tools in the spec
- [ ] PR opened: <!-- url -->
- [ ] Merged to `main`

**Status:** ⬜ NOT STARTED
