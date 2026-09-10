//! The interactive 3D mesh viewer: load → auto-frame → orbit/render loop → encode → present.
//!
//! This is the flagship [`MediaViewer`](crate::dispatch::MediaViewer) (task 022). Like the
//! still-image viewer it adds no rendering math of its own — it composes the sibling crates,
//! honouring the one architectural law:
//!
//! 1. a loader from `rgfx-3d` ([`load_obj`](rgfx_3d::load_obj) / [`load_stl`](rgfx_3d::load_stl) /
//!    [`load_gltf`](rgfx_3d::load_gltf), chosen by [`MeshFormat`]) turns the file into a
//!    [`Scene`];
//! 2. an [`OrbitController`] frames the model's bounding sphere and drives the camera;
//! 3. the [`Rasterizer`] (a [`SceneRenderer`]) renders the [`Scene`] into a reused
//!    [`Framebuffer`];
//! 4. a [`BrailleEncoder`] turns that framebuffer into a [`TerminalFrame`], presented through the
//!    diffing [`FrameEngine`] (interactive) or serialized to a file (`--output`).
//!
//! All camera and toggle state lives in a pure, TTY-free viewer state so the input → camera
//! transitions and viewport/aspect recompute are unit-testable without a terminal.

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Context;
use glam::Vec3;
use rgfx_3d::{
    AnimatedScene, OrbitController, Rasterizer, ShadingMode, direction_from_azimuth_elevation,
    simplify_scene,
};
use rgfx_core::{
    BoundingSphere, Camera, Color, Framebuffer, Scene, SceneRenderer, TerminalEncoder,
    TerminalFrame, Viewport,
};
use rgfx_terminal::{
    BrailleEncoder, BrailleOptions, ColorMode, Event, FrameEngine, KeyCode, KeyEvent, MouseButton,
    MouseEvent, MouseEventKind, SUBPIXEL_X, SUBPIXEL_Y, detect_color_mode,
};

use crate::cli::Shading;
use crate::config::Settings;
use crate::dispatch::ViewRequest;
use crate::media::{Input, MeshFormat};
use crate::terminal::{Session, is_quit};

/// Radians orbited per arrow-key press (~6.9°).
const ORBIT_STEP: f32 = 0.12;

/// Radians the view rolls per `z`/`x` press (rotation about the line of sight — the third axis).
const ROLL_STEP: f32 = 0.12;

/// Radians of orbit per terminal cell of mouse drag (drag-to-orbit sensitivity).
const MOUSE_ORBIT_STEP: f32 = 0.04;

/// The default 3/4 orbit applied after auto-framing so a model reads as 3D on load instead of a
/// flat, axis-aligned silhouette. Yaw swings to the right-front, pitch lifts slightly above.
const DEFAULT_YAW: f32 = -0.6; // ~ -34°
const DEFAULT_PITCH: f32 = 0.45; // ~ +26°
/// Distance multiplier applied by a single zoom-in (`+`) press.
const ZOOM_IN: f32 = 0.9;
/// Distance multiplier applied by a single zoom-out (`-`) press.
const ZOOM_OUT: f32 = 1.0 / ZOOM_IN;
/// The smallest bounding radius used for clip-plane math, guarding degenerate (point) meshes.
const MIN_RADIUS: f32 = 1e-4;
/// Default render width in columns for non-interactive `--output` when no `--width` is given.
const DEFAULT_OUTPUT_COLS: u16 = 80;
/// How long the interactive loop blocks waiting for input. A static model never redraws on its
/// own, so this only bounds shutdown latency; resize and key events wake the loop immediately.
const POLL_TIMEOUT: Duration = Duration::from_millis(500);

/// The filled shading modes cycled by the `S` key. Wireframe is a separate `W` toggle, and the
/// `L` key forces the lit modes to [`ShadingMode::Unlit`] rather than being part of the cycle.
const SHADING_CYCLE: [ShadingMode; 5] = [
    ShadingMode::Flat,
    ShadingMode::Smooth,
    ShadingMode::Unlit,
    ShadingMode::Normals,
    ShadingMode::Depth,
];

/// The still human-readable renderer name shown in the status bar.
const RENDERER_NAME: &str = "braille";

/// Radians the light's azimuth/elevation moves per arrow press while the light menu is open.
const LIGHT_ANGLE_STEP: f32 = 0.15;
/// Ambient light delta applied per `+`/`-` press while the light menu is open.
const AMBIENT_STEP: f32 = 0.05;
/// Elevation is clamped to just short of the poles so the light never degenerates onto the axis.
const MAX_LIGHT_ELEVATION: f32 = 1.5;
/// Default light azimuth (upper-front-right), chosen to match the rasterizer's default direction.
const DEFAULT_LIGHT_AZIMUTH: f32 = 0.46;
/// Default light elevation (upper-front-right), chosen to match the rasterizer's default direction.
const DEFAULT_LIGHT_ELEVATION: f32 = 0.56;
/// Default ambient floor, matching the rasterizer's default so lighting reads the same on load.
const DEFAULT_AMBIENT: f32 = 0.15;

/// Seconds scrubbed per `←`/`→` press while the animation menu is open.
const ANIM_SCRUB_STEP: f32 = 0.1;
/// Multiplier applied to playback speed per `+` press (and its reciprocal per `-`).
const ANIM_SPEED_STEP: f32 = 1.25;
/// Playback speed is clamped to this range so it never stalls or runs away.
const ANIM_MIN_SPEED: f32 = 0.1;
const ANIM_MAX_SPEED: f32 = 8.0;
/// Target tick interval while an animation plays: the loop wakes this often to advance time and
/// redraw (~30 fps), independent of user input.
const ANIM_TICK: Duration = Duration::from_millis(33);

/// Which frame the light's azimuth/elevation direction is interpreted in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LightMode {
    /// The direction is fixed *relative to the camera*: it is transformed by the inverse view
    /// matrix into world space each frame, so the light stays put on screen and the object appears
    /// to rotate under a stationary light. This is the default.
    Viewer,
    /// The direction is a fixed *world-space* direction: shading stays constant as the camera
    /// orbits (the light appears glued to the object).
    World,
}

impl LightMode {
    /// The other mode (used by the `M` toggle).
    fn toggled(self) -> Self {
        match self {
            LightMode::Viewer => LightMode::World,
            LightMode::World => LightMode::Viewer,
        }
    }

    /// A short lower-case name for the status bar / menu.
    fn name(self) -> &'static str {
        match self {
            LightMode::Viewer => "viewer",
            LightMode::World => "world",
        }
    }
}

/// The viewer-side directional-light model: where the light sits (azimuth/elevation), how its
/// direction is interpreted ([`LightMode`]), the ambient floor, whether it is on, and whether the
/// modal light menu is open. Pure state with no terminal dependency, so the menu's key routing and
/// the per-mode world-direction math are unit-testable headlessly.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LightState {
    /// How the azimuth/elevation direction is interpreted (viewer- vs world-fixed).
    mode: LightMode,
    /// Azimuth around the vertical axis, in radians (wraps freely).
    azimuth: f32,
    /// Elevation above the horizontal plane, in radians (clamped near the poles).
    elevation: f32,
    /// Ambient floor in `0.0..=1.0`, handed to the rasterizer.
    ambient: f32,
    /// Whether directional lighting is applied (the quick on/off, now inside the menu).
    on: bool,
    /// Whether the modal light menu is open (captures input while true).
    menu_open: bool,
}

impl LightState {
    /// The default light: viewer-fixed, upper-front-right, low ambient, on, menu closed.
    fn new() -> Self {
        Self {
            mode: LightMode::Viewer,
            azimuth: DEFAULT_LIGHT_AZIMUTH,
            elevation: DEFAULT_LIGHT_ELEVATION,
            ambient: DEFAULT_AMBIENT,
            on: true,
            menu_open: false,
        }
    }

    /// Restores the light to its defaults, leaving the menu open/closed as it was.
    fn reset(&mut self) {
        let menu_open = self.menu_open;
        *self = Self::new();
        self.menu_open = menu_open;
    }

    /// Sets the ambient level, clamped to `0.0..=1.0`.
    fn set_ambient(&mut self, ambient: f32) {
        self.ambient = ambient.clamp(0.0, 1.0);
    }

    /// The world-space direction handed to the rasterizer for the current mode and camera. In
    /// [`LightMode::Viewer`] the azimuth/elevation direction is treated as camera-space and mapped
    /// to world space by the inverse view matrix (so it stays fixed on screen); in
    /// [`LightMode::World`] it is already the world direction.
    fn world_direction(&self, camera: &Camera) -> Vec3 {
        let dir = direction_from_azimuth_elevation(self.azimuth, self.elevation);
        match self.mode {
            LightMode::Viewer => camera.view_matrix().inverse().transform_vector3(dir),
            LightMode::World => dir,
        }
    }

    /// Routes a key while the menu is open, mutating the light. Returns whether anything changed
    /// (so the caller can request a redraw). The menu open/close keys (`Esc`/`L`) are handled by
    /// [`ViewerState::on_key`] before this is reached.
    fn on_menu_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Left => self.azimuth -= LIGHT_ANGLE_STEP,
            KeyCode::Right => self.azimuth += LIGHT_ANGLE_STEP,
            KeyCode::Up => {
                self.elevation = (self.elevation + LIGHT_ANGLE_STEP)
                    .clamp(-MAX_LIGHT_ELEVATION, MAX_LIGHT_ELEVATION);
            }
            KeyCode::Down => {
                self.elevation = (self.elevation - LIGHT_ANGLE_STEP)
                    .clamp(-MAX_LIGHT_ELEVATION, MAX_LIGHT_ELEVATION);
            }
            KeyCode::Char('m') | KeyCode::Char('M') => self.mode = self.mode.toggled(),
            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.set_ambient(self.ambient + AMBIENT_STEP)
            }
            KeyCode::Char('-') | KeyCode::Char('_') => {
                self.set_ambient(self.ambient - AMBIENT_STEP)
            }
            KeyCode::Char('o') | KeyCode::Char('O') | KeyCode::Char(' ') => self.on = !self.on,
            KeyCode::Char('r') | KeyCode::Char('R') => self.reset(),
            _ => return false,
        }
        true
    }
}

/// Per-animation metadata (name + duration) the menu needs to drive playback. Kept separate from
/// the [`AnimatedScene`] geometry so the menu state machine is pure and unit-testable under a mock
/// clock, without loading a real asset.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AnimMeta {
    /// The animation's display name.
    name: String,
    /// The animation's duration in seconds (may be `0.0` for a single-keyframe pose).
    duration: f32,
}

/// The viewer-side animation-playback model: which animation is selected, the current playhead
/// time, play/pause, loop, and speed, plus whether the modal animation menu is open.
///
/// Pure state with no terminal or geometry dependency: [`advance`](Self::advance) takes a real
/// elapsed delta and [`on_menu_key`](Self::on_menu_key) routes menu keys, so playback (including
/// loop wrap) is unit-testable headlessly under a mock clock.
#[derive(Clone, Debug)]
pub(crate) struct AnimState {
    /// Metadata for each available animation (empty when the file has none).
    anims: Vec<AnimMeta>,
    /// Index of the selected animation within [`anims`](Self::anims).
    selected: usize,
    /// The current playhead time in seconds.
    time: f32,
    /// Whether playback is advancing.
    playing: bool,
    /// Whether the playhead wraps to the start at the end (default on — the user wants a loop).
    looping: bool,
    /// Playback speed multiplier (`1.0` = real time).
    speed: f32,
    /// Whether the modal animation menu is open (captures input while true).
    menu_open: bool,
}

impl AnimState {
    /// Builds the playback state for the given animations. Starts paused at `t = 0`, looping on,
    /// speed `1.0`, menu closed.
    pub(crate) fn new(anims: Vec<AnimMeta>) -> Self {
        Self {
            anims,
            selected: 0,
            time: 0.0,
            playing: false,
            looping: true,
            speed: 1.0,
            menu_open: false,
        }
    }

    /// Whether the file carries at least one animation (the `A` menu is otherwise a no-op message).
    pub(crate) fn has_animations(&self) -> bool {
        !self.anims.is_empty()
    }

    /// The selected animation's metadata, if any.
    fn current(&self) -> Option<&AnimMeta> {
        self.anims.get(self.selected)
    }

    /// The selected animation's duration, or `0.0` when there is none.
    fn duration(&self) -> f32 {
        self.current().map_or(0.0, |a| a.duration)
    }

    /// Whether playback is currently advancing (playing, with a real animation selected).
    pub(crate) fn is_playing(&self) -> bool {
        self.playing && self.has_animations()
    }

    /// Advances the playhead by `dt` seconds of real time (scaled by [`speed`](Self::speed)).
    /// Returns whether the time changed (so the caller can request a redraw). When looping, the
    /// time wraps modulo the duration; otherwise it clamps to the end and pauses there.
    pub(crate) fn advance(&mut self, dt: f32) -> bool {
        if !self.is_playing() || dt <= 0.0 {
            return false;
        }
        let duration = self.duration();
        if duration <= 0.0 {
            return false;
        }
        self.time += dt * self.speed;
        if self.looping {
            self.time = self.time.rem_euclid(duration);
        } else if self.time >= duration {
            self.time = duration;
            self.playing = false;
        }
        true
    }

    /// Routes a key while the animation menu is open, mutating playback. Returns whether anything
    /// changed. The open/close keys (`Esc`/`A`) are handled by [`ViewerState::on_key`] first.
    fn on_menu_key(&mut self, key: KeyEvent) -> bool {
        let duration = self.duration();
        match key.code {
            KeyCode::Char(' ') => self.playing = !self.playing,
            KeyCode::Char('l') | KeyCode::Char('L') => self.looping = !self.looping,
            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.speed = (self.speed * ANIM_SPEED_STEP).clamp(ANIM_MIN_SPEED, ANIM_MAX_SPEED);
            }
            KeyCode::Char('-') | KeyCode::Char('_') => {
                self.speed = (self.speed / ANIM_SPEED_STEP).clamp(ANIM_MIN_SPEED, ANIM_MAX_SPEED);
            }
            KeyCode::Left => self.scrub(-ANIM_SCRUB_STEP, duration),
            KeyCode::Right => self.scrub(ANIM_SCRUB_STEP, duration),
            KeyCode::Up => self.select_prev(),
            KeyCode::Down => self.select_next(),
            KeyCode::Char('r') | KeyCode::Char('R') => self.reset(),
            _ => return false,
        }
        true
    }

    /// Scrubs the playhead by `delta` seconds, wrapping when looping and clamping otherwise.
    fn scrub(&mut self, delta: f32, duration: f32) {
        if duration <= 0.0 {
            self.time = 0.0;
            return;
        }
        self.time = if self.looping {
            (self.time + delta).rem_euclid(duration)
        } else {
            (self.time + delta).clamp(0.0, duration)
        };
    }

    /// Selects the previous animation (wrapping), resetting the playhead to its start.
    fn select_prev(&mut self) {
        if self.anims.is_empty() {
            return;
        }
        self.selected = (self.selected + self.anims.len() - 1) % self.anims.len();
        self.time = 0.0;
    }

    /// Selects the next animation (wrapping), resetting the playhead to its start.
    fn select_next(&mut self) {
        if self.anims.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.anims.len();
        self.time = 0.0;
    }

    /// Resets the playhead to `0`, speed to `1.0`, and loop on, keeping the selection and the menu
    /// open/closed as they were.
    fn reset(&mut self) {
        self.time = 0.0;
        self.speed = 1.0;
        self.looping = true;
    }
}

/// A loaded scene together with the simplification that was (or was not) applied on load.
struct Loaded {
    /// The scene actually rendered — simplified when [`simplify_note`](Self::simplify_note) is set.
    scene: Scene,
    /// `Some((original_tris, simplified_tris))` when the mesh was decimated on load, so the status
    /// bar can surface the change; `None` when it was rendered at full detail.
    simplify_note: Option<(usize, usize)>,
    /// The un-baked animation data, when the (glTF) asset carries ≥1 animation. Playing an
    /// animation rebuilds the scene's world-space vertices each frame from this. `None` for static
    /// assets (and all non-glTF formats), which renders the baked [`scene`](Self::scene) as before.
    animation: Option<AnimatedScene>,
}

/// Runs the 3D viewer for a request. Entry point called by the dispatch layer.
pub fn view(request: &ViewRequest<'_>, format: MeshFormat) -> anyhow::Result<()> {
    let path = match request.input {
        Input::File(p) => p.as_path(),
        Input::Stdin => {
            anyhow::bail!("reading meshes from stdin is not supported (pass a file path)")
        }
    };
    let loaded = load_scene(path, format, request.settings)?;

    match &request.settings.output {
        Some(out) => write_output(&loaded, path, request.settings, out),
        None => run_interactive(&loaded, path, request.settings),
    }
}

/// Loads a [`Scene`] with the loader matching `format`, then applies mesh simplification when the
/// settings call for it (an explicit `--simplify`, or the automatic budget for a heavy mesh). The
/// original scene's triangle count is preserved in the returned [`Loaded::simplify_note`] — the
/// `rgfx info` path loads independently, so it always reports the true on-disk counts.
fn load_scene(path: &Path, format: MeshFormat, settings: &Settings) -> anyhow::Result<Loaded> {
    // For glTF, also load the un-baked animation data. If it carries animations we render from it
    // (the rest pose as the base scene) and keep it for playback.
    let mut animation = None;
    let scene =
        match format {
            MeshFormat::Obj => rgfx_3d::load_obj(path)
                .with_context(|| format!("loading OBJ {}", path.display()))?,
            MeshFormat::Stl => rgfx_3d::load_stl(path)
                .with_context(|| format!("loading STL {}", path.display()))?,
            MeshFormat::Gltf => {
                let animated = rgfx_3d::load_gltf_animated(path)
                    .with_context(|| format!("loading glTF {}", path.display()))?;
                if animated.has_animations() {
                    let scene = animated.rest_scene();
                    animation = Some(animated);
                    scene
                } else {
                    // No animation: fall back to the static baked loader (identical geometry).
                    rgfx_3d::load_gltf(path)
                        .with_context(|| format!("loading glTF {}", path.display()))?
                }
            }
            MeshFormat::Blend => rgfx_3d::load_blend(path).with_context(|| {
                format!(
                    "loading Blender file {} (via headless export)",
                    path.display()
                )
            })?,
        };

    // Simplification decimates vertices, which would desync the per-node mesh correspondence the
    // animation path relies on; leave animated assets at full detail.
    let total = scene.triangle_count();
    if animation.is_none() {
        if let Some(target) = simplify_target(total, settings) {
            if target < total {
                let simplified = simplify_scene(&scene, target);
                let new_tris = simplified.triangle_count();
                tracing::info!(
                    original = total,
                    simplified = new_tris,
                    "simplified mesh on load"
                );
                return Ok(Loaded {
                    scene: simplified,
                    simplify_note: Some((total, new_tris)),
                    animation: None,
                });
            }
        }
    }
    Ok(Loaded {
        scene,
        simplify_note: None,
        animation,
    })
}

/// Decides the triangle target for a mesh with `total` triangles under `settings`, or `None` to
/// render at full detail.
///
/// An explicit `--simplify` wins: a value in `(0, 1]` is a fraction of `total`, a value greater
/// than `1` is an absolute triangle count. With no explicit request, a mesh above the configured
/// automatic budget targets that budget. `--no-simplify` clears both (it is folded into
/// [`Settings`] as `simplify = None` and `auto_simplify = false`).
///
/// The returned target may equal or exceed `total`; the caller only simplifies when it is strictly
/// smaller, so "keep everything" requests are a no-op.
pub(crate) fn simplify_target(total: usize, settings: &Settings) -> Option<usize> {
    if let Some(v) = settings.simplify {
        if v <= 0.0 {
            return None;
        }
        let target = if v <= 1.0 {
            (total as f32 * v).round() as usize
        } else {
            v.round() as usize
        };
        return Some(target.max(1));
    }
    if settings.auto_simplify && total > settings.simplify_budget {
        return Some(settings.simplify_budget);
    }
    None
}

/// Nearest-neighbor upscales the rendered `src` framebuffer into the full-size `dst`, used by the
/// adaptive-resolution path: while interacting the scene is rasterized into a smaller `src` (fewer
/// fragments to shade) and stretched to the display size here, which is far cheaper than a
/// full-resolution rasterize of a heavy mesh. Every `dst` pixel is written, so no pre-clear is
/// needed.
fn upscale_into(src: &Framebuffer, dst: &mut Framebuffer) {
    let (sw, sh) = (src.width(), src.height());
    let (dw, dh) = (dst.width(), dst.height());
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 {
        return;
    }
    for y in 0..dh {
        let sy = (y * sh / dh).min(sh - 1);
        for x in 0..dw {
            let sx = (x * sw / dw).min(sw - 1);
            dst.set(x, y, src.get(sx, sy));
        }
    }
}

/// The bounding sphere of a scene, or an error when the scene carries no geometry.
fn scene_sphere(scene: &Scene) -> anyhow::Result<BoundingSphere> {
    Ok(scene
        .bounding_box()
        .context("mesh has no geometry to display")?
        .bounding_sphere())
}

/// A short display name for the loaded file (its final path component), used in the status bar.
fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// What a handled key press means for the render loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeyAction {
    /// Quit the viewer and restore the terminal.
    Quit,
    /// State changed; re-render on the next loop iteration.
    Redraw,
    /// Nothing actionable; keep idling without a redraw.
    Ignore,
}

/// The pure, terminal-free state of the 3D viewer: the orbit camera plus the render toggles.
///
/// Every interaction ([`ViewerState::on_key`], [`ViewerState::set_aspect`]) mutates this state
/// without touching a terminal, which is what makes the input → camera transitions and the
/// viewport/aspect recompute unit-testable headlessly.
pub(crate) struct ViewerState {
    /// The orbit controller: target + spherical coordinates around it, and the reset home.
    controls: OrbitController,
    /// The camera the controller drives and the rasterizer renders through.
    camera: Camera,
    /// The model's bounding sphere, used for framing and near/far clip math.
    sphere: BoundingSphere,
    /// Index into [`SHADING_CYCLE`] selecting the current filled shading mode.
    shading_index: usize,
    /// Whether the mesh is drawn as a wireframe (overrides the filled shading).
    wireframe: bool,
    /// Whether ANSI color output is enabled (attaches per-cell foreground colors).
    color: bool,
    /// The directional-light model (mode, azimuth/elevation, ambient, on/off) and its modal menu.
    light: LightState,
    /// The animation-playback model (selection, playhead, play/pause, loop, speed) and its modal
    /// menu. Empty (no animations) for static assets; the `A` menu then reports as much.
    anim: AnimState,
    /// Whether the status bar / key help overlay is shown.
    show_ui: bool,
    /// The last pointer cell while a left-drag is in progress, for drag-to-orbit deltas.
    last_drag: Option<(u16, u16)>,
    /// Reused reduced-resolution framebuffer for the adaptive interaction path (no per-frame
    /// allocation): the scene is rasterized into this while the user is actively orbiting/zooming,
    /// then stretched up to the full framebuffer for encoding.
    scratch: Framebuffer,
}

impl ViewerState {
    /// Builds the initial state for `sphere`, framing it in `viewport` with the settings' field of
    /// view, shading, wireframe, and color defaults.
    pub(crate) fn new(sphere: BoundingSphere, settings: &Settings, viewport: Viewport) -> Self {
        let aspect = viewport.aspect(SUBPIXEL_X, SUBPIXEL_Y).max(f32::EPSILON);
        let mut camera = Camera::perspective(aspect, settings.fov_degrees.to_radians());
        let mut controls = OrbitController::from_camera(&camera);
        controls.auto_frame(&mut camera, &sphere, aspect);
        // Start at a 3/4 view (and make it the reset home) so the model is immediately legible in
        // 3D rather than a flat, dead-on silhouette; then push it into the camera.
        controls.set_view(DEFAULT_YAW, DEFAULT_PITCH);
        controls.sync(&mut camera);

        let shading_index = cycle_index(shading_mode(settings.shading));
        let mut state = Self {
            controls,
            camera,
            sphere,
            shading_index,
            wireframe: settings.wireframe,
            color: settings.color,
            light: LightState::new(),
            anim: AnimState::new(Vec::new()),
            show_ui: true,
            last_drag: None,
            scratch: Framebuffer::new(0, 0),
        };
        state.update_clip();
        state
    }

    /// Attaches the asset's animations (name + duration each), enabling the animation menu. Called
    /// once after construction when the loaded asset carries animations.
    pub(crate) fn set_animations(&mut self, anims: Vec<AnimMeta>) {
        self.anim = AnimState::new(anims);
    }

    /// Whether playback is currently advancing (see [`AnimState::is_playing`]).
    pub(crate) fn anim_is_playing(&self) -> bool {
        self.anim.is_playing()
    }

    /// Advances the animation playhead by `dt` real seconds; returns whether the time changed.
    pub(crate) fn advance_anim(&mut self, dt: f32) -> bool {
        self.anim.advance(dt)
    }

    /// The selected animation index (for [`AnimatedScene::animate_into`]).
    pub(crate) fn anim_selected(&self) -> usize {
        self.anim.selected
    }

    /// The current playhead time in seconds.
    pub(crate) fn anim_time(&self) -> f32 {
        self.anim.time
    }

    /// The effective shading mode, resolving the wireframe and lighting toggles over the cycled
    /// filled mode.
    pub(crate) fn effective_shading(&self) -> ShadingMode {
        if self.wireframe {
            return ShadingMode::Wireframe;
        }
        let base = SHADING_CYCLE[self.shading_index];
        if !self.light.on && matches!(base, ShadingMode::Flat | ShadingMode::Smooth) {
            ShadingMode::Unlit
        } else {
            base
        }
    }

    /// Applies a key press, updating the camera or toggles and reporting what the loop should do.
    ///
    /// The modal light menu takes priority: `L` opens/closes it, and while it is open the arrows
    /// and light keys drive the light rather than the camera (Ctrl+C still quits). While it is
    /// closed the arrows orbit the camera as before.
    pub(crate) fn on_key(&mut self, key: KeyEvent) -> KeyAction {
        // Ctrl+C always quits, even out of the modal menu.
        if key.modifiers.ctrl && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C')) {
            return KeyAction::Quit;
        }

        // While the light menu is open it captures input: Esc/L close it, everything else drives
        // the light (never the camera or a plain-key quit).
        if self.light.menu_open {
            return match key.code {
                KeyCode::Esc | KeyCode::Char('l') | KeyCode::Char('L') => {
                    self.light.menu_open = false;
                    KeyAction::Redraw
                }
                _ => {
                    if self.light.on_menu_key(key) {
                        KeyAction::Redraw
                    } else {
                        KeyAction::Ignore
                    }
                }
            };
        }

        // While the animation menu is open it captures input: Esc/A close it, everything else
        // drives playback (never the camera or a plain-key quit). Ctrl+C already returned above.
        if self.anim.menu_open {
            return match key.code {
                KeyCode::Esc | KeyCode::Char('a') | KeyCode::Char('A') => {
                    self.anim.menu_open = false;
                    KeyAction::Redraw
                }
                _ => {
                    if self.anim.on_menu_key(key) {
                        KeyAction::Redraw
                    } else {
                        KeyAction::Ignore
                    }
                }
            };
        }

        if is_quit(key) {
            return KeyAction::Quit;
        }
        match key.code {
            KeyCode::Left => self.controls.orbit(-ORBIT_STEP, 0.0),
            KeyCode::Right => self.controls.orbit(ORBIT_STEP, 0.0),
            KeyCode::Up => self.controls.orbit(0.0, ORBIT_STEP),
            KeyCode::Down => self.controls.orbit(0.0, -ORBIT_STEP),
            KeyCode::Char('+') | KeyCode::Char('=') => self.controls.zoom(ZOOM_IN),
            KeyCode::Char('-') | KeyCode::Char('_') => self.controls.zoom(ZOOM_OUT),
            KeyCode::Char('z') | KeyCode::Char('Z') => self.controls.roll(-ROLL_STEP),
            KeyCode::Char('x') | KeyCode::Char('X') => self.controls.roll(ROLL_STEP),
            KeyCode::Char('r') | KeyCode::Char('R') => self.controls.reset(),
            KeyCode::Char('w') | KeyCode::Char('W') => self.wireframe = !self.wireframe,
            KeyCode::Char('s') | KeyCode::Char('S') => {
                self.shading_index = (self.shading_index + 1) % SHADING_CYCLE.len();
            }
            KeyCode::Char('c') | KeyCode::Char('C') => self.color = !self.color,
            KeyCode::Char('l') | KeyCode::Char('L') => self.light.menu_open = true,
            KeyCode::Char('a') | KeyCode::Char('A') => self.anim.menu_open = true,
            KeyCode::Char('f') | KeyCode::Char('F') => self.show_ui = !self.show_ui,
            _ => return KeyAction::Ignore,
        }
        KeyAction::Redraw
    }

    /// Applies a mouse event: left-drag orbits the model in both axes (the horizontal drag gives
    /// the left/right rotation, the vertical drag the up/down), and the wheel zooms.
    pub(crate) fn on_mouse(&mut self, m: MouseEvent) -> KeyAction {
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.last_drag = Some((m.col, m.row));
                KeyAction::Ignore
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let action = if let Some((lc, lr)) = self.last_drag {
                    let dx = m.col as f32 - lc as f32;
                    let dy = m.row as f32 - lr as f32;
                    // Drag right → orbit right (yaw+); drag up (row index decreases) → pitch up.
                    self.controls
                        .orbit(dx * MOUSE_ORBIT_STEP, -dy * MOUSE_ORBIT_STEP);
                    KeyAction::Redraw
                } else {
                    KeyAction::Ignore
                };
                self.last_drag = Some((m.col, m.row));
                action
            }
            MouseEventKind::Up(_) => {
                self.last_drag = None;
                KeyAction::Ignore
            }
            MouseEventKind::ScrollUp => {
                self.controls.zoom(ZOOM_IN);
                KeyAction::Redraw
            }
            MouseEventKind::ScrollDown => {
                self.controls.zoom(ZOOM_OUT);
                KeyAction::Redraw
            }
            _ => KeyAction::Ignore,
        }
    }

    /// Recomputes the camera aspect for a new viewport (e.g. on resize), keeping the current orbit.
    pub(crate) fn set_aspect(&mut self, viewport: Viewport) {
        self.controls.sync(&mut self.camera);
        self.camera.aspect = viewport.aspect(SUBPIXEL_X, SUBPIXEL_Y).max(f32::EPSILON);
        self.update_clip();
    }

    /// Recomputes near/far clip planes to bracket the model at the current orbit distance, so
    /// zooming never clips the model against the near plane.
    fn update_clip(&mut self) {
        let distance = self.controls.distance();
        let radius = self.sphere.radius.max(MIN_RADIUS);
        self.camera.near = (distance - radius).max(radius * 1e-3).max(MIN_RADIUS);
        self.camera.far = (distance + radius).max(self.camera.near + MIN_RADIUS);
    }

    /// Builds the rasterizer for the current toggle state, including the per-frame world-space
    /// light direction derived from the light model and the (already synced) camera.
    fn rasterizer(&self) -> Rasterizer {
        let mut ras = Rasterizer::new(self.effective_shading());
        // A soft blue-grey surface reads well both as grayscale luminance and, with `--color`, as a
        // colored fill. Unlit/normals/depth ignore or override this.
        ras.base_color = if self.color {
            Color::rgb(0.55, 0.68, 0.92)
        } else {
            Color::WHITE
        };
        ras.clear_color = Color::TRANSPARENT;
        // The rasterizer always lights in world space; the light model computes the world direction
        // for the current mode + camera so a Viewer-fixed light stays put on screen as we orbit.
        ras.set_light_direction(self.light.world_direction(&self.camera));
        ras.ambient = self.light.ambient;
        ras
    }

    /// Renders the scene at full resolution into `fb` (reused) and encodes it into a
    /// [`TerminalFrame`], optionally overlaying the status bar.
    fn render(
        &mut self,
        scene: &Scene,
        viewport: Viewport,
        fb: &mut Framebuffer,
        encoder_color: ColorMode,
        status: Option<&StatusInfo<'_>>,
    ) -> anyhow::Result<TerminalFrame> {
        self.render_scaled(scene, viewport, fb, encoder_color, status, 1)
    }

    /// Renders and encodes the scene at a `downscale` factor (`1` = full resolution). For
    /// `downscale > 1` the scene is rasterized into a framebuffer reduced by that factor on each
    /// axis — roughly `downscale²` fewer fragments to shade — then nearest-neighbor stretched up to
    /// the full framebuffer before encoding. The displayed size (cell grid) is unchanged; only the
    /// internal render resolution drops, which keeps interaction responsive on heavy meshes.
    fn render_scaled(
        &mut self,
        scene: &Scene,
        viewport: Viewport,
        fb: &mut Framebuffer,
        encoder_color: ColorMode,
        status: Option<&StatusInfo<'_>>,
        downscale: u32,
    ) -> anyhow::Result<TerminalFrame> {
        let (pw, ph) = viewport.render_size(SUBPIXEL_X, SUBPIXEL_Y);
        fb.resize(pw, ph);
        self.set_aspect(viewport);

        // Copy the camera before borrowing `self` mutably for the scratch buffer; the rasterizer is
        // built from copies of the current toggle/light state.
        let camera = self.camera;
        let mut ras = self.rasterizer();
        if downscale <= 1 {
            ras.render(scene, &camera, fb).context("rendering mesh")?;
        } else {
            let lw = (pw / downscale as usize).max(1);
            let lh = (ph / downscale as usize).max(1);
            self.scratch.resize(lw, lh);
            ras.render(scene, &camera, &mut self.scratch)
                .context("rendering mesh")?;
            upscale_into(&self.scratch, fb);
        }

        let color = if self.color {
            encoder_color
        } else {
            ColorMode::None
        };
        let encoder = BrailleEncoder::with_options(BrailleOptions {
            color,
            ..BrailleOptions::default()
        });
        let mut frame = encoder.encode(fb, viewport);

        if self.show_ui {
            if let Some(info) = status {
                self.overlay_status(&mut frame, info);
            }
        }
        // The modal menus draw over everything (even with the status bar hidden) since the user
        // explicitly opened them.
        if self.light.menu_open {
            self.overlay_light_menu(&mut frame);
        }
        if self.anim.menu_open {
            self.overlay_anim_menu(&mut frame);
        }
        Ok(frame)
    }

    /// Draws the two-line status bar (info + key help) across the bottom rows of `frame`.
    fn overlay_status(&self, frame: &mut TerminalFrame, info: &StatusInfo<'_>) {
        let light = if self.light.on {
            format!("light:{}", self.light.mode.name())
        } else {
            "light:off".to_string()
        };
        // Surface any load-time simplification so the triangle change is never silent.
        let tris = match info.simplify {
            Some((orig, new)) => format!("{orig}\u{2192}{new} tris (simplified)"),
            None => format!("{} tris", info.triangles),
        };
        // When the asset has animations, surface the playhead/loop state on the status line.
        let anim = if self.anim.has_animations() {
            let a = &self.anim;
            let name = a.current().map(|m| m.name.as_str()).unwrap_or("");
            format!(
                " | anim:{name} t={:.1}s {}{}",
                a.time,
                if a.looping { "loop" } else { "once" },
                if a.playing { " play" } else { " pause" },
            )
        } else {
            String::new()
        };
        let status = format!(
            "{} | {} | {:.1} fps | {} | {}{} | {}{}",
            info.file,
            tris,
            info.fps,
            RENDERER_NAME,
            shading_name(self.effective_shading()),
            if self.color { " | color" } else { "" },
            light,
            anim,
        );
        let help = if self.anim.has_animations() {
            "arrows:orbit  +/-:zoom  R:reset  W:wire  S:shade  C:color  L:light  A:anim  F:ui  Q:quit"
        } else {
            "arrows:orbit  z/x:roll  +/-:zoom  R:reset  W:wire  S:shade  C:color  L:light-menu  F:ui  Q:quit"
        };
        crate::viewer_chrome::overlay_bottom_bar(frame, &status, help);
    }

    /// Draws the modal animation-menu panel (selection, playhead, play/pause, loop, speed, and key
    /// help) floating near the top-left of `frame`. When the asset has no animations it reports
    /// exactly that instead.
    fn overlay_anim_menu(&self, frame: &mut TerminalFrame) {
        let a = &self.anim;
        let lines = if let Some(cur) = a.current() {
            let idx = format!("{}/{}", a.selected + 1, a.anims.len());
            vec![
                "Animation menu".to_string(),
                format!("clip:   {} ({idx})", cur.name),
                format!("time:   {:.2}s / {:.2}s", a.time, cur.duration),
                format!("state:  {}", if a.playing { "playing" } else { "paused" }),
                format!("loop:   {}", if a.looping { "on" } else { "off" }),
                format!("speed:  {:.2}x", a.speed),
                String::new(),
                "Space play/pause   L loop".to_string(),
                "\u{2190}/\u{2192} scrub   \u{2191}/\u{2193} clip".to_string(),
                "+/- speed   R reset   Esc/A close".to_string(),
            ]
        } else {
            vec![
                "Animation menu".to_string(),
                "no animations in this file".to_string(),
                String::new(),
                "Esc/A close".to_string(),
            ]
        };
        crate::viewer_chrome::overlay_panel(frame, 1, 1, &lines);
    }

    /// Draws the modal light-menu panel (mode, azimuth/elevation, ambient, on/off, and key help)
    /// floating near the top-left of `frame`.
    fn overlay_light_menu(&self, frame: &mut TerminalFrame) {
        let l = &self.light;
        // Azimuth wraps, so normalize it to 0..360 for display; elevation stays signed.
        let az_deg = l.azimuth.to_degrees().rem_euclid(360.0).round() as i32;
        let el_deg = l.elevation.to_degrees().round() as i32;
        let lines = vec![
            "Light menu".to_string(),
            format!("mode:      {}", l.mode.name()),
            format!("azimuth:   {az_deg}\u{b0}"),
            format!("elevation: {el_deg}\u{b0}"),
            format!("ambient:   {:.2}", l.ambient),
            format!("light:     {}", if l.on { "on" } else { "off" }),
            String::new(),
            "\u{2190}/\u{2192} az   \u{2191}/\u{2193} el   M mode".to_string(),
            "+/- ambient   O/Space on/off".to_string(),
            "R reset   Esc/L close".to_string(),
        ];
        crate::viewer_chrome::overlay_panel(frame, 1, 1, &lines);
    }
}

/// The dynamic status-bar inputs that are not part of [`ViewerState`].
struct StatusInfo<'a> {
    /// The displayed file name.
    file: &'a str,
    /// The rendered scene's triangle count (after any simplification).
    triangles: usize,
    /// The most recent measured frames-per-second.
    fps: f32,
    /// `Some((original, simplified))` triangle counts when the mesh was decimated on load.
    simplify: Option<(usize, usize)>,
}

// The bottom-bar row rendering lives in `crate::viewer_chrome` (shared with the image and
// playback viewers); only the 3D-specific status/help text composition remains above.

/// The index of `mode` within [`SHADING_CYCLE`], or `0` when it is not a cycled mode.
fn cycle_index(mode: ShadingMode) -> usize {
    SHADING_CYCLE.iter().position(|&m| m == mode).unwrap_or(0)
}

/// Maps the CLI [`Shading`] enum onto the rasterizer's [`ShadingMode`].
fn shading_mode(shading: Shading) -> ShadingMode {
    match shading {
        Shading::Unlit => ShadingMode::Unlit,
        Shading::Flat => ShadingMode::Flat,
        Shading::Smooth => ShadingMode::Smooth,
        Shading::Normals => ShadingMode::Normals,
        Shading::Depth => ShadingMode::Depth,
    }
}

/// A short lower-case name for a shading mode, for the status bar.
fn shading_name(mode: ShadingMode) -> &'static str {
    match mode {
        ShadingMode::Unlit => "unlit",
        ShadingMode::Flat => "flat",
        ShadingMode::Smooth => "smooth",
        ShadingMode::Normals => "normals",
        ShadingMode::Depth => "depth",
        ShadingMode::Wireframe => "wireframe",
        _ => "shaded",
    }
}

/// The non-interactive `--output` viewport: a fixed width (from `--width` or the default) with a
/// height chosen so the braille render field is roughly square.
fn output_viewport(settings: &Settings) -> Viewport {
    let cols = settings
        .width
        .unwrap_or(DEFAULT_OUTPUT_COLS as u32)
        .clamp(1, u16::MAX as u32) as u16;
    // cols*2 px wide, rows*4 px tall; rows = cols/2 makes the pixel field ~square.
    let rows = (cols / 2).max(1);
    Viewport::new(cols, rows)
}

/// Renders one frame of the scene to `out` as text (`--output`). Non-interactive: no terminal is
/// entered, and no status overlay is drawn. Always full-resolution (adaptive scaling is an
/// interaction-only optimization).
fn write_output(
    loaded: &Loaded,
    path: &Path,
    settings: &Settings,
    out: &Path,
) -> anyhow::Result<()> {
    let scene = &loaded.scene;
    let sphere = scene_sphere(scene)?;
    let viewport = output_viewport(settings);
    let mut state = ViewerState::new(sphere, settings, viewport);
    let mut fb = Framebuffer::new(0, 0);
    let color = if settings.color {
        ColorMode::TrueColor
    } else {
        ColorMode::None
    };
    let frame = state.render(scene, viewport, &mut fb, color, None)?;
    std::fs::write(out, frame.to_text())
        .with_context(|| format!("writing output to {}", out.display()))?;
    if let Some((orig, new)) = loaded.simplify_note {
        tracing::info!(
            original = orig,
            simplified = new,
            "rendered simplified mesh"
        );
    }
    tracing::info!(path = %out.display(), file = %display_name(path), "wrote rendered mesh");
    Ok(())
}

/// Downscale factor applied to the render resolution while the user is actively interacting. A
/// factor of 2 quarters the fragment count, keeping orbit/zoom responsive on heavy meshes; the
/// full-resolution frame is restored once input goes idle.
const INTERACT_DOWNSCALE: u32 = 2;

/// How long the loop waits for input while interacting. When this elapses with no further input the
/// view is treated as idle and re-rendered at full resolution.
const INTERACT_IDLE: Duration = Duration::from_millis(120);

/// Runs the interactive viewer: auto-frame, then an event-driven orbit/render loop that redraws
/// only on input or resize. While the user is actively orbiting/zooming the scene renders at a
/// reduced resolution for responsiveness, snapping back to full resolution once input goes idle.
/// The [`Session`] restores the terminal on every exit path.
fn run_interactive(loaded: &Loaded, path: &Path, settings: &Settings) -> anyhow::Result<()> {
    let scene = &loaded.scene;
    let sphere = scene_sphere(scene)?;
    let file = display_name(path);
    let triangles = scene.triangle_count();
    let simplify = loaded.simplify_note;

    let mut session = Session::open_with_mouse()?;
    let mut engine = FrameEngine::new(detect_color_mode());
    // One framebuffer, reused across every re-render (no per-frame allocation).
    let mut fb = Framebuffer::new(0, 0);

    let mut viewport = session.viewport()?;
    let mut state = ViewerState::new(sphere, settings, viewport);
    // Enable the animation menu when the asset carries animations.
    let animated = loaded.animation.as_ref();
    if let Some(a) = animated {
        state.set_animations(
            a.animations()
                .iter()
                .map(|anim| AnimMeta {
                    name: anim.name().to_string(),
                    duration: anim.duration(),
                })
                .collect(),
        );
        if a.has_skinning() {
            tracing::info!(
                "glTF asset declares skins; playing node motion only (skinning not applied)"
            );
        }
    }
    // Reused per-frame scene buffer: the posed (world-space) geometry at the current playhead is
    // baked into this each frame while animating, avoiding per-frame allocation.
    let mut posed = Scene::default();
    let mut dirty = true;
    // While `interacting`, render at reduced resolution and poll with a short idle timeout so the
    // loop wakes to upgrade the frame to full resolution shortly after input stops.
    let mut interacting = false;
    let mut fps = 0.0f32;
    // Baseline for the time-driven animation tick; reset whenever playback is not advancing so
    // resuming never jumps.
    let mut last_tick = Instant::now();

    loop {
        // Advance the animation playhead by real elapsed time while playing, then redraw.
        if state.anim_is_playing() {
            let now = Instant::now();
            let dt = now.duration_since(last_tick).as_secs_f32();
            last_tick = now;
            if state.advance_anim(dt) {
                dirty = true;
            }
        } else {
            last_tick = Instant::now();
        }

        if dirty {
            let started = Instant::now();
            let info = StatusInfo {
                file: &file,
                triangles,
                fps,
                simplify,
            };
            // When the asset is animated, pose its geometry at the current playhead into the reused
            // buffer; otherwise render the static scene directly.
            let render_scene: &Scene = if let Some(a) = animated {
                a.animate_into(state.anim_selected(), state.anim_time(), &mut posed);
                &posed
            } else {
                scene
            };
            let downscale = if interacting { INTERACT_DOWNSCALE } else { 1 };
            let frame = state.render_scaled(
                render_scene,
                viewport,
                &mut fb,
                engine.mode(),
                Some(&info),
                downscale,
            )?;
            session.render_frame(&mut engine, &frame)?;
            let elapsed = started.elapsed().as_secs_f32();
            if elapsed > 0.0 {
                fps = 1.0 / elapsed;
            }
            dirty = false;
        }

        // While playing, wake on a fixed tick to advance time; otherwise poll quickly when
        // interacting (to restore full resolution) or slowly when fully idle.
        let timeout = if state.anim_is_playing() {
            ANIM_TICK
        } else if interacting {
            INTERACT_IDLE
        } else {
            POLL_TIMEOUT
        };
        match session.poll_event(timeout)? {
            Some(Event::Resize(cols, rows)) => {
                viewport = Viewport::new(cols, rows);
                state.set_aspect(viewport);
                interacting = true;
                dirty = true;
            }
            Some(Event::Key(key)) => match state.on_key(key) {
                KeyAction::Quit => break,
                KeyAction::Redraw => {
                    interacting = true;
                    dirty = true;
                }
                KeyAction::Ignore => {}
            },
            Some(Event::Mouse(m)) => match state.on_mouse(m) {
                KeyAction::Quit => break,
                KeyAction::Redraw => {
                    interacting = true;
                    dirty = true;
                }
                KeyAction::Ignore => {}
            },
            // Idle tick after interaction: upgrade to a full-resolution frame.
            None if interacting => {
                interacting = false;
                dirty = true;
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::RenderOpts;
    use crate::config::Config;
    use rgfx_core::{Mesh, Scene};
    use rgfx_terminal::{KeyModifiers, braille_char};

    /// The built-in cube as a single-mesh scene — a deterministic, always-available model.
    fn cube_scene() -> Scene {
        Scene::new("cube", vec![rgfx_3d::primitives::cube(1.0)])
    }

    fn settings(opts: RenderOpts) -> Settings {
        Settings::resolve(&Config::default(), &opts)
    }

    fn state(viewport: Viewport) -> ViewerState {
        let scene = cube_scene();
        let sphere = scene_sphere(&scene).unwrap();
        ViewerState::new(sphere, &settings(RenderOpts::default()), viewport)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
        }
    }

    /// Any non-blank cell (a lit braille glyph or overlaid text): proof the model was drawn.
    fn has_visible_content(frame: &TerminalFrame) -> bool {
        frame
            .to_text()
            .chars()
            .any(|c| c != braille_char(0) && c != '\n' && c != ' ')
    }

    #[test]
    fn renders_cube_to_a_deterministic_nonblank_frame() {
        let scene = cube_scene();
        let viewport = Viewport::new(40, 20);
        let mut s = state(viewport);
        let mut fb = Framebuffer::new(0, 0);

        let a = s
            .render(&scene, viewport, &mut fb, ColorMode::None, None)
            .unwrap();
        let b = s
            .render(&scene, viewport, &mut fb, ColorMode::None, None)
            .unwrap();

        // Deterministic: identical camera + scene → byte-identical output.
        assert_eq!(a.to_text(), b.to_text());
        assert_eq!((a.cols(), a.rows()), (40, 20));
        assert!(has_visible_content(&a), "the cube silhouette must be drawn");
    }

    #[test]
    fn renders_at_responsive_widths() {
        let scene = cube_scene();
        for cols in [100u16, 60, 30] {
            let viewport = Viewport::new(cols, 24);
            let mut s = state(viewport);
            let mut fb = Framebuffer::new(0, 0);
            let frame = s
                .render(&scene, viewport, &mut fb, ColorMode::None, None)
                .unwrap();
            assert_eq!(frame.cols(), cols as usize);
            assert!(
                has_visible_content(&frame),
                "cube must render at {cols} cols"
            );
        }
    }

    #[test]
    fn arrow_keys_orbit_the_camera() {
        let mut s = state(Viewport::new(40, 20));
        let o0 = s.controls.orientation();
        let p0 = s.controls.position();

        // Right tumbles the view; the opposite arrow exactly cancels it (arcball composition).
        assert_eq!(s.on_key(key(KeyCode::Right)), KeyAction::Redraw);
        assert_ne!(s.controls.orientation(), o0, "right orbits the camera");
        assert_eq!(s.on_key(key(KeyCode::Left)), KeyAction::Redraw);
        assert!(
            (s.controls.position() - p0).length() < 1e-5,
            "left cancels right"
        );

        let p1 = s.controls.position();
        s.on_key(key(KeyCode::Up));
        assert!(
            (s.controls.position() - p1).length() > 1e-6,
            "up tumbles the camera"
        );
        s.on_key(key(KeyCode::Down));
        assert!(
            (s.controls.position() - p1).length() < 1e-5,
            "down cancels up"
        );
    }

    #[test]
    fn zoom_and_reset_transitions() {
        let mut s = state(Viewport::new(40, 20));
        let home = s.controls.distance();

        s.on_key(key(KeyCode::Char('+')));
        assert!((s.controls.distance() - home * ZOOM_IN).abs() < 1e-5);
        s.on_key(key(KeyCode::Char('-')));
        assert!((s.controls.distance() - home * ZOOM_IN * ZOOM_OUT).abs() < 1e-5);

        // Orbit + zoom away, then reset restores the framed home distance and orientation.
        s.on_key(key(KeyCode::Right));
        s.on_key(key(KeyCode::Char('+')));
        assert_eq!(s.on_key(key(KeyCode::Char('r'))), KeyAction::Redraw);
        assert!((s.controls.distance() - home).abs() < 1e-5);
    }

    #[test]
    fn toggles_change_shading_color_lighting_and_ui() {
        let mut s = state(Viewport::new(40, 20));

        // Wireframe overrides the filled mode.
        assert!(!s.wireframe);
        s.on_key(key(KeyCode::Char('w')));
        assert!(s.wireframe);
        assert_eq!(s.effective_shading(), ShadingMode::Wireframe);
        s.on_key(key(KeyCode::Char('w')));
        assert!(!s.wireframe);

        // Shading cycles through the filled modes.
        let before = s.effective_shading();
        s.on_key(key(KeyCode::Char('s')));
        assert_ne!(s.effective_shading(), before);

        // Lighting off forces lit modes to Unlit. Lighting on/off now lives in the light menu:
        // open it with `L`, toggle the light off with `O`, close with `L`.
        s.shading_index = cycle_index(ShadingMode::Flat);
        assert_eq!(s.effective_shading(), ShadingMode::Flat);
        s.on_key(key(KeyCode::Char('l')));
        assert!(s.light.menu_open);
        s.on_key(key(KeyCode::Char('o')));
        assert!(!s.light.on);
        assert_eq!(s.effective_shading(), ShadingMode::Unlit);
        s.on_key(key(KeyCode::Char('l')));
        assert!(!s.light.menu_open);

        // Color and UI toggle.
        assert!(!s.color);
        s.on_key(key(KeyCode::Char('c')));
        assert!(s.color);
        assert!(s.show_ui);
        s.on_key(key(KeyCode::Char('f')));
        assert!(!s.show_ui);
    }

    #[test]
    fn wireframe_and_flat_produce_different_output() {
        let scene = cube_scene();
        let viewport = Viewport::new(48, 24);
        let mut fb = Framebuffer::new(0, 0);

        let mut flat = state(viewport);
        flat.shading_index = cycle_index(ShadingMode::Flat);
        let flat_text = flat
            .render(&scene, viewport, &mut fb, ColorMode::None, None)
            .unwrap()
            .to_text();

        let mut wire = state(viewport);
        wire.wireframe = true;
        let wire_text = wire
            .render(&scene, viewport, &mut fb, ColorMode::None, None)
            .unwrap()
            .to_text();

        assert_ne!(flat_text, wire_text, "wireframe must differ from filled");
    }

    #[test]
    fn color_mode_attaches_cell_foregrounds() {
        let scene = cube_scene();
        let viewport = Viewport::new(40, 20);
        let mut s = state(viewport);
        s.color = true;
        s.shading_index = cycle_index(ShadingMode::Unlit);
        let mut fb = Framebuffer::new(0, 0);

        let frame = s
            .render(&scene, viewport, &mut fb, ColorMode::TrueColor, None)
            .unwrap();
        assert!(
            frame.cells().iter().any(|c| c.fg.is_some()),
            "color mode should attach at least one foreground color"
        );
    }

    #[test]
    fn resize_recomputes_camera_aspect_and_framebuffer() {
        let scene = cube_scene();
        let mut s = state(Viewport::new(80, 24));
        let mut fb = Framebuffer::new(0, 0);

        let viewport = Viewport::new(60, 20);
        s.render(&scene, viewport, &mut fb, ColorMode::None, None)
            .unwrap();

        // Framebuffer resized to the braille pixel size of the new viewport.
        assert_eq!(fb.width(), 60 * SUBPIXEL_X as usize);
        assert_eq!(fb.height(), 20 * SUBPIXEL_Y as usize);
        // Camera aspect matches the new viewport's pixel aspect.
        let expect = viewport.aspect(SUBPIXEL_X, SUBPIXEL_Y);
        assert!((s.camera.aspect - expect).abs() < 1e-6);
    }

    #[test]
    fn quit_keys_report_quit() {
        let mut s = state(Viewport::new(40, 20));
        assert_eq!(s.on_key(key(KeyCode::Char('q'))), KeyAction::Quit);
        assert_eq!(s.on_key(key(KeyCode::Esc)), KeyAction::Quit);
        // Ctrl+C quits, but a plain 'c' is the color toggle, not a quit.
        assert_eq!(
            s.on_key(KeyEvent {
                code: KeyCode::Char('c'),
                modifiers: KeyModifiers {
                    ctrl: true,
                    ..KeyModifiers::NONE
                },
            }),
            KeyAction::Quit
        );
        assert_eq!(s.on_key(key(KeyCode::Char('c'))), KeyAction::Redraw);
    }

    #[test]
    fn empty_scene_is_a_clean_error() {
        let empty = Scene::new("empty", vec![Mesh::default()]);
        assert!(scene_sphere(&empty).is_err());
    }

    #[test]
    fn output_viewport_uses_width_and_squareish_rows() {
        let s = settings(RenderOpts {
            width: Some(120),
            ..RenderOpts::default()
        });
        assert_eq!(output_viewport(&s), Viewport::new(120, 60));
        let d = settings(RenderOpts::default());
        assert_eq!(output_viewport(&d), Viewport::new(80, 40));
    }

    #[test]
    fn default_view_is_a_three_quarter_angle_not_dead_on() {
        // Regression (task 031): the model must load at a 3/4 view so it reads as 3D, rather than
        // the flat, dead-on (+Z) silhouette that looked like a "poorly loaded" blob.
        let s = state(Viewport::new(80, 24));
        let dir = s.controls.direction();
        // A 3/4 view tilts off both axes (nonzero x and y), unlike a flat dead-on +Z silhouette.
        assert!(
            dir.x.abs() > 1e-3 && dir.y.abs() > 1e-3,
            "default view must be an off-axis 3/4 angle"
        );
        // And it matches the configured default yaw/pitch.
        let mut reference = OrbitController::new(Vec3::ZERO, 1.0);
        reference.set_view(DEFAULT_YAW, DEFAULT_PITCH);
        assert!((dir - reference.direction()).length() < 1e-5);
    }

    #[test]
    fn roll_keys_change_roll_and_request_redraw() {
        let mut s = state(Viewport::new(80, 24));
        assert_eq!(s.controls.roll_angle(), 0.0);
        assert_eq!(s.on_key(key(KeyCode::Char('x'))), KeyAction::Redraw);
        assert!(s.controls.roll_angle() > 0.0, "x rolls one way");
        assert_eq!(s.on_key(key(KeyCode::Char('z'))), KeyAction::Redraw);
        assert!(s.controls.roll_angle().abs() < 1e-6, "z rolls back");
    }

    #[test]
    fn mouse_left_drag_orbits_both_axes() {
        let mut s = state(Viewport::new(80, 24));
        let before = s.camera.position;
        let mouse = |kind, col, row| MouseEvent {
            kind,
            col,
            row,
            modifiers: KeyModifiers::NONE,
        };
        // Press, then drag right and up: yaw and pitch should both change (position moves).
        assert_eq!(
            s.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 40, 12)),
            KeyAction::Ignore
        );
        assert_eq!(
            s.on_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 55, 4)),
            KeyAction::Redraw
        );
        s.set_aspect(Viewport::new(80, 24)); // sync controls -> camera
        assert!(
            (s.camera.position - before).length() > 1e-3,
            "drag must reorient the camera"
        );
        // Releasing ends the drag.
        assert_eq!(
            s.on_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 55, 4)),
            KeyAction::Ignore
        );
        assert!(s.last_drag.is_none());
    }

    #[test]
    fn mouse_wheel_zooms() {
        let mut s = state(Viewport::new(80, 24));
        let d0 = s.controls.distance();
        let m = MouseEvent {
            kind: MouseEventKind::ScrollUp,
            col: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(s.on_mouse(m), KeyAction::Redraw);
        assert!(s.controls.distance() < d0, "scroll up zooms in");
    }

    // --- Light model + modal menu -----------------------------------------------------------------

    /// A camera at a known orbit position (looking at the origin, +Y up).
    fn camera_at(position: Vec3) -> Camera {
        let mut cam = Camera::perspective(1.0, 60_f32.to_radians());
        cam.position = position;
        cam.target = Vec3::ZERO;
        cam
    }

    #[test]
    fn viewer_mode_world_direction_rotates_with_the_camera() {
        // The requested default: a viewer-fixed light yields a *different* world direction at two
        // orbit angles, so the object appears to rotate under a stationary light.
        let light = LightState::new();
        assert_eq!(light.mode, LightMode::Viewer);
        let front = light.world_direction(&camera_at(Vec3::new(0.0, 0.0, 3.0)));
        let side = light.world_direction(&camera_at(Vec3::new(3.0, 0.0, 0.0)));
        assert!(
            (front - side).length() > 1e-2,
            "viewer-fixed light must rotate with the camera"
        );
        assert!((front.length() - 1.0).abs() < 1e-5, "direction stays unit");
    }

    #[test]
    fn world_mode_direction_is_invariant_to_the_camera() {
        let mut light = LightState::new();
        light.mode = LightMode::World;
        let a = light.world_direction(&camera_at(Vec3::new(0.0, 0.0, 3.0)));
        let b = light.world_direction(&camera_at(Vec3::new(3.0, 1.0, -2.0)));
        assert!(
            (a - b).length() < 1e-6,
            "world-fixed light must not depend on the camera"
        );
    }

    #[test]
    fn menu_routes_arrows_to_light_when_open_and_camera_when_closed() {
        let mut s = state(Viewport::new(40, 20));

        // Closed: arrows orbit the camera; the light angles are untouched.
        let orient0 = s.controls.orientation();
        let az0 = s.light.azimuth;
        assert_eq!(s.on_key(key(KeyCode::Right)), KeyAction::Redraw);
        assert_ne!(
            s.controls.orientation(),
            orient0,
            "closed menu orbits camera"
        );
        assert_eq!(s.light.azimuth, az0, "closed menu must not move the light");

        // `L` opens the modal menu.
        assert_eq!(s.on_key(key(KeyCode::Char('l'))), KeyAction::Redraw);
        assert!(s.light.menu_open);

        // Open: arrows move the light; the camera orbit is frozen.
        let orient_frozen = s.controls.orientation();
        assert_eq!(s.on_key(key(KeyCode::Right)), KeyAction::Redraw);
        assert!((s.light.azimuth - (az0 + LIGHT_ANGLE_STEP)).abs() < 1e-6);
        assert_eq!(
            s.controls.orientation(),
            orient_frozen,
            "camera must not orbit while the menu is open"
        );
        let el0 = s.light.elevation;
        s.on_key(key(KeyCode::Up));
        assert!((s.light.elevation - (el0 + LIGHT_ANGLE_STEP)).abs() < 1e-6);

        // `Esc` closes; arrows orbit the camera again.
        assert_eq!(s.on_key(key(KeyCode::Esc)), KeyAction::Redraw);
        assert!(!s.light.menu_open);
        let orient_resumed = s.controls.orientation();
        s.on_key(key(KeyCode::Left));
        assert_ne!(
            s.controls.orientation(),
            orient_resumed,
            "camera orbits again after the menu closes"
        );
    }

    #[test]
    fn menu_mode_toggle_ambient_clamp_onoff_and_reset() {
        let mut s = state(Viewport::new(40, 20));
        s.on_key(key(KeyCode::Char('l'))); // open

        // `M` cycles Viewer <-> World.
        assert_eq!(s.light.mode, LightMode::Viewer);
        s.on_key(key(KeyCode::Char('m')));
        assert_eq!(s.light.mode, LightMode::World);
        s.on_key(key(KeyCode::Char('m')));
        assert_eq!(s.light.mode, LightMode::Viewer);

        // Ambient is clamped to 0..=1 at both ends.
        for _ in 0..50 {
            s.on_key(key(KeyCode::Char('+')));
        }
        assert!((s.light.ambient - 1.0).abs() < 1e-6, "ambient clamps at 1");
        for _ in 0..50 {
            s.on_key(key(KeyCode::Char('-')));
        }
        assert!(s.light.ambient.abs() < 1e-6, "ambient clamps at 0");

        // On/off via `O` and `Space`.
        assert!(s.light.on);
        s.on_key(key(KeyCode::Char('o')));
        assert!(!s.light.on);
        s.on_key(key(KeyCode::Char(' ')));
        assert!(s.light.on);

        // Move the light, then `R` restores defaults while keeping the menu open.
        s.on_key(key(KeyCode::Right));
        s.on_key(key(KeyCode::Char('m')));
        s.on_key(key(KeyCode::Char('+')));
        s.on_key(key(KeyCode::Char('r')));
        assert_eq!(s.light.mode, LightMode::Viewer);
        assert!((s.light.azimuth - DEFAULT_LIGHT_AZIMUTH).abs() < 1e-6);
        assert!((s.light.elevation - DEFAULT_LIGHT_ELEVATION).abs() < 1e-6);
        assert!((s.light.ambient - DEFAULT_AMBIENT).abs() < 1e-6);
        assert!(s.light.on);
        assert!(s.light.menu_open, "reset keeps the menu open");
    }

    #[test]
    fn ctrl_c_quits_even_with_the_menu_open() {
        let mut s = state(Viewport::new(40, 20));
        s.on_key(key(KeyCode::Char('l')));
        assert!(s.light.menu_open);
        assert_eq!(
            s.on_key(KeyEvent {
                code: KeyCode::Char('c'),
                modifiers: KeyModifiers {
                    ctrl: true,
                    ..KeyModifiers::NONE
                },
            }),
            KeyAction::Quit
        );
    }

    #[test]
    fn viewer_light_changes_shading_as_the_object_orbits() {
        // End-to-end: a lit render differs after orbiting, because the viewer-fixed light catches
        // different faces as the object turns beneath it.
        let scene = cube_scene();
        let viewport = Viewport::new(48, 24);
        let mut fb = Framebuffer::new(0, 0);
        let mut s = state(viewport);
        s.shading_index = cycle_index(ShadingMode::Flat);

        let before = s
            .render(&scene, viewport, &mut fb, ColorMode::None, None)
            .unwrap()
            .to_text();
        for _ in 0..6 {
            s.on_key(key(KeyCode::Right));
        }
        let after = s
            .render(&scene, viewport, &mut fb, ColorMode::None, None)
            .unwrap()
            .to_text();
        assert_ne!(before, after, "orbiting must change the lit render");
    }

    #[test]
    fn open_menu_draws_a_panel_over_the_render() {
        let scene = cube_scene();
        let viewport = Viewport::new(60, 24);
        let mut fb = Framebuffer::new(0, 0);
        let mut s = state(viewport);
        s.on_key(key(KeyCode::Char('l'))); // open the menu

        let info = StatusInfo {
            file: "cube",
            triangles: 12,
            fps: 60.0,
            simplify: None,
        };
        let frame = s
            .render(&scene, viewport, &mut fb, ColorMode::None, Some(&info))
            .unwrap();
        assert!(
            frame.to_text().contains("Light menu"),
            "the modal menu panel must be drawn while open"
        );
    }

    // --- Simplification budget decision + adaptive resolution -------------------------------------

    fn settings_with(opts: RenderOpts) -> Settings {
        Settings::resolve(&Config::default(), &opts)
    }

    #[test]
    fn auto_simplify_triggers_only_above_the_budget() {
        let s = settings_with(RenderOpts::default());
        let budget = s.simplify_budget;
        // At or below the budget: no simplification.
        assert_eq!(simplify_target(budget, &s), None);
        assert_eq!(simplify_target(budget / 2, &s), None);
        // Above the budget: target the budget.
        assert_eq!(simplify_target(budget + 1, &s), Some(budget));
        assert_eq!(simplify_target(budget * 5, &s), Some(budget));
    }

    #[test]
    fn no_simplify_flag_disables_all_simplification() {
        // Even an explicit ratio is suppressed by --no-simplify.
        let s = settings_with(RenderOpts {
            simplify: Some(0.25),
            no_simplify: true,
            ..RenderOpts::default()
        });
        assert_eq!(simplify_target(1_000_000, &s), None);
        assert!(!s.auto_simplify);
        assert_eq!(s.simplify, None);
    }

    #[test]
    fn explicit_ratio_targets_a_fraction_of_the_triangles() {
        let s = settings_with(RenderOpts {
            simplify: Some(0.25),
            ..RenderOpts::default()
        });
        // 0.25 of 800k -> ~200k.
        assert_eq!(simplify_target(800_000, &s), Some(200_000));
        // Ratio applies even below the auto budget.
        assert_eq!(simplify_target(1000, &s), Some(250));
    }

    #[test]
    fn explicit_absolute_target_is_used_directly() {
        let s = settings_with(RenderOpts {
            simplify: Some(50_000.0),
            ..RenderOpts::default()
        });
        assert_eq!(simplify_target(705_000, &s), Some(50_000));
        // A target above the current count is returned as-is; load_scene treats it as a no-op.
        assert_eq!(simplify_target(10_000, &s), Some(50_000));
    }

    #[test]
    fn nonpositive_simplify_value_is_ignored() {
        let s = settings_with(RenderOpts {
            simplify: Some(0.0),
            ..RenderOpts::default()
        });
        // Zero/negative falls through to auto (which is off below the budget).
        assert_eq!(simplify_target(1000, &s), None);
    }

    #[test]
    fn adaptive_downscaled_render_matches_full_dimensions_and_draws() {
        // A reduced-resolution interaction frame must still fill the same cell grid and show the
        // model (it is upscaled into the full framebuffer before encoding).
        let scene = cube_scene();
        let viewport = Viewport::new(48, 24);
        let mut s = state(viewport);
        let mut fb = Framebuffer::new(0, 0);

        let full = s
            .render_scaled(&scene, viewport, &mut fb, ColorMode::None, None, 1)
            .unwrap();
        let low = s
            .render_scaled(&scene, viewport, &mut fb, ColorMode::None, None, 2)
            .unwrap();

        assert_eq!((low.cols(), low.rows()), (full.cols(), full.rows()));
        assert!(
            has_visible_content(&low),
            "the downscaled frame must still draw the model"
        );
    }

    #[test]
    fn status_bar_shows_simplified_counts() {
        let scene = cube_scene();
        let viewport = Viewport::new(80, 24);
        let mut s = state(viewport);
        let mut fb = Framebuffer::new(0, 0);
        let info = StatusInfo {
            file: "big.obj",
            triangles: 150_000,
            fps: 30.0,
            simplify: Some((705_000, 150_000)),
        };
        let frame = s
            .render(&scene, viewport, &mut fb, ColorMode::None, Some(&info))
            .unwrap();
        let text = frame.to_text();
        assert!(
            text.contains("705000\u{2192}150000") && text.contains("simplified"),
            "status bar must surface the N->M simplification"
        );
    }

    // --- Animation playback state machine (mock clock) --------------------------------------------

    fn meta(name: &str, duration: f32) -> AnimMeta {
        AnimMeta {
            name: name.to_string(),
            duration,
        }
    }

    /// A viewer with two animations attached, for the menu/playback tests.
    fn anim_state() -> ViewerState {
        let mut s = state(Viewport::new(40, 20));
        s.set_animations(vec![meta("walk", 2.0), meta("wave", 1.0)]);
        s
    }

    #[test]
    fn play_pause_and_loop_toggle_via_menu() {
        let mut a = AnimState::new(vec![meta("walk", 2.0)]);
        assert!(!a.playing && a.looping);
        assert!(a.on_menu_key(key(KeyCode::Char(' '))));
        assert!(a.playing, "space starts playback");
        assert!(a.on_menu_key(key(KeyCode::Char('l'))));
        assert!(!a.looping, "l toggles loop off");
        assert!(a.on_menu_key(key(KeyCode::Char(' '))));
        assert!(!a.playing, "space pauses");
    }

    #[test]
    fn advance_wraps_when_looping_and_clamps_when_not() {
        // Looping: time wraps modulo duration under a mock clock.
        let mut a = AnimState::new(vec![meta("walk", 2.0)]);
        a.playing = true;
        assert!(a.advance(1.5));
        assert!((a.time - 1.5).abs() < 1e-6);
        assert!(a.advance(1.0)); // 2.5 -> wrap to 0.5
        assert!((a.time - 0.5).abs() < 1e-6, "looping wraps, got {}", a.time);
        assert!(a.playing, "looping keeps playing");

        // Non-looping: time clamps to the end and playback stops.
        let mut b = AnimState::new(vec![meta("walk", 2.0)]);
        b.playing = true;
        b.looping = false;
        assert!(b.advance(5.0));
        assert!((b.time - 2.0).abs() < 1e-6, "clamps to duration");
        assert!(!b.playing, "stops at the end when not looping");
        // Further advance does nothing once paused.
        assert!(!b.advance(1.0));
    }

    #[test]
    fn advance_is_inert_when_paused_or_zero_duration() {
        let mut a = AnimState::new(vec![meta("pose", 0.0)]);
        a.playing = true;
        assert!(!a.advance(1.0), "zero-duration clip never advances");
        let mut b = AnimState::new(vec![meta("walk", 2.0)]); // paused by default
        assert!(!b.advance(1.0));
        assert!(b.time == 0.0);
    }

    #[test]
    fn speed_scales_and_clamps() {
        let mut a = AnimState::new(vec![meta("walk", 2.0)]);
        a.playing = true;
        for _ in 0..40 {
            a.on_menu_key(key(KeyCode::Char('+')));
        }
        assert!((a.speed - ANIM_MAX_SPEED).abs() < 1e-3, "speed clamps high");
        for _ in 0..80 {
            a.on_menu_key(key(KeyCode::Char('-')));
        }
        assert!((a.speed - ANIM_MIN_SPEED).abs() < 1e-3, "speed clamps low");
    }

    #[test]
    fn scrub_and_select_clip() {
        let mut a = AnimState::new(vec![meta("walk", 2.0), meta("wave", 1.0)]);
        // Scrub right advances time; left wraps below zero to near the end (looping).
        a.on_menu_key(key(KeyCode::Right));
        assert!((a.time - ANIM_SCRUB_STEP).abs() < 1e-6);
        a.on_menu_key(key(KeyCode::Left));
        a.on_menu_key(key(KeyCode::Left));
        assert!(a.time > 1.0, "scrub left wraps past zero when looping");

        // Down/Up select the next/previous clip and reset the playhead.
        assert_eq!(a.selected, 0);
        a.on_menu_key(key(KeyCode::Down));
        assert_eq!(a.selected, 1);
        assert_eq!(a.time, 0.0, "selecting a clip resets the playhead");
        a.on_menu_key(key(KeyCode::Down));
        assert_eq!(a.selected, 0, "selection wraps");
        a.on_menu_key(key(KeyCode::Up));
        assert_eq!(a.selected, 1, "up wraps back");
    }

    #[test]
    fn reset_restores_speed_time_and_loop_keeping_selection() {
        let mut a = AnimState::new(vec![meta("walk", 2.0), meta("wave", 1.0)]);
        a.selected = 1;
        a.time = 0.7;
        a.speed = 4.0;
        a.looping = false;
        a.on_menu_key(key(KeyCode::Char('r')));
        assert_eq!(a.time, 0.0);
        assert!((a.speed - 1.0).abs() < 1e-6);
        assert!(a.looping);
        assert_eq!(a.selected, 1, "reset keeps the selected clip");
    }

    #[test]
    fn menu_routes_keys_to_playback_and_freezes_camera() {
        let mut s = anim_state();
        // `A` opens the modal menu.
        let orient0 = s.controls.orientation();
        assert_eq!(s.on_key(key(KeyCode::Char('a'))), KeyAction::Redraw);
        assert!(s.anim.menu_open);

        // Open: Space plays, arrows scrub/select — the camera orbit is frozen.
        assert_eq!(s.on_key(key(KeyCode::Char(' '))), KeyAction::Redraw);
        assert!(s.anim.playing);
        assert_eq!(s.on_key(key(KeyCode::Right)), KeyAction::Redraw);
        assert!(s.anim.time > 0.0);
        assert_eq!(
            s.controls.orientation(),
            orient0,
            "camera must not orbit while the animation menu is open"
        );

        // `Esc` closes; arrows orbit the camera again.
        assert_eq!(s.on_key(key(KeyCode::Esc)), KeyAction::Redraw);
        assert!(!s.anim.menu_open);
        let before = s.controls.orientation();
        s.on_key(key(KeyCode::Left));
        assert_ne!(
            s.controls.orientation(),
            before,
            "camera orbits again once the menu closes"
        );
    }

    #[test]
    fn ctrl_c_quits_with_the_anim_menu_open() {
        let mut s = anim_state();
        s.on_key(key(KeyCode::Char('a')));
        assert!(s.anim.menu_open);
        assert_eq!(
            s.on_key(KeyEvent {
                code: KeyCode::Char('c'),
                modifiers: KeyModifiers {
                    ctrl: true,
                    ..KeyModifiers::NONE
                },
            }),
            KeyAction::Quit
        );
    }

    #[test]
    fn open_anim_menu_draws_panel_and_status_line() {
        let scene = cube_scene();
        let viewport = Viewport::new(120, 24);
        let mut fb = Framebuffer::new(0, 0);
        let mut s = anim_state();
        s.on_key(key(KeyCode::Char('a'))); // open the menu

        let info = StatusInfo {
            file: "walk.glb",
            triangles: 12,
            fps: 60.0,
            simplify: None,
        };
        let frame = s
            .render(&scene, viewport, &mut fb, ColorMode::None, Some(&info))
            .unwrap();
        let text = frame.to_text();
        assert!(
            text.contains("Animation menu"),
            "the modal animation panel must be drawn while open"
        );
        assert!(
            text.contains("anim:walk") && text.contains("loop"),
            "status line must show the clip name and loop state"
        );
    }

    #[test]
    fn no_animations_menu_reports_it_clearly() {
        let scene = cube_scene();
        let viewport = Viewport::new(60, 24);
        let mut fb = Framebuffer::new(0, 0);
        let mut s = state(viewport); // no animations attached
        assert!(!s.anim.has_animations());
        s.on_key(key(KeyCode::Char('a'))); // opens, but there is nothing to play
        assert!(s.anim.menu_open);

        let frame = s
            .render(&scene, viewport, &mut fb, ColorMode::None, None)
            .unwrap();
        assert!(
            frame.to_text().contains("no animations in this file"),
            "the menu must say there are no animations"
        );
    }
}
