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

**Status:** 🟦 IN REVIEW (branch `task/038-anim-b`)

## Implementation notes
- `rgfx-3d`: new `anim` module — `NodeTransform`, `AnimChannel`/`ChannelSamples`, `Interpolation`
  (LINEAR/STEP), `SceneAnimation` (+`wrap_time`), `AnimatedScene` (hierarchy + un-baked
  `MeshInstance`s + `evaluate`/`bake_static`), and a buffer-reusing `SceneAnimator::pose_into`.
  The glTF loader now builds the hierarchy once and *bakes from it* for the static path (so
  `load_gltf` is unchanged), and `load_gltf_animated` exposes the un-baked form. `GltfStats` gains
  per-animation names/durations/skinned flags.
- `rgfx-cli`: `A` opens a modal Animation menu via `viewer_chrome::overlay_panel` mirroring the
  light menu — Space play/pause, `L` loop (default on), Up/Down select clip, `←/→` scrub, `+/-`
  speed, `R` reset, `Esc`/`A` close. Playing makes the viewer time-driven (≈30 fps ticks advancing
  by real elapsed time); status shows `anim:<name> t=..s ▶/❚❚ loop/once`. No animations → the menu
  reports "no animations in this file". `rgfx info` lists animation names + durations.
- **Skinning:** detected (`AnimationInfo::skinned`) and surfaced ("skinned: node motion only" /
  `[skinned]` in info) but not applied — only rigid node motion plays; nothing crashes.
- Fixture: `crates/rgfx-3d/tests/assets/animated_triangle.glb` (one 1.0s LINEAR translation clip).
