# 031 — 3D viewer improvements

- **Crate:** `rgfx-3d` + `rgfx-cli`
- **Depends on:** 022 (viewer), 011–017 (3D stack)
- **Branch:** `task/031-3d-viewer-improvements`

## Requests
1. **First-frame bug**: on load the model looked "poorly loaded" and only read correctly after
   moving. Root cause: the viewer opened dead-on (+Z), where a mesh projects to a flat,
   ambiguous silhouette. Fixed by opening at a default 3/4 view; verified frame-1 shows a
   legible 3D cube under a real PTY.
2. **Third rotation axis (roll)**: rotate about the line of sight.
3. **Blender `.blend` support**.
4. **Mouse orbit**: drag to rotate left/right + up/down (follow-up request).

## Changes
- **rgfx-3d `OrbitController`**: added `roll`/`roll_angle`/`set_view` and roll-aware `sync`
  (`up` rotates about the view axis); `reset` restores roll; `auto_frame` home preserves it.
- **rgfx-3d `blend`**: `load_blend`/`load_blend_with_stats`/`blender_available` — headless
  Blender export to a temp `.glb`, then the existing glTF path. Runtime dep only; actionable
  `Error::External` when `blender` is missing (`RGFX_BLENDER` override).
- **rgfx-cli media**: detect `.blend` (extension + `BLENDER` magic) → `MeshFormat::Blend`;
  route in the viewer and `rgfx info`.
- **rgfx-cli viewer**: open at 3/4 (`DEFAULT_YAW/PITCH`), `z`/`x` roll, drag-to-orbit +
  wheel-zoom (mouse capture), status/README controls updated.

## Verification
- rgfx-3d + rgfx-cli: clippy `-D warnings` clean; roll/mouse/blend-detection unit tests added.
- PTY: first frame is a legible 3/4 cube (no residual pixels), exits cleanly.
- Blender export path: detection + arg construction + missing-binary error verified; the live
  export/render was **not** exercised here (Blender not installed in this environment).

## Completion
- [x] Implemented · [x] Gate green · [ ] Merged

**Status:** 🟡 IN REVIEW
