# 026 — Benchmarks

- **Crate:** `benches/` (workspace-level, or a `rgfx-bench` crate)
- **Depends on:** 013 (rasterizer), 004 (braille), 007 (frame engine)
- **Blocks:** 025-optimization work (future)
- **Branch:** `task/026-benchmarks`
- **Skills:** `finish-task`

## Goal
Establish the benchmarking harness from the spec so future optimization work has a baseline.
Measure the stages that matter: transform, rasterization, Braille encode, terminal diff, total
frame time, FPS.

## Scope / deliverables
1. Add `criterion` (dev-dependency) benches.
2. Procedurally generate benchmark meshes at 1k / 10k / 100k / 1M triangles (no large asset
   files in the repo).
3. Benchmarks for: mesh transform, triangle rasterization, Braille encode of a framebuffer,
   terminal diff of two frames, and end-to-end frame render.
4. A short `BENCHMARKS.md` documenting how to run (`cargo bench`) and interpret them, with a
   baseline table filled from a local run.

## Contracts / API
Benches use public APIs only. Don't add benches to CI's default path (they're slow); optionally
a separate manual workflow.

## Acceptance criteria
- [ ] `cargo bench` runs all benches without error.
- [ ] Mesh generators produce the exact requested triangle counts.
- [ ] BENCHMARKS.md documents commands + a baseline. Clippy `-D warnings` on bench code.

## Tests required
- [ ] Unit test the procedural mesh generators (counts + validity), so benches can't silently drift.

## Out of scope
Actual optimization (Rayon/SIMD/tiling) — that's a future task; this only measures.

## Completion
- [ ] Implemented
- [ ] Finish gate green
- [ ] PR opened: <!-- url -->
- [ ] Merged to `main`

**Status:** ✅ DONE (merged)
