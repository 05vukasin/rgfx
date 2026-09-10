# 035 — Fixed light + interactive light menu (3D viewer)

- **Crate:** `rgfx-cli` (viewer light state + modal menu) · `rgfx-3d` (helper; light is already world-space)
- **Depends on:** 014 (lighting), 022 (3D viewer), 033 (viewer chrome)
- **Branch:** `task/035-light-controls-and-menu`
- **Skill:** invoke **`rgfx-fix`** first (required; the PreToolUse hook blocks `crates/**` edits until then).

## Motivation (user request)
When rotating a model, the light should **stay fixed and only the object should appear to
rotate** — so different faces catch the light and you read the 3D shape. Today the rasterizer
lights in **world space** with a fixed direction, but the viewer orbits the **camera** (the
object never moves in world space), so the shading is locked to the object and the light seems
glued to it. The user also wants a **shortcut that opens a separate light menu** to change this
(make the light *not* stay fixed) and to **move the light around in 3D space**.

## Design
Add a viewer-side `LightState` and compute the **world-space** `light_direction` handed to the
rasterizer each frame from the light mode + current camera:

- **Mode `Viewer` (new default):** light fixed relative to the camera — `light_world =
  view_matrix.inverse().transform_vector3(light_view)` — so it stays put on screen and the
  object appears to rotate *under* a stationary light. This is the requested default.
- **Mode `World`:** `light_world = light_dir` directly (fixed in world; shading static as you
  orbit — the "doesn't stay fixed to my view" alternative).

The rasterizer keeps taking a world-space `light_direction` (no contract change); the viewer
sets it via `Rasterizer::set_light_direction` / `ambient` each render.

### The light menu (shortcut)
- **`L` opens/closes a modal Light Menu** overlay panel (drawn over the render; the current
  quick on/off toggle moves *into* the menu). While the menu is open, input drives the light,
  not the camera:
  - `←/→` azimuth, `↑/↓` elevation (move the light around the sphere)
  - `M` cycle mode (Viewer ⇄ World)
  - `+`/`-` ambient (clamped 0..=1) — optional: intensity
  - `O` or `Space` toggle light on/off
  - `R` reset light to defaults · `Esc` or `L` close the menu
- While the menu is **closed**, arrows orbit the camera as before.
- The menu panel lists the current mode, azimuth/elevation, ambient, and on/off, plus its key
  help — reuse `viewer_chrome` where sensible.
- Status bar (when menu closed) shows the light mode; update the help line and README controls.

### rgfx-3d helper (optional, keep small)
A pure `fn direction_from_azimuth_elevation(az, el) -> Vec3` (or in the viewer) so the menu can
move the light on a sphere deterministically. If added to rgfx-3d, export + unit-test it.

## Acceptance criteria
- [ ] Default (`Viewer` mode): orbiting shows the light staying fixed on screen while faces
      light/darken as the object turns (verify the world light_direction changes with the camera).
- [ ] `World` mode: the world light_direction is constant as the camera orbits.
- [ ] `L` opens a visible modal menu; while open, arrows move the light (not the camera); `Esc`/`L` closes; camera controls resume when closed.
- [ ] Ambient clamped to 0..=1; on/off works; reset restores defaults. clippy `-D warnings` clean.

## Tests required
- [ ] Viewer-mode world direction rotates with a known camera (differs at two orbit angles);
      World-mode direction is invariant to the camera.
- [ ] Menu state machine: open routes arrows to light (azimuth/elevation change, camera orbit
      unchanged); closed routes arrows to the camera; ambient clamp; on/off + reset.
- [ ] (If added) azimuth/elevation → direction is a unit vector with expected quadrant.

## Out of scope
Multiple lights, shadows, specular. Point-light attenuation (this is a single directional light
whose direction is what the menu controls).

## Completion
- [x] Implemented · [x] Gate green + PTY check · [ ] PR opened · [ ] Merged

**Status:** 🟦 IN REVIEW (branch `task/035-light-menu-c`)

### Implementation notes
- Pure helper `rgfx_3d::direction_from_azimuth_elevation(az, el) -> Vec3` (unit vector; +Z at
  zero, +Y at el = π/2), exported and unit-tested.
- Viewer-side `LightState` (mode Viewer/World, azimuth, elevation, ambient 0..=1, on/off,
  `menu_open`). Each render computes the world light direction: Viewer →
  `camera.view_matrix().inverse().transform_vector3(local)` (light fixed to the view, object
  rotates under it — the default); World → the direction directly. Fed to the rasterizer via
  `set_light_direction` + `ambient`; no per-frame allocation.
- `L` toggles a modal menu. Open: `←/→` azimuth, `↑/↓` elevation, `M` mode, `+/-` ambient,
  `O`/Space on/off, `R` reset, `Esc`/`L` close (Ctrl+C still quits). Closed: arrows orbit.
- Menu panel drawn via a new `viewer_chrome::overlay_panel` (box-drawn, top-left). Status bar
  shows the light mode (or `off`); help line + README updated.
