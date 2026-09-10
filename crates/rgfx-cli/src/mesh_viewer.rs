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
use rgfx_3d::{OrbitController, Rasterizer, ShadingMode, direction_from_azimuth_elevation};
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

/// Radians the light moves around its sphere per arrow-key press while the light menu is open.
const LIGHT_ORBIT_STEP: f32 = 0.12;
/// Ambient level added or removed per `+`/`-` press in the light menu.
const AMBIENT_STEP: f32 = 0.05;
/// How close the light elevation may approach the poles (±90°) before being clamped.
const MAX_ELEVATION: f32 = std::f32::consts::FRAC_PI_2;
/// Default light azimuth (radians) — matches the rasterizer's upper-front-right default direction
/// `(0.5, 0.7, 1.0)`.
const DEFAULT_AZIMUTH: f32 = 0.463_65;
/// Default light elevation (radians) — matches the rasterizer's default direction.
const DEFAULT_ELEVATION: f32 = 0.559_38;
/// Default ambient floor, matching [`Rasterizer`]'s own default.
const DEFAULT_AMBIENT: f32 = 0.15;

/// How the viewer's directional light is anchored relative to the camera.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LightMode {
    /// The light is fixed relative to the view: as the camera orbits, the world-space light
    /// direction rotates with it, so the light appears to stay put on screen while the object
    /// turns *under* it. This is the requested default.
    Viewer,
    /// The light is fixed in world space: orbiting the camera leaves the world-space light
    /// direction unchanged, so the shading is locked to the object.
    World,
}

impl LightMode {
    /// A short lower-case name for the status bar / menu.
    fn name(self) -> &'static str {
        match self {
            LightMode::Viewer => "viewer",
            LightMode::World => "world",
        }
    }
}

/// The viewer's directional-light model: a mode, a position on a sphere (azimuth + elevation), an
/// ambient floor, and an on/off switch. This is pure and terminal-free so the menu state machine
/// and the per-mode world-direction math are unit-testable headlessly.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LightState {
    /// Whether the light is anchored to the view or to world space.
    mode: LightMode,
    /// Azimuth about world up (`+Y`), in radians.
    azimuth: f32,
    /// Elevation above/below the horizon plane, in radians (clamped to the poles).
    elevation: f32,
    /// Ambient floor in `0.0..=1.0`.
    ambient: f32,
    /// Whether the light is on. When off, the viewer renders unlit.
    on: bool,
}

impl Default for LightState {
    fn default() -> Self {
        Self {
            mode: LightMode::Viewer,
            azimuth: DEFAULT_AZIMUTH,
            elevation: DEFAULT_ELEVATION,
            ambient: DEFAULT_AMBIENT,
            on: true,
        }
    }
}

impl LightState {
    /// The light direction in the light's own frame (view space for [`LightMode::Viewer`], world
    /// space for [`LightMode::World`]), from the current azimuth/elevation.
    fn local_direction(&self) -> Vec3 {
        direction_from_azimuth_elevation(self.azimuth, self.elevation)
    }

    /// The **world-space** direction *toward* the light for the current mode and `camera`:
    ///
    /// - [`LightMode::Viewer`]: the azimuth/elevation direction is interpreted in *view* space and
    ///   transformed into world space by the inverse view matrix, so it rotates with the camera —
    ///   the light stays fixed on screen while the object appears to rotate under it.
    /// - [`LightMode::World`]: the azimuth/elevation direction is used directly in world space, so
    ///   it is invariant as the camera orbits.
    pub(crate) fn world_direction(&self, camera: &Camera) -> Vec3 {
        let dir = self.local_direction();
        match self.mode {
            LightMode::Viewer => camera.view_matrix().inverse().transform_vector3(dir),
            LightMode::World => dir,
        }
    }

    /// Whether the light is currently on.
    pub(crate) fn is_on(&self) -> bool {
        self.on
    }

    /// The ambient floor in `0.0..=1.0`.
    pub(crate) fn ambient(&self) -> f32 {
        self.ambient
    }

    /// Rotates the light around world up by `delta` radians.
    fn orbit_azimuth(&mut self, delta: f32) {
        self.azimuth += delta;
    }

    /// Tilts the light up/down by `delta` radians, clamped just short of the poles.
    fn orbit_elevation(&mut self, delta: f32) {
        self.elevation = (self.elevation + delta).clamp(-MAX_ELEVATION, MAX_ELEVATION);
    }

    /// Cycles the light mode (Viewer ⇄ World).
    fn cycle_mode(&mut self) {
        self.mode = match self.mode {
            LightMode::Viewer => LightMode::World,
            LightMode::World => LightMode::Viewer,
        };
    }

    /// Adjusts the ambient floor by `delta`, clamped to `0.0..=1.0`.
    fn adjust_ambient(&mut self, delta: f32) {
        self.ambient = (self.ambient + delta).clamp(0.0, 1.0);
    }

    /// Toggles the light on/off.
    fn toggle(&mut self) {
        self.on = !self.on;
    }

    /// Resets every light parameter to its default.
    fn reset(&mut self) {
        *self = LightState::default();
    }
}

/// Runs the 3D viewer for a request. Entry point called by the dispatch layer.
pub fn view(request: &ViewRequest<'_>, format: MeshFormat) -> anyhow::Result<()> {
    let path = match request.input {
        Input::File(p) => p.as_path(),
        Input::Stdin => {
            anyhow::bail!("reading meshes from stdin is not supported (pass a file path)")
        }
    };
    let scene = load_scene(path, format)?;

    match &request.settings.output {
        Some(out) => write_output(&scene, path, request.settings, out),
        None => run_interactive(&scene, path, request.settings),
    }
}

/// Loads a [`Scene`] with the loader matching `format`.
fn load_scene(path: &Path, format: MeshFormat) -> anyhow::Result<Scene> {
    match format {
        MeshFormat::Obj => {
            rgfx_3d::load_obj(path).with_context(|| format!("loading OBJ {}", path.display()))
        }
        MeshFormat::Stl => {
            rgfx_3d::load_stl(path).with_context(|| format!("loading STL {}", path.display()))
        }
        MeshFormat::Gltf => {
            rgfx_3d::load_gltf(path).with_context(|| format!("loading glTF {}", path.display()))
        }
        MeshFormat::Blend => rgfx_3d::load_blend(path).with_context(|| {
            format!(
                "loading Blender file {} (via headless export)",
                path.display()
            )
        }),
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
    /// The directional-light model (mode, sphere position, ambient, on/off) driven by the menu.
    light: LightState,
    /// Whether the modal light menu is open. When open, input drives the light, not the camera.
    menu_open: bool,
    /// Whether the status bar / key help overlay is shown.
    show_ui: bool,
    /// The last pointer cell while a left-drag is in progress, for drag-to-orbit deltas.
    last_drag: Option<(u16, u16)>,
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
            light: LightState::default(),
            menu_open: false,
            show_ui: true,
            last_drag: None,
        };
        state.update_clip();
        state
    }

    /// The effective shading mode, resolving the wireframe and lighting toggles over the cycled
    /// filled mode.
    pub(crate) fn effective_shading(&self) -> ShadingMode {
        if self.wireframe {
            return ShadingMode::Wireframe;
        }
        let base = SHADING_CYCLE[self.shading_index];
        if !self.light.is_on() && matches!(base, ShadingMode::Flat | ShadingMode::Smooth) {
            ShadingMode::Unlit
        } else {
            base
        }
    }

    /// Applies a key press, updating the camera or toggles and reporting what the loop should do.
    ///
    /// While the modal light menu is open, keys drive the light (see [`ViewerState::on_menu_key`])
    /// rather than the camera; `L`/`Esc` close it. While it is closed, `L` opens it and the arrows
    /// orbit the camera as usual.
    pub(crate) fn on_key(&mut self, key: KeyEvent) -> KeyAction {
        if self.menu_open {
            return self.on_menu_key(key);
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
            KeyCode::Char('l') | KeyCode::Char('L') => self.menu_open = true,
            KeyCode::Char('f') | KeyCode::Char('F') => self.show_ui = !self.show_ui,
            _ => return KeyAction::Ignore,
        }
        KeyAction::Redraw
    }

    /// Handles a key press while the modal light menu is open, routing it to the light instead of
    /// the camera. `Ctrl+C` still quits; `Esc`/`L` close the menu; unhandled keys are ignored (so
    /// they do not leak through to the camera).
    fn on_menu_key(&mut self, key: KeyEvent) -> KeyAction {
        if key.modifiers.ctrl && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C')) {
            return KeyAction::Quit;
        }
        match key.code {
            KeyCode::Left => self.light.orbit_azimuth(-LIGHT_ORBIT_STEP),
            KeyCode::Right => self.light.orbit_azimuth(LIGHT_ORBIT_STEP),
            KeyCode::Up => self.light.orbit_elevation(LIGHT_ORBIT_STEP),
            KeyCode::Down => self.light.orbit_elevation(-LIGHT_ORBIT_STEP),
            KeyCode::Char('m') | KeyCode::Char('M') => self.light.cycle_mode(),
            KeyCode::Char('+') | KeyCode::Char('=') => self.light.adjust_ambient(AMBIENT_STEP),
            KeyCode::Char('-') | KeyCode::Char('_') => self.light.adjust_ambient(-AMBIENT_STEP),
            KeyCode::Char('o') | KeyCode::Char('O') | KeyCode::Char(' ') => self.light.toggle(),
            KeyCode::Char('r') | KeyCode::Char('R') => self.light.reset(),
            KeyCode::Esc | KeyCode::Char('l') | KeyCode::Char('L') => self.menu_open = false,
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

    /// Builds the rasterizer for the current toggle state.
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
        ras
    }

    /// Renders the scene into `fb` (reused) and encodes it into a [`TerminalFrame`], optionally
    /// overlaying the status bar.
    fn render(
        &mut self,
        scene: &Scene,
        viewport: Viewport,
        fb: &mut Framebuffer,
        encoder_color: ColorMode,
        status: Option<&StatusInfo<'_>>,
    ) -> anyhow::Result<TerminalFrame> {
        let (pw, ph) = viewport.render_size(SUBPIXEL_X, SUBPIXEL_Y);
        fb.resize(pw, ph);
        self.set_aspect(viewport);

        let mut ras = self.rasterizer();
        // Set the world-space light direction from the light mode + current camera each frame (see
        // `LightState::world_direction`), plus the ambient floor. `effective_shading` already
        // forces the lit modes to Unlit when the light is off, so these are otherwise inert.
        ras.set_light_direction(self.light.world_direction(&self.camera));
        ras.ambient = self.light.ambient();
        ras.render(scene, &self.camera, fb)
            .context("rendering mesh")?;

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

        // The modal light menu takes over the overlay when open; otherwise the status bar shows
        // (when the UI is enabled).
        if self.menu_open {
            self.overlay_light_menu(&mut frame);
        } else if self.show_ui {
            if let Some(info) = status {
                self.overlay_status(&mut frame, info);
            }
        }
        Ok(frame)
    }

    /// Draws the two-line status bar (info + key help) across the bottom rows of `frame`. The info
    /// line reports the current light mode (and `off` when the light is disabled).
    fn overlay_status(&self, frame: &mut TerminalFrame, info: &StatusInfo<'_>) {
        let light = if self.light.is_on() {
            format!("light:{}", self.light.mode.name())
        } else {
            "light:off".to_string()
        };
        let status = format!(
            "{} | {} tris | {:.1} fps | {} | {} | {}{}",
            info.file,
            info.triangles,
            info.fps,
            RENDERER_NAME,
            shading_name(self.effective_shading()),
            light,
            if self.color { " | color" } else { "" },
        );
        let help = "arrows:orbit  z/x:roll  +/-:zoom  R:reset  W:wire  S:shade  C:color  L:light-menu  F:ui  Q:quit";
        crate::viewer_chrome::overlay_bottom_bar(frame, &status, help);
    }

    /// Draws the modal light-menu panel across the top rows of `frame`: the current mode, the light
    /// azimuth/elevation, the ambient level, the on/off state, and the menu key help. Reuses
    /// [`crate::viewer_chrome::write_line`] so each row fully replaces the render underneath.
    fn overlay_light_menu(&self, frame: &mut TerminalFrame) {
        let l = &self.light;
        let lines = [
            "-- Light Menu --".to_string(),
            format!("Mode: {} (M cycles)", l.mode.name()),
            format!(
                "Azimuth: {:>4}  Elevation: {:>4}",
                format!("{}", l.azimuth.to_degrees().round() as i32),
                format!("{}", l.elevation.to_degrees().round() as i32),
            ),
            format!(
                "Ambient: {:.2}   Light: {}",
                l.ambient,
                if l.on { "on" } else { "off" }
            ),
            "arrows:move  M:mode  +/-:ambient  O/Space:on/off  R:reset  Esc/L:close".to_string(),
        ];
        for (row, text) in lines.iter().enumerate() {
            crate::viewer_chrome::write_line(frame, row, text);
        }
    }
}

/// The dynamic status-bar inputs that are not part of [`ViewerState`].
struct StatusInfo<'a> {
    /// The displayed file name.
    file: &'a str,
    /// The scene's triangle count.
    triangles: usize,
    /// The most recent measured frames-per-second.
    fps: f32,
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
/// entered, and no status overlay is drawn.
fn write_output(scene: &Scene, path: &Path, settings: &Settings, out: &Path) -> anyhow::Result<()> {
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
    tracing::info!(path = %out.display(), file = %display_name(path), "wrote rendered mesh");
    Ok(())
}

/// Runs the interactive viewer: auto-frame, then an event-driven orbit/render loop that redraws
/// only on input or resize. The [`Session`] restores the terminal on every exit path.
fn run_interactive(scene: &Scene, path: &Path, settings: &Settings) -> anyhow::Result<()> {
    let sphere = scene_sphere(scene)?;
    let file = display_name(path);
    let triangles = scene.triangle_count();

    let mut session = Session::open_with_mouse()?;
    let mut engine = FrameEngine::new(detect_color_mode());
    // One framebuffer, reused across every re-render (no per-frame allocation).
    let mut fb = Framebuffer::new(0, 0);

    let mut viewport = session.viewport()?;
    let mut state = ViewerState::new(sphere, settings, viewport);
    let mut dirty = true;
    let mut fps = 0.0f32;

    loop {
        if dirty {
            let started = Instant::now();
            let info = StatusInfo {
                file: &file,
                triangles,
                fps,
            };
            let frame = state.render(scene, viewport, &mut fb, engine.mode(), Some(&info))?;
            session.render_frame(&mut engine, &frame)?;
            let elapsed = started.elapsed().as_secs_f32();
            if elapsed > 0.0 {
                fps = 1.0 / elapsed;
            }
            dirty = false;
        }

        match session.poll_event(POLL_TIMEOUT)? {
            Some(Event::Resize(cols, rows)) => {
                viewport = Viewport::new(cols, rows);
                state.set_aspect(viewport);
                dirty = true;
            }
            Some(Event::Key(key)) => match state.on_key(key) {
                KeyAction::Quit => break,
                KeyAction::Redraw => dirty = true,
                KeyAction::Ignore => {}
            },
            Some(Event::Mouse(m)) => match state.on_mouse(m) {
                KeyAction::Quit => break,
                KeyAction::Redraw => dirty = true,
                KeyAction::Ignore => {}
            },
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
        let yaw0 = s.controls.yaw();
        let pitch0 = s.controls.pitch();

        assert_eq!(s.on_key(key(KeyCode::Right)), KeyAction::Redraw);
        assert!((s.controls.yaw() - (yaw0 + ORBIT_STEP)).abs() < 1e-6);

        assert_eq!(s.on_key(key(KeyCode::Left)), KeyAction::Redraw);
        assert!((s.controls.yaw() - yaw0).abs() < 1e-6);

        s.on_key(key(KeyCode::Up));
        assert!((s.controls.pitch() - (pitch0 + ORBIT_STEP)).abs() < 1e-6);
        s.on_key(key(KeyCode::Down));
        assert!((s.controls.pitch() - pitch0).abs() < 1e-6);
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

        // Lighting off (via the menu) forces lit modes to Unlit.
        s.shading_index = cycle_index(ShadingMode::Flat);
        assert_eq!(s.effective_shading(), ShadingMode::Flat);
        s.on_key(key(KeyCode::Char('l'))); // open the light menu
        assert!(s.menu_open);
        s.on_key(key(KeyCode::Char('o'))); // toggle the light off inside the menu
        assert!(!s.light.is_on());
        assert_eq!(s.effective_shading(), ShadingMode::Unlit);
        s.on_key(key(KeyCode::Char('l'))); // close the menu
        assert!(!s.menu_open);

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
        assert!((s.controls.yaw() - DEFAULT_YAW).abs() < 1e-6);
        assert!((s.controls.pitch() - DEFAULT_PITCH).abs() < 1e-6);
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

    #[test]
    fn viewer_mode_light_rotates_with_camera_world_mode_is_invariant() {
        let vp = Viewport::new(80, 24);
        let mut s = state(vp);
        s.set_aspect(vp); // sync the camera to the current orbit
        let cam_a = s.camera;
        // Orbit to a clearly different camera angle and re-sync.
        s.controls.orbit(1.0, 0.3);
        s.set_aspect(vp);
        let cam_b = s.camera;

        // Viewer mode (the default): the world-space light direction rotates with the camera, so
        // the two orbit angles yield different world directions.
        let mut light = LightState::default();
        assert_eq!(light.mode, LightMode::Viewer);
        let va = light.world_direction(&cam_a);
        let vb = light.world_direction(&cam_b);
        assert!(
            (va - vb).length() > 1e-3,
            "viewer-mode light must rotate with the camera"
        );

        // World mode: the world-space light direction is invariant to the camera orbit.
        light.cycle_mode();
        assert_eq!(light.mode, LightMode::World);
        let wa = light.world_direction(&cam_a);
        let wb = light.world_direction(&cam_b);
        assert!(
            (wa - wb).length() < 1e-6,
            "world-mode light must not change as the camera orbits"
        );
    }

    #[test]
    fn light_menu_routes_arrows_to_light_and_preserves_camera() {
        let mut s = state(Viewport::new(80, 24));

        // Menu closed: arrows orbit the camera as before.
        let yaw0 = s.controls.yaw();
        assert_eq!(s.on_key(key(KeyCode::Right)), KeyAction::Redraw);
        assert!((s.controls.yaw() - (yaw0 + ORBIT_STEP)).abs() < 1e-6);

        // L opens the modal menu.
        assert_eq!(s.on_key(key(KeyCode::Char('l'))), KeyAction::Redraw);
        assert!(s.menu_open);

        // Menu open: arrows move the light on its sphere, and the camera does NOT orbit.
        let yaw_locked = s.controls.yaw();
        let az0 = s.light.azimuth;
        let el0 = s.light.elevation;
        assert_eq!(s.on_key(key(KeyCode::Left)), KeyAction::Redraw);
        assert_eq!(s.on_key(key(KeyCode::Up)), KeyAction::Redraw);
        assert!((s.light.azimuth - (az0 - LIGHT_ORBIT_STEP)).abs() < 1e-6);
        assert!((s.light.elevation - (el0 + LIGHT_ORBIT_STEP)).abs() < 1e-6);
        assert!(
            (s.controls.yaw() - yaw_locked).abs() < 1e-9,
            "camera must not orbit while the menu is open"
        );

        // Esc closes the menu; camera controls resume.
        assert_eq!(s.on_key(key(KeyCode::Esc)), KeyAction::Redraw);
        assert!(!s.menu_open);
        let yaw2 = s.controls.yaw();
        s.on_key(key(KeyCode::Right));
        assert!((s.controls.yaw() - (yaw2 + ORBIT_STEP)).abs() < 1e-6);
    }

    #[test]
    fn light_menu_mode_ambient_onoff_and_reset() {
        let mut s = state(Viewport::new(80, 24));
        s.on_key(key(KeyCode::Char('l'))); // open the menu

        // M cycles Viewer -> World -> Viewer.
        assert_eq!(s.light.mode, LightMode::Viewer);
        s.on_key(key(KeyCode::Char('m')));
        assert_eq!(s.light.mode, LightMode::World);
        s.on_key(key(KeyCode::Char('m')));
        assert_eq!(s.light.mode, LightMode::Viewer);

        // Ambient clamps to 0..=1 at both ends.
        for _ in 0..100 {
            s.on_key(key(KeyCode::Char('-')));
        }
        assert_eq!(s.light.ambient(), 0.0);
        for _ in 0..100 {
            s.on_key(key(KeyCode::Char('+')));
        }
        assert_eq!(s.light.ambient(), 1.0);

        // On/off toggles with both O and Space.
        assert!(s.light.is_on());
        s.on_key(key(KeyCode::Char('o')));
        assert!(!s.light.is_on());
        s.on_key(key(KeyCode::Char(' ')));
        assert!(s.light.is_on());

        // R resets every light parameter to its default.
        s.on_key(key(KeyCode::Char('m'))); // perturb mode
        s.on_key(key(KeyCode::Left)); // perturb azimuth
        s.on_key(key(KeyCode::Char('o'))); // perturb on/off
        s.on_key(key(KeyCode::Char('r')));
        assert_eq!(s.light, LightState::default());
    }

    #[test]
    fn light_elevation_clamps_at_the_poles() {
        let mut l = LightState::default();
        for _ in 0..1000 {
            l.orbit_elevation(LIGHT_ORBIT_STEP);
        }
        assert!(l.elevation <= MAX_ELEVATION + 1e-6);
        for _ in 0..2000 {
            l.orbit_elevation(-LIGHT_ORBIT_STEP);
        }
        assert!(l.elevation >= -MAX_ELEVATION - 1e-6);
    }
}
