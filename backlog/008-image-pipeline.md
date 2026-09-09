# 008 — Image pipeline

- **Crate:** `rgfx-image` (creates the crate)
- **Depends on:** 001
- **Blocks:** 009, 010, 018, 021, 025
- **Branch:** `task/008-image-pipeline`
- **Skills:** `new-crate`, `rgfx-architecture`, `finish-task`

## Goal
Create `rgfx-image` and implement the still-image → `Framebuffer` pipeline for PNG and JPEG:
decode, aspect-correct, resize to a target render size, and convert to color/grayscale.

## Scope / deliverables
1. Scaffold `crates/rgfx-image` (`image.workspace = true`).
2. `load(path) -> Result<DecodedImage>` for PNG + JPEG (RGBA/RGB) via the `image` crate.
3. `to_framebuffer(&DecodedImage, target: Viewport, opts) -> Framebuffer`:
   - aspect-ratio correction accounting for terminal cell aspect (cells are ~1:2 W:H; braille
     subpixels 2×4) — expose the correction factor so the CLI can pass renderer-specific ratios;
   - high-quality resize (Lanczos/Triangle) to the render resolution;
   - write RGBA into the framebuffer color buffer; compute luma lazily via `Framebuffer::luma`.
4. Implement `FrameSource` for a single still image (yields one frame, then `Ok(false)`).

## Contracts / API
Produce `rgfx_core::Framebuffer`. Never touch the terminal or any encoder. Errors via the crate
`Error` (map `image::ImageError`).

## Acceptance criteria
- [ ] A 4×4 test PNG decodes to the expected pixels in the framebuffer.
- [ ] Resize to a target width preserves aspect within 1px given a stated cell aspect ratio.
- [ ] Malformed/empty image input returns `Err`, never panics.
- [ ] Clippy `-D warnings`; documented.

## Tests required
- [ ] Decode a tiny embedded PNG (bytes in test) → known framebuffer values.
- [ ] Aspect-ratio math unit tests for several terminal sizes + cell ratios.
- [ ] Corrupt-bytes input → `Err`.

## Out of scope
Dithering/tone (009), WebP/BMP/GIF (010), the CLI wiring (021).

## Completion
- [ ] Implemented
- [ ] Finish gate green
- [ ] PR opened: <!-- url -->
- [ ] Merged to `main`

**Status:** ✅ DONE (merged)
