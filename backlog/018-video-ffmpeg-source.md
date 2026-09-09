# 018 — Video via FFmpeg (frame source)

- **Crate:** `rgfx-video` (creates the crate)
- **Depends on:** 001, 008
- **Blocks:** 019, 023
- **Branch:** `task/018-video-ffmpeg-source`
- **Skills:** `new-crate`, `rgfx-architecture`, `finish-task`

## Goal
Create `rgfx-video` and decode video frames into the framebuffer pipeline. Per the spec's
recommended first strategy, shell out to external `ffmpeg`/`ffprobe` (no linking) and pipe raw
RGB frames — keeping FFmpeg an optional runtime dependency, not a build dependency.

## Scope / deliverables
1. Scaffold `crates/rgfx-video`. Base build must **not** require FFmpeg; gate video behind a
   `ffmpeg` cargo feature and detect the `ffmpeg`/`ffprobe` binaries at runtime.
2. `probe(path)` via `ffprobe` → duration, fps, width/height, codec (JSON parse).
3. `VideoSource` implementing `rgfx_core::FrameSource`: spawn `ffmpeg -i … -f rawvideo -pix_fmt
   rgb24 -` , read frames from stdout, convert each to a `Framebuffer` via 008's pipeline
   (reused framebuffer, honor target viewport). `frame_delay()` from the source fps.
4. Clean subprocess lifecycle (kill child on drop/exhaustion; handle broken pipe).
5. Clear error if `ffmpeg` is missing (actionable message, no panic).

## Contracts / API
`FrameSource` into a reused `Framebuffer`. No terminal deps. Errors via crate `Error`.

## Acceptance criteria
- [ ] `probe` on a tiny bundled/generated clip returns correct dims/fps (test skipped/ignored if ffmpeg absent).
- [ ] `VideoSource` yields the expected frame count for a short clip; frames convert to framebuffers.
- [ ] Missing `ffmpeg` → actionable `Err`, never a panic.
- [ ] Child process is reaped on drop (no zombies). Clippy `-D warnings`.

## Tests required
- [ ] ffprobe JSON parsing unit test (fixture JSON, no ffmpeg needed).
- [ ] Frame-reader chunking logic (raw RGB byte stream → frames) with a synthetic byte buffer.
- [ ] `#[ignore]`/feature-gated integration test that runs a real short clip when ffmpeg present.

## Out of scope
Playback controls (019), CLI wiring (023), the `ffmpeg-next` linked approach (future).

## Completion
- [ ] Implemented
- [ ] Finish gate green
- [ ] PR opened: <!-- url -->
- [ ] Merged to `main`

**Status:** ✅ DONE (merged)
