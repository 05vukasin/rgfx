# 019 — Video playback controls

- **Crate:** `rgfx-video`
- **Depends on:** 018
- **Blocks:** 023
- **Branch:** `task/019-video-playback-controls`
- **Skills:** `rgfx-architecture`, `finish-task`

## Goal
Add playback control to the video source: pause, seek, restart, and adaptive frame skipping so
playback stays in sync when rendering can't keep up.

## Scope / deliverables
1. A `Player` wrapping `VideoSource`: `play`/`pause`/`toggle`, `seek(Duration)` (respawn ffmpeg
   with `-ss`), `restart`.
2. Timing model driven by frame PTS / source fps; integrate with the frame engine's clock (007)
   so the CLI can pace playback.
3. Adaptive frame skipping: when behind schedule, drop frames to catch up (bounded).
4. State query: current position, duration, paused, fps.

## Contracts / API
Keep it decoupled from the terminal — the `Player` produces frames + timing; the CLI (023)
drives the loop and rendering. Errors via crate `Error`.

## Acceptance criteria
- [ ] Pause halts frame advancement; resume continues from the same frame.
- [ ] Seek repositions to the target timestamp (±1 frame) via ffmpeg `-ss`.
- [ ] Under simulated slow rendering, adaptive skipping keeps wall-clock position within tolerance.
- [ ] Clippy `-D warnings`; documented.

## Tests required
- [ ] Pause/resume state machine (mocked clock/source).
- [ ] Adaptive-skip decision math (behind by N frames → skip count) deterministic under a mock clock.
- [ ] Seek argument construction (`-ss` formatting) unit test.

## Out of scope
Audio (later). CLI key bindings (023).

## Completion
- [ ] Implemented
- [ ] Finish gate green
- [ ] PR opened: <!-- url -->
- [ ] Merged to `main`

**Status:** ⬜ NOT STARTED
