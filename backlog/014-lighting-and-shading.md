# 014 — Lighting & shading

- **Crate:** `rgfx-3d`
- **Depends on:** 013
- **Blocks:** 022
- **Branch:** `task/014-lighting-and-shading`
- **Skills:** `rgfx-architecture`, `finish-task`

## Goal
Add shading to the rasterizer: normals, ambient + directional diffuse light, and flat vs smooth
(Gouraud) shading modes, plus the debug modes (normals/depth) and wireframe cycling.

## Scope / deliverables
1. Per-face normals (flat) and per-vertex interpolated normals (smooth), computed from geometry
   if the mesh lacks normals.
2. Directional light: `intensity = max(0, dot(n, light_dir))` + ambient term; clamp to [0,1].
3. `ShadingMode` enum finalized: `Unlit`, `Flat`, `Smooth`, `Normals`, `Depth`, `Wireframe`.
4. Wire modes into the 013 `Rasterizer` fragment stage (perspective-correct normal interp for
   smooth). Configurable light direction, ambient level, base color.

## Contracts / API
Extend the 013 `Rasterizer`/`SceneRenderer`; do not fork it. Shading writes final `Color` into
the framebuffer. No terminal deps.

## Acceptance criteria
- [ ] Flat shading: a face facing the light is brighter than one facing away, per the dot law.
- [ ] Smooth shading interpolates intensity across a face (no faceting on a subdivided sphere).
- [ ] Normals mode encodes normal→color deterministically; depth mode maps z→grayscale.
- [ ] Ambient floor prevents fully-black unlit-but-visible faces. Clippy `-D warnings`.

## Tests required
- [ ] Lambert intensity for known normal/light pairs.
- [ ] Flat vs smooth intensity difference on a two-triangle quad with differing vertex normals.
- [ ] Normals/depth mode mapping unit tests.

## Out of scope
Textures/PBR (non-goal for MVP). File loaders (015–017).

## Completion
- [ ] Implemented
- [ ] Finish gate green
- [ ] PR opened: <!-- url -->
- [ ] Merged to `main`

**Status:** ⬜ NOT STARTED
