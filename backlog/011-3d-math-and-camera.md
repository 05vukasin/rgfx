# 011 — 3D math & camera

- **Crate:** `rgfx-3d` (creates the crate)
- **Depends on:** 001
- **Blocks:** 012, 013, 022
- **Branch:** `task/011-3d-math-and-camera`
- **Skills:** `new-crate`, `rgfx-architecture`, `finish-task`

## Goal
Create `rgfx-3d` and implement the math/transform layer and camera model needed by every 3D
task: model/view/projection transforms, perspective + orthographic cameras, orbit/zoom/pan
controls, and automatic model framing from a bounding sphere.

## Scope / deliverables
1. Scaffold `crates/rgfx-3d` (`glam.workspace = true`).
2. `Camera` with `PerspectiveCamera`/`OrthographicCamera` variants: position, target, up, fov,
   near, far, aspect. `view_matrix()`, `projection_matrix()`, `view_proj()`.
3. Orbit controls: orbit around X/Y, zoom (dolly), pan, reset — as methods mutating the camera.
4. `auto_frame(&mut self, bounds: &BoundingSphere, viewport_aspect)`: position the camera so the
   mesh fits the viewport with sensible near/far (README "Automatic Model Framing").
5. Update aspect from a `Viewport` on resize.

## Contracts / API
Use `rgfx_core` mesh/bounds types (or define `BoundingSphere` in core if missing — flag in PR,
don't silently fork). `glam` for all linear algebra. No rasterization here.

## Acceptance criteria
- [ ] View matrix places a known point at the expected camera-space coordinate.
- [ ] Perspective projection maps a point on the near plane / center as expected.
- [ ] `auto_frame` fits a unit sphere so its projected radius is within the viewport.
- [ ] Orbit by 360° returns to the original orientation (within epsilon).
- [ ] Clippy `-D warnings`; documented.

## Tests required
- [ ] view/projection matrix values against hand-computed references.
- [ ] auto-framing produces in-viewport projected bounds for several aspect ratios.
- [ ] orbit/zoom/pan/reset invariants.

## Out of scope
Rasterization (013), wireframe (012), shading (014), loaders (015–017).

## Completion
- [ ] Implemented
- [ ] Finish gate green
- [ ] PR opened: <!-- url -->
- [ ] Merged to `main`

**Status:** ⬜ NOT STARTED
