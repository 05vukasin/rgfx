# 029 — Avatar & procedural animation API

- **Crate:** `rgfx-core` (types) + `rgfx-terminal`/`rgfx-cli` (playback)
- **Depends on:** 004 (braille), 007 (frame engine)
- **Blocks:** —
- **Branch:** `task/029-avatar-animation-api`
- **Skills:** `rgfx-architecture`, `finish-task`

## Goal
Add the reusable animation primitives from the spec (`Frame`, `Animation`, `Sprite`,
`AnimationPlayer`) and a small set of CLI avatar states — the foundation for AI-CLI avatars and
loading spinners.

## Scope / deliverables
1. In `rgfx-core`: `Frame` (a framebuffer or encoded cells + duration), `Animation` (ordered
   frames + loop policy), `Sprite`, and `AnimationPlayer` (advance by elapsed time, implements
   `FrameSource`). Keep these decoupled from any specific encoder.
2. A set of built-in state animations: `Idle`, `Thinking`, `Speaking`, `Loading`, `Success`,
   `Error` — small Braille/Unicode frame sequences (the spec's spinner example, scaled up).
3. A CLI entry (e.g. `rgfx avatar --state thinking`) or a documented library example that plays
   an animation via the frame engine.

## Contracts / API
`AnimationPlayer: FrameSource` so it flows through the same pipeline as images/video. Built-in
frame data lives with the animation types; no ad-hoc terminal writes in the library.

## Acceptance criteria
- [ ] `AnimationPlayer` advances frames by elapsed time and loops per policy (deterministic under a mock clock).
- [ ] Each built-in state has a valid, non-empty frame sequence that renders through an encoder.
- [ ] The CLI/example plays an animation and exits cleanly. Clippy `-D warnings`; documented.

## Tests required
- [ ] Player timing/advance/loop logic with a mocked clock.
- [ ] Each built-in state animation is well-formed (frame count, durations, encodes without panic).

## Out of scope
Rigged/skeletal animation, glTF animation playback (future 3D task). Audio.

## Completion
- [ ] Implemented
- [ ] Finish gate green
- [ ] PR opened: <!-- url -->
- [ ] Merged to `main`

**Status:** ⬜ NOT STARTED
