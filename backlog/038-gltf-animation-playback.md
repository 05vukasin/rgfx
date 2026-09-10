# 038 — glTF animation playback + animation menu

- **Crate:** `rgfx-3d` (animation load/eval) · `rgfx-cli` (animation menu + loop)
- **Depends on:** 017 (glTF loader), 022 (viewer), 035 (menu pattern)
- **Branch:** `task/038-gltf-animation-playback`
- **Skill:** invoke **`rgfx-fix`** first (required; hook-enforced).

## Motivation (user request)
For an animated model, play its animation in a **loop** so you can see it move. Expose it as an
**option with its own menu**, similar to the light menu (task 035).

## Scope — rigid node-transform animation
glTF animations drive node **TRS** (translation/rotation/scale) over time. Support **rigid
node-transform animation** (objects/parts moving, rotating, scaling): evaluate node transforms at
time `t` and rebuild the scene's world-space vertex positions each frame.
**Skeletal skinning (per-vertex joint weights) is OUT OF SCOPE** for this task — detect skinned
animations and either play their node motion or show "skinned animation not supported yet" without
crashing.

### rgfx-3d
- Extend the glTF loader to also return the node hierarchy + per-mesh node assignment + the
  animations (channels: node target, path T/R/S, keyframe times + values, interpolation — support
  at least `LINEAR` and `STEP`). Keep the current baked-static path working (loaders currently bake
  world transforms; add a way to retrieve un-baked local data + hierarchy for animation).
- `SceneAnimation` type + `evaluate(anim, t) -> per-node local transforms`, and a helper that
  applies node world transforms to produce an animated `Scene` (reused framebuffer/scene buffers;
  avoid per-frame reallocation where practical). Pure, unit-tested (keyframe interpolation, loop
  wrap, node world-transform composition).
- Report animation names + durations (extend `GltfStats`).

### rgfx-cli (viewer)
- If the scene has ≥1 animation, enable an **Animation menu** (shortcut, e.g. `A`), mirroring the
  light menu (task 035) and drawn via `viewer_chrome::overlay_panel`:
  - play/pause (Space), **loop** on/off (default on — the user wants a loop), select animation
    (if several), speed `+/-`, scrub `←/→`, reset `R`, `Esc`/`A` close.
- Playback advances by real elapsed time (drive the existing frame-engine loop; the viewer becomes
  time-driven while playing — redraw each tick, not just on input). Status bar shows
  `anim:<name> t=..s` and loop state.
- No animation present → the menu reports "no animations in this file" (no crash).

## Acceptance criteria
- [ ] An animated glTF/GLB with node-transform animation visibly moves and **loops** in the viewer.
- [ ] `A` opens the animation menu; play/pause, loop toggle, speed, and scrub work; `Esc` closes;
      camera controls resume when closed.
- [ ] Static models and skinned-only animations don't crash (clear "not supported"/"no animations").
- [ ] `rgfx info` lists animation names + durations. clippy `-D warnings` clean.

## Tests required
- [ ] Keyframe evaluation: LINEAR/STEP interpolation at boundaries and midpoints; loop wraps time.
- [ ] Node world-transform composition for a small hierarchy (parent moves child).
- [ ] An embedded/`tests/assets` minimal animated glb: vertex positions differ between t=0 and
      t=mid; menu state machine (play/pause/loop/scrub) under a mock clock.

## Out of scope
Skeletal skinning, morph targets, animation blending, camera/light animation tracks.

## Completion
- [x] Implemented · [x] Gate green + PTY check · [x] PR opened · [ ] Merged

**Status:** ✅ IMPLEMENTED (PR open)

## Implementation notes
- `rgfx-3d`: added `load_gltf_animated` → `AnimatedScene` (un-baked local meshes + node hierarchy +
  per-mesh node assignment + `SceneAnimation`s). `NodeTransform`/`Interpolation` public; channels
  support `LINEAR` + `STEP` (CUBICSPLINE downgraded to linear, tangents dropped); morph-weight
  channels skipped. `evaluate(anim, t)` → per-node local transforms (pure), `animate_into(anim, t,
  &mut Scene)` composes parent→child world transforms and bakes world-space vertices into a reused
  scene buffer. `rest_scene()` matches the static `load_gltf`. `GltfStats` gained
  `animations: Vec<AnimationInfo>` (name + duration). Skins detected via `has_skinning()` (node
  motion still plays; per-vertex skinning not applied).
- `rgfx-cli`: `A` opens an Animation menu (`overlay_panel`): Space play/pause, `L` loop (default on),
  `←/→` scrub, `↑/↓` select clip, `+/-` speed, `R` reset, `Esc`/`A` close. While playing the loop is
  time-driven (wakes every ~33 ms, advances the playhead by real elapsed time, loops via
  `rem_euclid`). Status bar shows `anim:<name> t=..s loop/once`. No animations → menu says
  "no animations in this file". Static assets unaffected; animated assets skip load-time simplify
  to keep per-node mesh correspondence.
- Fixture: `crates/rgfx-3d/tests/assets/animated_triangle.gltf` (embedded buffer, one LINEAR
  translation channel, 1 s).
