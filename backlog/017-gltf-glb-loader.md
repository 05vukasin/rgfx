# 017 — glTF / GLB loader

- **Crate:** `rgfx-3d`
- **Depends on:** 013
- **Blocks:** 022, 024
- **Branch:** `task/017-gltf-glb-loader`
- **Skills:** `rgfx-architecture`, `finish-task`

## Goal
Load modern glTF 2.0 / GLB assets via the `gltf` crate — static meshes first, with node
transforms applied so multi-mesh scenes render correctly.

## Scope / deliverables
1. `load_gltf(path) -> Result<Scene>` for `.gltf` (+ external buffers) and `.glb` (binary).
2. Progressive support (this task = steps 1–2): static meshes + node transform hierarchy
   (bake node world transforms into the scene). Materials/textures/animation are later tasks.
3. Base color factor from materials → mesh base color (no texture sampling yet).
4. Bounding box/sphere; stats (meshes, vertices, triangles, materials, animations count) for `info`.
5. Robust errors for malformed assets.

## Contracts / API
Return `rgfx_core` `Scene`/`Mesh` with world-space transforms applied. No rendering/terminal deps.

## Acceptance criteria
- [ ] A known `.glb` and `.gltf` load with correct mesh/triangle counts.
- [ ] Node transforms are applied (a translated child mesh ends up in the right place — assert bounds).
- [ ] Reports animation/material counts (even though not yet rendered) for `info`.
- [ ] Malformed input → `Err`, no panic. Clippy `-D warnings`.

## Tests required
- [ ] Load embedded/`assets/` minimal glb + gltf → counts + bounds.
- [ ] Node-transform application test (translated node → shifted bounds).
- [ ] Malformed input → `Err`.

## Out of scope
Textures, PBR, skinning, animation playback (future tasks). Point clouds (later).

## Completion
- [ ] Implemented
- [ ] Finish gate green
- [ ] PR opened: <!-- url -->
- [ ] Merged to `main`

**Status:** ✅ DONE (merged)
