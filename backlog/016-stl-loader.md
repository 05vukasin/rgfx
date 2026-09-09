# 016 — STL loader

- **Crate:** `rgfx-3d`
- **Depends on:** 013
- **Blocks:** 022, 024
- **Branch:** `task/016-stl-loader`
- **Skills:** `rgfx-architecture`, `finish-task`

## Goal
Load STL meshes (binary + ASCII) via `stl_io` so `rgfx part.stl` works — the 3D-printing
inspection use case.

## Scope / deliverables
1. `load_stl(path) -> Result<Scene>` handling both binary and ASCII STL.
2. STL has per-face normals only → generate/keep face normals; optionally weld vertices to
   enable smooth shading (dedup coincident vertices with a tolerance, rebuild index buffer).
3. Bounding box/sphere for auto-framing; stats for `info`.
4. Robust errors for malformed files.

## Contracts / API
Return `rgfx_core` `Scene`/`Mesh`. No rendering/terminal deps. Errors via crate `Error`.

## Acceptance criteria
- [ ] A known binary STL and a known ASCII STL both load with correct triangle counts.
- [ ] Vertex welding produces a smaller vertex set with an equivalent surface (assert counts).
- [ ] Malformed STL → `Err`, no panic. Clippy `-D warnings`.

## Tests required
- [ ] Load embedded binary + ASCII STL cubes → expected counts + bounds.
- [ ] Weld dedup test (coincident vertices merged within tolerance).
- [ ] Malformed input → `Err`.

## Out of scope
OBJ/glTF. Color (STL is colorless; base color comes from shading).

## Completion
- [ ] Implemented
- [ ] Finish gate green
- [ ] PR opened: <!-- url -->
- [ ] Merged to `main`

**Status:** ✅ DONE (merged)
