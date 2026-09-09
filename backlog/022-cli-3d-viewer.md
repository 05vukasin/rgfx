# 022 — CLI interactive 3D viewer

- **Crate:** `rgfx-cli`
- **Depends on:** 020, 011, 012, 013, 014, 015, 016, 017, 003, 004, 007 (006 optional)
- **Blocks:** 027
- **Branch:** `task/022-cli-3d-viewer`
- **Skills:** `finish-task`

## Goal
The headline feature and v0.1 milestone: `rgfx model.obj` opens an interactive terminal 3D
viewer with orbit/zoom/pan, wireframe/shading toggles, auto-framing, a status bar, and safe
responsive rendering. This is the culmination of the 3D + terminal crates.

## Scope / deliverables
1. 3D viewer implementing the dispatch trait: load a mesh (OBJ/STL/glTF via 015–017 by kind),
   auto-frame the camera (011), and run an event loop rendering via the `Rasterizer` (013/014)
   into a reused framebuffer, encoded with the frame engine (007) + chosen encoder (004).
2. Controls per spec: `←→` orbit Y, `↑↓` orbit X, `+/-` zoom, `R` reset, `W` wireframe,
   `S` cycle shading, `C` toggle color, `L` toggle lighting, `F` toggle UI, `Q`/`Esc` quit.
3. **Event-driven redraw**: render only on input/resize (static model = no busy loop). On
   resize, recompute viewport, resize framebuffer, update camera aspect, re-render.
4. Status bar (toggle with `F`): file name, triangle count, current FPS, renderer, shading,
   plus a key-help line.

## Contracts / API
CLI glue only — no rendering math here. Reuse the framebuffer + frame engine (no per-frame
alloc). Terminal cleanup guaranteed on all exit paths.

## Acceptance criteria
- [ ] `rgfx cube.obj` (bundled asset) opens, renders, responds to orbit/zoom, and quits cleanly restoring the terminal.
- [ ] Wireframe/shading/color/lighting toggles change output; reset restores initial camera.
- [ ] Resizing the terminal re-frames without artifacts or crashes.
- [ ] Renders correctly at 100 / 60 / 30 columns (responsive test). Clippy `-D warnings`.

## Tests required
- [ ] Headless render-to-framebuffer of a bundled mesh at fixed camera → deterministic snapshot
      (drive the render path without a TTY).
- [ ] Input→camera-state transitions (orbit/zoom/reset) unit-tested via the viewer's state.
- [ ] Viewport/aspect recompute on simulated resize.

## Out of scope
Mouse controls (future), animation playback, textures.

## Completion
- [ ] Implemented
- [ ] Finish gate green
- [ ] Manual check: OBJ + STL + GLB all open and orbit smoothly in a real terminal
- [ ] PR opened: <!-- url -->
- [ ] Merged to `main`

**Status:** ✅ DONE (merged)
