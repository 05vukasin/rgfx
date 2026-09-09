# 010 — More formats & animated GIF

- **Crate:** `rgfx-image`
- **Depends on:** 008
- **Blocks:** 023
- **Branch:** `task/010-image-formats-and-gif`
- **Skills:** `rgfx-architecture`, `finish-task`

## Goal
Broaden decoding to WebP and BMP, and implement animated GIF as a multi-frame `FrameSource`
with correct frame timing and looping — reusing the 008 framebuffer pipeline for every frame.

## Scope / deliverables
1. Extend `load`/format detection to WebP and BMP (via `image` crate features).
2. `GifSource` implementing `rgfx_core::FrameSource`: decode frames lazily, honor per-frame
   delay via `frame_delay()`, handle disposal methods and looping.
3. Frame-to-framebuffer reuses 008's `to_framebuffer` (no duplicated resize/aspect logic); the
   target framebuffer is reused across frames.

## Contracts / API
`FrameSource::next_frame` renders into the caller-provided reused `Framebuffer`; returns
`Ok(false)` at end (or loops if configured). No terminal deps.

## Acceptance criteria
- [ ] WebP and BMP test images decode to expected framebuffers.
- [ ] A 3-frame GIF yields 3 frames in order with correct per-frame delays, then loops/ends per config.
- [ ] GIF disposal (restore-to-background) composites correctly across frames.
- [ ] Malformed inputs error, don't panic. Clippy `-D warnings`; documented.

## Tests required
- [ ] Embedded tiny WebP/BMP → known pixels.
- [ ] Embedded multi-frame GIF → frame count, ordering, delays, disposal correctness.

## Out of scope
MP4/WebM video (018). CLI playback loop (023).

## Completion
- [ ] Implemented
- [ ] Finish gate green
- [ ] PR opened: <!-- url -->
- [ ] Merged to `main`

**Status:** ⬜ NOT STARTED
