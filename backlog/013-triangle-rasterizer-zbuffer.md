# 013 — Triangle rasterizer & z-buffer

- **Crate:** `rgfx-3d`
- **Depends on:** 011
- **Blocks:** 014, 015, 016, 017, 022, 026
- **Branch:** `task/013-triangle-rasterizer-zbuffer`
- **Skills:** `rgfx-architecture`, `finish-task`

## Goal
The CPU software rasterizer core: transform → clip → project → viewport → backface cull →
barycentric fill → depth test into the `Framebuffer`'s color+depth buffers. This is the heart
of the 3D engine and defines the `SceneRenderer` trait implementation everyone else extends.

## Scope / deliverables
1. Full vertex pipeline: model→view→projection, homogeneous clip, perspective divide, viewport
   transform. Near-plane clipping of triangles (at minimum) to avoid divide-by-zero artifacts.
2. Backface culling (winding order configurable).
3. Barycentric-coordinate triangle fill with perspective-correct interpolation of depth (and
   attributes: normals, color) — hooks 014 will use for shading.
4. Z-buffer depth test using `Framebuffer::depth_mut` (`frag_depth < depth[i]` writes).
5. Implement `rgfx_core::SceneRenderer` for a `Rasterizer` with a `ShadingMode` param (start
   with unlit/depth/normals; flat/smooth come in 014). Wireframe mode delegates to 012.

## Contracts / API
Implement `SceneRenderer::render(&mut self, scene, camera, target)`. Reuse the framebuffer's
depth buffer (cleared to 1.0). No terminal/encoder deps, no stdout.

## Acceptance criteria
- [ ] A single screen-space triangle fills exactly the expected pixels (edge rule consistent, no gaps/overdraw on shared edges).
- [ ] Depth test: a nearer triangle occludes a farther one at overlapping pixels.
- [ ] Backface culling removes CW/CCW-wound triangles per config.
- [ ] Near-plane clipping prevents artifacts for a triangle crossing the camera plane.
- [ ] No OOB writes for triangles partially/fully off-screen; no panics. Clippy `-D warnings`.

## Tests required
- [ ] Barycentric fill pixel-set for a known triangle (+ edge/shared-edge cases).
- [ ] Depth occlusion test with two overlapping triangles.
- [ ] Backface cull + clipping unit tests.
- [ ] Snapshot: a known mesh (cube) rendered depth/normals into a small framebuffer.

## Out of scope
Lighting/flat/smooth shading (014), mesh file loaders (015–017).

## Completion
- [ ] Implemented
- [ ] Finish gate green
- [ ] PR opened: <!-- url -->
- [ ] Merged to `main`

**Status:** ⬜ NOT STARTED
