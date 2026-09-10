# 037 — Free 360° rotation (arcball), no pitch clamp

- **Crate:** `rgfx-3d` (OrbitController) · `rgfx-cli` (viewer wiring)
- **Depends on:** 011 (camera/orbit), 031 (roll), 022 (viewer)
- **Branch:** `task/037-free-360-rotation`
- **Skill:** invoke **`rgfx-fix`** first (required; the hook blocks `crates/**` edits until then).

## Motivation (user report)
On a small model (skull, 4.7k faces — not a perf issue) the user can only rotate horizontally:
"I don't have a 360 rotation for all directions, I can only do it in [one] direction." Cause:
`OrbitController` uses Euler yaw/pitch with `pitch` clamped to `±(π/2 − ε)` (`MAX_PITCH`), so
the camera can spin around the equator but can never tumble over the poles.

## Design — quaternion arcball
Give the controller a **free orientation** so the model can be rotated unrestrictedly in every
direction (true trackball/arcball, no gimbal lock, no clamp):
- Represent the view orientation as a `glam::Quat` (plus `target` + `distance`, unchanged).
- `rotate(yaw_delta, pitch_delta)`: apply incremental rotations **about the camera's current
  right and up axes** (so dragging/arrowing always tumbles relative to what you see), composing
  into the quaternion — no pole clamp. Roll (task 031) composes about the view axis.
- `sync(camera)`: derive `position = target + (orientation * forward) * distance` and
  `up = orientation * up_basis`.
- `reset` restores the framed home orientation; `auto_frame`/`set_view` set it.
- Keep `zoom`/`dolly`/`pan`/`distance`/`target` behavior. Preserve the existing public methods the
  viewer calls (`orbit`, `roll`, `zoom`, `dolly`, `pan`, `reset`, `sync`, `auto_frame`,
  `from_camera`, `set_view`, `distance`); update their internals to the quaternion model. Where an
  exact `yaw()`/`pitch()` no longer makes sense, keep them deriving best-effort values or update
  the dependent tests accordingly.

The viewer (`mesh_viewer`) keeps arrows = rotate, `z`/`x` = roll, mouse-drag = rotate — now with
full freedom. No pole sticking.

## Acceptance criteria
- [ ] From any start, repeated Up (or Down) arrows tumble the model a full 360° over the top and
      back (no clamp, no flip artifacts); Left/Right still spin fully.
- [ ] Mouse drag rotates freely in all directions; roll still works; reset returns to the framed view.
- [ ] A full 2π of combined rotation returns to the start orientation (within epsilon).
- [ ] clippy `-D warnings` clean; the viewer still builds and renders (PTY smoke).

## Tests required
- [ ] Arcball: N small up-rotations summing to 2π return the camera to its start position
      (within epsilon) — i.e., no clamp and no gimbal lock at the poles.
- [ ] Rotating purely up then purely right changes the camera basis as expected (orthonormal,
      finite) past the old ±90° pitch limit.
- [ ] reset restores the framed orientation; roll composes without disturbing the tumble.

## Out of scope
Performance / large-mesh handling (task 036).

## Completion
- [x] Implemented · [x] Gate green + PTY check · [x] PR opened · [ ] Merged

**Status:** ✅ IMPLEMENTED (PR open)

### Implementation notes
`OrbitController` is now a quaternion arcball: a unit `glam::Quat` `orientation` (base frame
right `+X` / up `+Y` / view `+Z` → world) plus `target`, `distance`, and a separate `roll`
scalar. `orbit(yaw, pitch)` composes `Quat::from_rotation_y(yaw) * Quat::from_rotation_x(-pitch)`
onto the orientation in the local frame — rotation about the camera's current up/right axes, no
clamp, no gimbal lock. `roll` is applied about the current view axis in `sync`, so it tilts the
horizon without disturbing the tumble. `set_view` builds the orientation from spherical angles
(so the default 3/4 view and `reset` home still work); `from_camera` builds it from the camera's
offset/up basis. `yaw()`/`pitch()` are kept as best-effort derivations of the view direction.
Camera up is derived from the same orientation, so it is always orthogonal to the view axis and
the basis never degenerates at the poles. Viewer wiring is unchanged (arrows orbit, z/x roll,
drag orbits) — just unrestricted now.
