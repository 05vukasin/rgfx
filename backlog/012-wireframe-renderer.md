# 012 — Wireframe renderer

- **Crate:** `rgfx-3d`
- **Depends on:** 011
- **Blocks:** 022
- **Branch:** `task/012-wireframe-renderer`
- **Skills:** `rgfx-architecture`, `finish-task`

## Goal
The first visible 3D milestone: render mesh edges as lines into a `Framebuffer`. Deliver a
built-in cube so the CLI can show interactive 3D before the full rasterizer exists.

## Scope / deliverables
1. Line rasterization (Bresenham or DDA) into a `Framebuffer` with clipping to the viewport.
2. A `WireframeRenderer` that projects mesh vertices via a `Camera` (from 011) and draws edges
   (deduplicated from triangle indices).
3. A built-in `Mesh::cube()` (and ideally a `tetrahedron`) generator for demos/tests.
4. Partial `SceneRenderer` implementation or a dedicated `render_wireframe(scene, camera, fb)`
   entry point (coordinate the trait shape with 013's author; wireframe is also a shading mode).

## Contracts / API
Draw into `rgfx_core::Framebuffer`. Use the `Camera` from 011. No terminal/encoder deps.

## Acceptance criteria
- [ ] Bresenham draws a known line's exact pixel set (horizontal, vertical, diagonal, steep).
- [ ] Lines clip correctly at framebuffer edges (no OOB writes, no panics).
- [ ] A projected cube produces the expected number of visible edges from a known camera.
- [ ] Clippy `-D warnings`; documented.

## Tests required
- [ ] Line rasterization pixel-set tests for several slopes + off-screen endpoints.
- [ ] Cube edge count / dedup test.
- [ ] Projected-cube snapshot into a small framebuffer (deterministic camera).

## Out of scope
Filled triangles / z-buffer (013), lighting (014), file loaders.

## Completion
- [ ] Implemented
- [ ] Finish gate green
- [ ] PR opened: <!-- url -->
- [ ] Merged to `main`

**Status:** ✅ DONE (merged)
