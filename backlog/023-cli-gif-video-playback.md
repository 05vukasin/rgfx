# 023 — CLI GIF & video playback

- **Crate:** `rgfx-cli`
- **Depends on:** 020, 010 (gif), 018 + 019 (video), 004/005, 007 (006 optional)
- **Blocks:** —
- **Branch:** `task/023-cli-gif-video-playback`
- **Skills:** `finish-task`

## Goal
Wire animated playback end to end: `rgfx animation.gif` and `rgfx video.mp4` play in the
terminal with the frame engine, honoring frame timing, with pause/seek/restart controls and
responsive resize.

## Scope / deliverables
1. A playback viewer implementing the dispatch trait, driven by any `FrameSource`
   (`GifSource` from 010, `Player`/`VideoSource` from 018/019).
2. Playback loop using the frame engine (007): pace to `frame_delay()`/fps, diff-render each
   frame, adaptive skip when behind (video).
3. Controls: `Space` pause/resume, `←→` seek (video), `R` restart, `+/-` fps (where meaningful),
   `Q`/`Esc` quit. `--fps` and `--renderer` respected.
4. Responsive resize: recompute framebuffer size mid-playback.
5. Graceful message if `ffmpeg` is unavailable for video (from 018's error).

## Contracts / API
CLI glue only. Reuse one framebuffer across all frames. Terminal cleanup guaranteed.

## Acceptance criteria
- [ ] `rgfx small.gif` (bundled) plays all frames with correct timing and loops/ends per config.
- [ ] `rgfx clip.mp4` plays when ffmpeg is present; clean error when absent.
- [ ] Pause/seek/restart work; resize mid-playback doesn't crash. Clippy `-D warnings`.

## Tests required
- [ ] Headless playback of a bundled multi-frame GIF: correct frame count + ordering through the viewer.
- [ ] Playback control state machine (pause/seek/restart) unit-tested with a mock `FrameSource`.
- [ ] Resize recompute mid-stream.

## Out of scope
Audio. Video decoding internals (018/019).

## Completion
- [ ] Implemented
- [ ] Finish gate green
- [ ] Manual check: real GIF + MP4 play smoothly in a terminal
- [ ] PR opened: <!-- url -->
- [ ] Merged to `main`

**Status:** ✅ DONE (merged)
