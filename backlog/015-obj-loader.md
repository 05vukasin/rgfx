# 015 — OBJ loader

- **Crate:** `rgfx-3d`
- **Depends on:** 013
- **Blocks:** 022, 024
- **Branch:** `task/015-obj-loader`
- **Skills:** `rgfx-architecture`, `finish-task`

## Goal
Load Wavefront OBJ meshes into the engine's `Mesh`/`Scene` types via `tobj`, so `rgfx model.obj`
becomes possible. First real file format.

## Scope / deliverables
1. `load_obj(path) -> Result<Scene>` using `tobj`: vertices, indices, normals (generate if
   absent), grouped meshes. Triangulate polygons.
2. Compute bounding box + sphere for auto-framing (011).
3. Report stats (mesh count, vertex/triangle count) for the `info` command (024).
4. Robust error handling for malformed/missing files.

## Contracts / API
Return `rgfx_core` `Scene`/`Mesh` types. Loader knows nothing about rendering or terminal.
Errors via crate `Error` (map `tobj::LoadError`).

## Acceptance criteria
- [ ] A known small OBJ (embedded or in `assets/`) loads with correct vertex/triangle counts.
- [ ] Missing normals are generated (face normals) so shading works.
- [ ] Non-triangular faces are triangulated correctly.
- [ ] Malformed OBJ returns `Err`, never panics. Clippy `-D warnings`.

## Tests required
- [ ] Load a tiny cube.obj → expected counts + bounds.
- [ ] Normal generation for a normal-less OBJ.
- [ ] Malformed input → `Err`.

## Out of scope
MTL materials/textures (later), STL/glTF (016/017).

## Completion
- [ ] Implemented
- [ ] Finish gate green
- [ ] PR opened: <!-- url -->
- [ ] Merged to `main`

**Status:** ✅ DONE (merged)
