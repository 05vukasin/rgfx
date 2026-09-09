# 007 — Frame engine (double buffer + diff + timing)

- **Crate:** `rgfx-terminal`
- **Depends on:** 003
- **Blocks:** 022, 023, 028
- **Branch:** `task/007-frame-engine-diffing`
- **Skills:** `rgfx-architecture`, `finish-task`

## Goal
The performance core of the terminal layer: maintain front/back `TerminalFrame`s and write only
changed cells, with batched output and frame timing. This is what makes interactive 3D and
video playback smooth without flicker.

## Scope / deliverables
1. A `FrameEngine` holding front + back `TerminalFrame` buffers (reused, not reallocated).
2. `render(&mut self, next: &TerminalFrame, out: &mut impl Write)`: diff `next` vs the current
   front buffer and emit cursor-move + write only for changed cell runs; then swap buffers.
3. Full-redraw path on resize or first frame.
4. Frame timing helpers: target FPS pacing, and adaptive frame skipping when behind (used by
   video). Expose `FrameClock`/`present` primitives.
5. Batched writes: coalesce runs, minimize cursor moves, single flush per frame (build on 003's
   buffered writer).

## Contracts / API
Operate on `rgfx_core::TerminalFrame`. Diff must be correct with color cells (coordinate with
006's cell representation). No `println!`.

## Acceptance criteria
- [ ] Changing one cell between frames writes only that cell's run (assert on emitted bytes).
- [ ] Identical consecutive frames emit ~nothing (only a possible flush).
- [ ] Resize/first-frame triggers a full redraw.
- [ ] Buffers are reused across frames (capacity stable).

## Tests required
- [ ] Diff: single-cell change, whole-row change, no change, size change → expected byte output
      against an in-memory writer.
- [ ] FrameClock pacing math (target vs elapsed → sleep/skip decision) is deterministic under a
      mocked clock.

## Out of scope
The encoders (004/005) and color serialization (006) — consume their output.

## Completion
- [ ] Implemented
- [ ] Finish gate green
- [ ] PR opened: <!-- url -->
- [ ] Merged to `main`

**Status:** ✅ DONE (merged)
