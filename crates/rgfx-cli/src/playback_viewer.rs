//! The animated-playback viewer: drive a [`rgfx_core::FrameSource`] through the frame engine.
//!
//! This is the viewer behind `rgfx animation.gif` and `rgfx video.mp4` (task 023). Like the
//! still-image and mesh viewers it adds no decoding of its own — it composes the sibling crates,
//! honouring the one architectural law:
//!
//! 1. a [`FrameSource`] — a `GifSource` (`rgfx-image`) for GIFs, or the `ffmpeg`-gated `Player`
//!    (`rgfx-video`) for video — renders each frame into a single reused [`Framebuffer`];
//! 2. a [`TerminalEncoder`] (Braille / ASCII / blocks, chosen from `--renderer`) turns that
//!    framebuffer into a [`TerminalFrame`];
//! 3. the diffing [`FrameEngine`] presents only the cells that changed, and the loop paces frame
//!    presentation to the source's `frame_delay()` / the target fps.
//!
//! The playback *policy* — pause/resume, seek, restart, fps nudging, and the wait between frames —
//! lives in a pure, terminal-free `PlaybackController` driving a `PlaybackSource`, so the control
//! state machine and the resize recompute are unit-testable without a real terminal. That policy
//! generalises [`rgfx_terminal::FrameClock`] to per-frame variable delays (GIFs carry a different
//! delay per frame), which is why the loop paces on an explicit deadline rather than a fixed
//! frame period.

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Context;
use rgfx_core::{FrameSource, Framebuffer, TerminalEncoder, TerminalFrame, Viewport};
use rgfx_image::{BayerSize, Dither, GifLoop, GifSource, Preprocess, RenderOptions, Tone};
use rgfx_terminal::{
    AsciiEncoder, BlockEncoder, BrailleEncoder, ColorMode, Event, FrameEngine, KeyCode, KeyEvent,
};

use crate::cli::{DitherMode, Renderer};
use crate::config::Settings;
use crate::dispatch::ViewRequest;
use crate::media::Input;
use crate::terminal::{Session, is_quit};

/// Default render width in columns for non-interactive `--output` when no `--width` is given.
const DEFAULT_OUTPUT_COLS: u16 = 80;
/// How long the loop blocks on input between checks. Bounds shutdown latency and, while paused,
/// how often the loop wakes; a due frame or an event wakes it sooner.
const POLL_TIMEOUT: Duration = Duration::from_millis(100);
/// The wall-clock step applied by a single `←`/`→` seek press (video only).
#[cfg(feature = "ffmpeg")]
const SEEK_STEP: Duration = Duration::from_secs(5);
/// The increment applied by a single `+`/`-` fps press.
const FPS_STEP: f64 = 2.0;
/// The clamped bounds for the adjustable target fps.
const FPS_MIN: f64 = 1.0;
const FPS_MAX: f64 = 240.0;

/// Entry point for the animated-GIF viewer, called by the dispatch layer.
pub(crate) fn view_gif(request: &ViewRequest<'_>) -> anyhow::Result<()> {
    let path = file_path(request.input, "GIFs")?;
    let bytes = std::fs::read(path).with_context(|| format!("loading GIF {}", path.display()))?;
    let name = display_name(path);

    match &request.settings.output {
        Some(out) => {
            let vp = output_viewport(request.settings);
            let mut source = GifPlayback::new(&bytes, vp, request.settings, name)?;
            write_output(&mut source, request.settings, vp, out)
        }
        None => run_interactive(request.settings, |vp| {
            Ok(Box::new(GifPlayback::new(
                &bytes,
                vp,
                request.settings,
                name.clone(),
            )?))
        }),
    }
}

/// Entry point for the video viewer (requires the `ffmpeg` feature at build time).
#[cfg(feature = "ffmpeg")]
pub(crate) fn view_video(request: &ViewRequest<'_>) -> anyhow::Result<()> {
    let path = file_path(request.input, "video")?.to_path_buf();
    let name = display_name(&path);

    match &request.settings.output {
        Some(out) => {
            let vp = output_viewport(request.settings);
            let mut source = VideoPlayback::open(&path, vp, request.settings, name)?;
            write_output(&mut source, request.settings, vp, out)
        }
        None => run_interactive(request.settings, |vp| {
            Ok(Box::new(VideoPlayback::open(
                &path,
                vp,
                request.settings,
                name.clone(),
            )?))
        }),
    }
}

/// Entry point for the video viewer when `ffmpeg` support was not compiled in.
///
/// Reports a clean, actionable error rather than pretending to play the file.
#[cfg(not(feature = "ffmpeg"))]
pub(crate) fn view_video(_request: &ViewRequest<'_>) -> anyhow::Result<()> {
    anyhow::bail!(
        "video playback requires the `ffmpeg` feature: rebuild with `--features ffmpeg` \
         (and install the `ffmpeg` executable on your PATH)"
    )
}

/// Extracts a file path from the input, rejecting stdin with a clear message.
fn file_path<'a>(input: &'a Input, what: &str) -> anyhow::Result<&'a Path> {
    match input {
        Input::File(p) => Ok(p.as_path()),
        Input::Stdin => {
            anyhow::bail!("reading {what} from stdin is not yet implemented (arrives in task 025)")
        }
    }
}

/// A short display name for a file (its final path component).
fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

// ---------------------------------------------------------------------------
// Renderer resolution (encoder + matching framebuffer render options)
// ---------------------------------------------------------------------------

/// The concrete encoder selected from the [`Renderer`] setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResolvedRenderer {
    Braille,
    Ascii,
    Blocks,
}

impl ResolvedRenderer {
    /// Resolves `auto` to a concrete encoder: half-blocks when colour is requested, otherwise the
    /// higher-resolution Braille encoder.
    fn resolve(renderer: Renderer, color: bool) -> Self {
        match renderer {
            Renderer::Braille => Self::Braille,
            Renderer::Ascii => Self::Ascii,
            Renderer::Blocks => Self::Blocks,
            Renderer::Auto => {
                if color {
                    Self::Blocks
                } else {
                    Self::Braille
                }
            }
        }
    }

    /// The base render options (subpixel factors, cell aspect, resize filter) for this encoder.
    fn base_options(self) -> RenderOptions {
        match self {
            Self::Braille => RenderOptions::braille(),
            Self::Ascii => RenderOptions::ascii(),
            Self::Blocks => RenderOptions::half_blocks(),
        }
    }

    /// A boxed encoder instance. Encoders are stateless and cheap to construct.
    fn encoder(self) -> Box<dyn TerminalEncoder> {
        match self {
            Self::Braille => Box::new(BrailleEncoder::new()),
            Self::Ascii => Box::new(AsciiEncoder::new()),
            Self::Blocks => Box::new(BlockEncoder::new()),
        }
    }
}

/// Maps the CLI [`DitherMode`] onto the image crate's [`Dither`], resolving `auto` per renderer
/// (Floyd–Steinberg for the 1-bit Braille path, none for the grayscale-ramp encoders).
fn resolve_dither(mode: DitherMode, resolved: ResolvedRenderer) -> Dither {
    match mode {
        DitherMode::Auto => match resolved {
            ResolvedRenderer::Braille => Dither::FloydSteinberg,
            ResolvedRenderer::Ascii | ResolvedRenderer::Blocks => Dither::None,
        },
        DitherMode::None => Dither::None,
        DitherMode::Threshold => Dither::ThresholdOnly,
        DitherMode::Floyd => Dither::FloydSteinberg,
        DitherMode::Atkinson => Dither::Atkinson,
        DitherMode::Bayer => Dither::Bayer(BayerSize::Four),
    }
}

/// Builds the render options (subpixels + the tone/dither quality stage) for `settings`.
fn render_options(settings: &Settings) -> (ResolvedRenderer, RenderOptions) {
    let resolved = ResolvedRenderer::resolve(settings.renderer, settings.color);
    let mut opts = resolved.base_options();
    opts.preprocess = Preprocess {
        tone: Tone {
            gamma: settings.gamma,
            contrast: settings.contrast,
            brightness: 0.0,
            sharpen: None,
        },
        dither: resolve_dither(settings.dither, resolved),
        threshold: settings.threshold,
    };
    (resolved, opts)
}

/// The non-interactive `--output` viewport: a fixed width with a roughly square pixel field.
fn output_viewport(settings: &Settings) -> Viewport {
    let cols = settings
        .width
        .unwrap_or(DEFAULT_OUTPUT_COLS as u32)
        .clamp(1, u16::MAX as u32) as u16;
    let rows = (cols / 2).max(1);
    Viewport::new(cols, rows)
}

/// The interactive render viewport: the terminal bounds, narrowed to `--width` (keeping the
/// terminal's cell aspect) when one is set. The source letterboxes its content within it.
fn playback_viewport(settings: &Settings, bounds: Viewport) -> Viewport {
    match settings.width {
        Some(w) => {
            let cols = w.clamp(1, u16::MAX as u32) as u16;
            let rows = ((cols as u32 * bounds.rows.max(1) as u32) / bounds.cols.max(1) as u32)
                .clamp(1, u16::MAX as u32) as u16;
            Viewport::new(cols, rows)
        }
        None => Viewport::new(bounds.cols.max(1), bounds.rows.max(1)),
    }
}

// ---------------------------------------------------------------------------
// The playback-source abstraction (unifies GIF and video for the loop + tests)
// ---------------------------------------------------------------------------

/// A source of animated frames the [`PlaybackController`] drives, abstracted so the control state
/// machine can be tested against a mock without a decoder or a terminal.
pub(crate) trait PlaybackSource {
    /// Renders the next frame into `target`. `Ok(false)` once the source is exhausted.
    fn next_frame(&mut self, target: &mut Framebuffer) -> anyhow::Result<bool>;

    /// How long to wait before presenting the following frame, honouring an fps override when set.
    fn next_wait(&self, override_fps: Option<f64>) -> Duration;

    /// Restarts the source from the beginning.
    fn restart(&mut self) -> anyhow::Result<()>;

    /// Rebuilds internal sizing for a new viewport (e.g. on a terminal resize).
    fn resize(&mut self, viewport: Viewport) -> anyhow::Result<()>;

    /// Whether seeking is meaningful for this source (video yes, GIF no).
    fn can_seek(&self) -> bool {
        false
    }

    /// Seeks by `SEEK_STEP` forward or backward. A no-op when [`PlaybackSource::can_seek`] is false.
    fn seek(&mut self, _forward: bool) -> anyhow::Result<()> {
        Ok(())
    }

    /// Freezes or resumes the source's internal timing (video); a no-op for GIFs.
    fn set_paused(&mut self, _paused: bool) -> anyhow::Result<()> {
        Ok(())
    }

    /// Whether the source should restart rather than end when exhausted.
    fn loops(&self) -> bool {
        false
    }

    /// A short status string (position / fps) for the overlay.
    fn status_line(&self) -> String {
        String::new()
    }
}

/// Forwards through a boxed source so the interactive loop can hold a `Box<dyn PlaybackSource>`
/// (GIF or video, chosen at runtime) in the generic [`PlaybackController`].
impl PlaybackSource for Box<dyn PlaybackSource> {
    fn next_frame(&mut self, target: &mut Framebuffer) -> anyhow::Result<bool> {
        (**self).next_frame(target)
    }
    fn next_wait(&self, override_fps: Option<f64>) -> Duration {
        (**self).next_wait(override_fps)
    }
    fn restart(&mut self) -> anyhow::Result<()> {
        (**self).restart()
    }
    fn resize(&mut self, viewport: Viewport) -> anyhow::Result<()> {
        (**self).resize(viewport)
    }
    fn can_seek(&self) -> bool {
        (**self).can_seek()
    }
    fn seek(&mut self, forward: bool) -> anyhow::Result<()> {
        (**self).seek(forward)
    }
    fn set_paused(&mut self, paused: bool) -> anyhow::Result<()> {
        (**self).set_paused(paused)
    }
    fn loops(&self) -> bool {
        (**self).loops()
    }
    fn status_line(&self) -> String {
        (**self).status_line()
    }
}

/// A GIF-backed [`PlaybackSource`]: owns the raw bytes so it can rebuild on resize.
struct GifPlayback {
    bytes: Vec<u8>,
    opts: RenderOptions,
    loop_mode: GifLoop,
    fallback: Duration,
    source: GifSource,
    name: String,
}

impl GifPlayback {
    /// Builds a GIF playback source from raw bytes rendered into `viewport`.
    fn new(
        bytes: &[u8],
        viewport: Viewport,
        settings: &Settings,
        name: String,
    ) -> anyhow::Result<Self> {
        let (_, opts) = render_options(settings);
        let loop_mode = if settings.loop_playback {
            GifLoop::Infinite
        } else {
            GifLoop::Once
        };
        let fallback = fps_to_delay(settings.fps as f64);
        let source = GifSource::from_bytes(bytes.to_vec(), viewport, opts, loop_mode)
            .with_context(|| format!("decoding GIF {name}"))?;
        Ok(Self {
            bytes: bytes.to_vec(),
            opts,
            loop_mode,
            fallback,
            source,
            name,
        })
    }
}

impl PlaybackSource for GifPlayback {
    fn next_frame(&mut self, target: &mut Framebuffer) -> anyhow::Result<bool> {
        Ok(self.source.next_frame(target)?)
    }

    fn next_wait(&self, override_fps: Option<f64>) -> Duration {
        match override_fps {
            Some(fps) if fps > 0.0 => fps_to_delay(fps),
            _ => self.source.frame_delay().unwrap_or(self.fallback),
        }
    }

    fn restart(&mut self) -> anyhow::Result<()> {
        Ok(self.source.reset()?)
    }

    fn resize(&mut self, viewport: Viewport) -> anyhow::Result<()> {
        self.source =
            GifSource::from_bytes(self.bytes.clone(), viewport, self.opts, self.loop_mode)
                .with_context(|| format!("re-decoding GIF {}", self.name))?;
        Ok(())
    }

    fn loops(&self) -> bool {
        self.loop_mode == GifLoop::Infinite
    }

    fn status_line(&self) -> String {
        format!("{} | gif", self.name)
    }
}

/// A video-backed [`PlaybackSource`] driving the `ffmpeg` [`Player`](rgfx_video::Player).
#[cfg(feature = "ffmpeg")]
struct VideoPlayback {
    path: std::path::PathBuf,
    opts: RenderOptions,
    loop_playback: bool,
    player: rgfx_video::Player,
    name: String,
}

#[cfg(feature = "ffmpeg")]
impl VideoPlayback {
    /// Opens `path` for playback into `viewport`, probing and spawning `ffmpeg`.
    fn open(
        path: &Path,
        viewport: Viewport,
        settings: &Settings,
        name: String,
    ) -> anyhow::Result<Self> {
        let (_, opts) = render_options(settings);
        let player = rgfx_video::Player::open(path, viewport, opts, rgfx_video::DEFAULT_MAX_SKIP)
            .with_context(|| format!("opening video {name}"))?;
        Ok(Self {
            path: path.to_path_buf(),
            opts,
            loop_playback: settings.loop_playback,
            player,
            name,
        })
    }
}

#[cfg(feature = "ffmpeg")]
impl PlaybackSource for VideoPlayback {
    fn next_frame(&mut self, target: &mut Framebuffer) -> anyhow::Result<bool> {
        Ok(self.player.next_frame(target)?)
    }

    fn next_wait(&self, _override_fps: Option<f64>) -> Duration {
        // Video paces itself to the source rate (with the player's internal adaptive skip); the
        // fps override does not re-rate the decoder.
        self.player.time_until_next_frame()
    }

    fn restart(&mut self) -> anyhow::Result<()> {
        Ok(self.player.restart()?)
    }

    fn resize(&mut self, viewport: Viewport) -> anyhow::Result<()> {
        self.player = rgfx_video::Player::open(
            &self.path,
            viewport,
            self.opts,
            rgfx_video::DEFAULT_MAX_SKIP,
        )
        .with_context(|| format!("resizing video {}", self.name))?;
        Ok(())
    }

    fn can_seek(&self) -> bool {
        true
    }

    fn seek(&mut self, forward: bool) -> anyhow::Result<()> {
        let pos = self.player.status().position;
        let target = if forward {
            pos.saturating_add(SEEK_STEP)
        } else {
            pos.saturating_sub(SEEK_STEP)
        };
        Ok(self.player.seek(target)?)
    }

    fn set_paused(&mut self, paused: bool) -> anyhow::Result<()> {
        if paused {
            self.player.pause();
        } else {
            self.player.play();
        }
        Ok(())
    }

    fn loops(&self) -> bool {
        self.loop_playback
    }

    fn status_line(&self) -> String {
        let s = self.player.status();
        let pos = s.position.as_secs_f64();
        match s.duration {
            Some(d) => format!(
                "{} | {:.1}/{:.1}s | {:.0} fps",
                self.name,
                pos,
                d.as_secs_f64(),
                s.fps
            ),
            None => format!("{} | {:.1}s | {:.0} fps", self.name, pos, s.fps),
        }
    }
}

/// Derives a per-frame delay from an fps value, guarding non-positive rates.
fn fps_to_delay(fps: f64) -> Duration {
    if fps.is_finite() && fps > 0.0 {
        Duration::from_secs_f64(1.0 / fps)
    } else {
        Duration::from_millis(100)
    }
}

// ---------------------------------------------------------------------------
// The pure control state machine
// ---------------------------------------------------------------------------

/// What a handled key press means for the render loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Quit the viewer and restore the terminal.
    Quit,
    /// Re-present the current position immediately (after a seek/restart).
    Redraw,
    /// Timing changed; recompute the next-frame deadline without decoding.
    Reschedule,
    /// Nothing actionable happened.
    Continue,
}

/// Ties the pure control policy (pause/seek/restart/fps) to a [`PlaybackSource`].
///
/// Every mutating interaction goes through here, which is what makes the pause/seek/restart state
/// machine and the resize recompute unit-testable against a mock source without a terminal.
pub(crate) struct PlaybackController<S: PlaybackSource> {
    source: S,
    paused: bool,
    /// The user's fps override (via `--fps` or `+/-`), or `None` to follow the source.
    override_fps: Option<f64>,
    /// Whether the bottom options bar is shown (toggled with `F`).
    show_ui: bool,
}

impl<S: PlaybackSource> PlaybackController<S> {
    /// Creates a controller starting in the playing state, seeding any `--fps` override.
    pub(crate) fn new(source: S, initial_fps: Option<f64>) -> Self {
        Self {
            source,
            paused: false,
            override_fps: initial_fps,
            show_ui: true,
        }
    }

    /// Whether playback is currently paused.
    pub(crate) fn is_paused(&self) -> bool {
        self.paused
    }

    /// Whether the bottom options bar is currently shown.
    pub(crate) fn show_ui(&self) -> bool {
        self.show_ui
    }

    /// The current effective target fps, if one is in force.
    pub(crate) fn override_fps(&self) -> Option<f64> {
        self.override_fps
    }

    /// Whether the source loops rather than ending when exhausted.
    pub(crate) fn loops(&self) -> bool {
        self.source.loops()
    }

    /// The wait before the next frame is due.
    pub(crate) fn wait(&self) -> Duration {
        self.source.next_wait(self.override_fps)
    }

    /// A short status string for the overlay.
    pub(crate) fn status_line(&self) -> String {
        self.source.status_line()
    }

    /// Advances one frame while playing; a paused controller is a no-op reporting `Ok(true)` so the
    /// caller keeps the last frame displayed. `Ok(false)` once the source is exhausted.
    pub(crate) fn advance(&mut self, target: &mut Framebuffer) -> anyhow::Result<bool> {
        if self.paused {
            return Ok(true);
        }
        self.source.next_frame(target)
    }

    /// Renders a frame regardless of pause state (used for the first frame and after a seek), then
    /// restores the pause state so a paused source stays frozen.
    pub(crate) fn force_frame(&mut self, target: &mut Framebuffer) -> anyhow::Result<bool> {
        if self.paused {
            self.source.set_paused(false)?;
            let produced = self.source.next_frame(target);
            self.source.set_paused(true)?;
            produced
        } else {
            self.source.next_frame(target)
        }
    }

    /// Restarts the source from the beginning, preserving the pause state.
    pub(crate) fn restart(&mut self) -> anyhow::Result<()> {
        self.source.restart()
    }

    /// Rebuilds the source for a new viewport.
    pub(crate) fn resize(&mut self, viewport: Viewport) -> anyhow::Result<()> {
        self.source.resize(viewport)
    }

    /// Applies a key press, mutating state and reporting what the loop should do.
    pub(crate) fn on_key(&mut self, key: KeyEvent) -> anyhow::Result<Outcome> {
        if is_quit(key) {
            return Ok(Outcome::Quit);
        }
        match key.code {
            KeyCode::Char(' ') => {
                self.toggle_pause()?;
                // Resuming should advance promptly; pausing just holds the frame.
                Ok(if self.paused {
                    Outcome::Continue
                } else {
                    Outcome::Reschedule
                })
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                self.restart()?;
                Ok(Outcome::Redraw)
            }
            KeyCode::Char('f') | KeyCode::Char('F') => {
                self.show_ui = !self.show_ui;
                Ok(Outcome::Redraw)
            }
            KeyCode::Left => self.seek(false),
            KeyCode::Right => self.seek(true),
            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.adjust_fps(FPS_STEP);
                Ok(Outcome::Reschedule)
            }
            KeyCode::Char('-') | KeyCode::Char('_') => {
                self.adjust_fps(-FPS_STEP);
                Ok(Outcome::Reschedule)
            }
            _ => Ok(Outcome::Continue),
        }
    }

    /// Toggles pause, keeping the source's internal clock in sync.
    fn toggle_pause(&mut self) -> anyhow::Result<()> {
        self.paused = !self.paused;
        self.source.set_paused(self.paused)
    }

    /// Seeks the source, if it supports seeking; otherwise a no-op that keeps playing.
    fn seek(&mut self, forward: bool) -> anyhow::Result<Outcome> {
        if !self.source.can_seek() {
            return Ok(Outcome::Continue);
        }
        self.source.seek(forward)?;
        Ok(Outcome::Redraw)
    }

    /// Nudges the target fps by `delta`, clamped, seeding from the current fallback when unset.
    fn adjust_fps(&mut self, delta: f64) {
        let base = self.override_fps.unwrap_or_else(|| {
            let d = self.source.next_wait(None).as_secs_f64();
            if d > 0.0 { 1.0 / d } else { 30.0 }
        });
        self.override_fps = Some((base + delta).clamp(FPS_MIN, FPS_MAX));
    }
}

// ---------------------------------------------------------------------------
// Interactive loop + non-interactive output
// ---------------------------------------------------------------------------

/// The two-line control help shown at the bottom of the frame.
const HELP: &str = "space:pause  <-/->:seek  R:restart  +/-:fps  F:ui  Q:quit";

/// Runs the interactive playback loop. `build` constructs the source for a given viewport, so the
/// loop owns the terminal lifecycle and can rebuild the source on resize.
fn run_interactive<F>(settings: &Settings, build: F) -> anyhow::Result<()>
where
    F: Fn(Viewport) -> anyhow::Result<Box<dyn PlaybackSource>>,
{
    let mut session = Session::open()?;
    let mut engine = FrameEngine::new(ColorMode::None);
    // One framebuffer, reused across every frame (no per-frame allocation).
    let mut fb = Framebuffer::new(0, 0);
    let (resolved, _) = render_options(settings);
    let encoder = resolved.encoder();

    let mut viewport = playback_viewport(settings, session.viewport()?);
    let source = build(viewport)?;
    // Start with no fps override so GIFs pace on their native per-frame delays and video follows
    // the source rate; the `+`/`-` keys introduce an override live.
    let mut controller = PlaybackController::new(source, None);

    // Present the first frame immediately; an empty source ends cleanly.
    if !controller.force_frame(&mut fb)? {
        return Ok(());
    }
    present(
        &mut engine,
        &mut session,
        &fb,
        encoder.as_ref(),
        viewport,
        &controller,
    )?;
    let mut next_due = Instant::now() + controller.wait();

    loop {
        let now = Instant::now();
        let timeout = if controller.is_paused() {
            POLL_TIMEOUT
        } else {
            next_due.saturating_duration_since(now).min(POLL_TIMEOUT)
        };

        match session.poll_event(timeout)? {
            Some(Event::Resize(cols, rows)) => {
                viewport = playback_viewport(settings, Viewport::new(cols, rows));
                controller.resize(viewport)?;
                if controller.force_frame(&mut fb)? {
                    present(
                        &mut engine,
                        &mut session,
                        &fb,
                        encoder.as_ref(),
                        viewport,
                        &controller,
                    )?;
                }
                next_due = Instant::now() + controller.wait();
            }
            Some(Event::Key(key)) => match controller.on_key(key)? {
                Outcome::Quit => break,
                Outcome::Redraw => {
                    if controller.force_frame(&mut fb)? {
                        present(
                            &mut engine,
                            &mut session,
                            &fb,
                            encoder.as_ref(),
                            viewport,
                            &controller,
                        )?;
                    }
                    next_due = Instant::now() + controller.wait();
                }
                Outcome::Reschedule => next_due = Instant::now(),
                Outcome::Continue => {}
            },
            _ => {}
        }

        if !controller.is_paused() && Instant::now() >= next_due {
            if controller.advance(&mut fb)? {
                present(
                    &mut engine,
                    &mut session,
                    &fb,
                    encoder.as_ref(),
                    viewport,
                    &controller,
                )?;
                next_due = Instant::now() + controller.wait();
            } else if controller.loops() {
                controller.restart()?;
                next_due = Instant::now();
            } else {
                break;
            }
        }
    }
    Ok(())
}

/// Encodes `fb` and presents it through the diffing frame engine, overlaying the status/help bar.
fn present<S: PlaybackSource>(
    engine: &mut FrameEngine,
    session: &mut Session,
    fb: &Framebuffer,
    encoder: &dyn TerminalEncoder,
    viewport: Viewport,
    controller: &PlaybackController<S>,
) -> anyhow::Result<()> {
    let mut frame = encoder.encode(fb, viewport);
    if controller.show_ui() {
        overlay_status(&mut frame, controller);
    }
    session.render_frame(engine, &frame)?;
    Ok(())
}

/// Draws the two-line status/help bar across the bottom rows of `frame`.
fn overlay_status<S: PlaybackSource>(
    frame: &mut TerminalFrame,
    controller: &PlaybackController<S>,
) {
    let paused = if controller.is_paused() {
        " | paused"
    } else {
        ""
    };
    let fps = controller
        .override_fps()
        .map(|f| format!(" | {f:.0} fps"))
        .unwrap_or_default();
    let status = format!("{}{}{}", controller.status_line(), fps, paused);
    crate::viewer_chrome::overlay_bottom_bar(frame, &status, HELP);
}

/// Renders the first frame to `out` as text (`--output`). Non-interactive: no terminal is entered.
fn write_output<S: PlaybackSource>(
    source: &mut S,
    settings: &Settings,
    viewport: Viewport,
    out: &Path,
) -> anyhow::Result<()> {
    let (resolved, _) = render_options(settings);
    let mut fb = Framebuffer::new(0, 0);
    if !source.next_frame(&mut fb)? {
        anyhow::bail!("source produced no frames to write");
    }
    let frame = resolved.encoder().encode(&fb, viewport);
    std::fs::write(out, frame.to_text())
        .with_context(|| format!("writing output to {}", out.display()))?;
    tracing::info!(path = %out.display(), "wrote first playback frame");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::RenderOpts;
    use crate::config::Config;

    /// The bundled 3-frame 2×2 test GIF (red → green-on-clear → blue-added), shared with
    /// `rgfx-image`'s decoder tests. Frame delays: 100 / 200 / 300 ms.
    const TEST_GIF_3FRAME: &[u8] = &[
        71, 73, 70, 56, 57, 97, 2, 0, 2, 0, 145, 0, 0, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255,
        33, 255, 11, 78, 69, 84, 83, 67, 65, 80, 69, 50, 46, 48, 3, 1, 0, 0, 0, 33, 249, 4, 8, 10,
        0, 0, 0, 44, 0, 0, 0, 0, 2, 0, 2, 0, 0, 2, 2, 140, 83, 0, 33, 249, 4, 4, 20, 0, 0, 0, 44,
        0, 0, 0, 0, 1, 0, 1, 0, 0, 2, 2, 84, 1, 0, 33, 249, 4, 4, 30, 0, 0, 0, 44, 1, 0, 1, 0, 1,
        0, 1, 0, 0, 2, 2, 92, 1, 0, 59,
    ];

    fn settings(opts: RenderOpts) -> Settings {
        Settings::resolve(&Config::default(), &opts)
    }

    fn gif(settings: &Settings) -> GifPlayback {
        GifPlayback::new(
            TEST_GIF_3FRAME,
            Viewport::new(2, 2),
            settings,
            "test.gif".into(),
        )
        .unwrap()
    }

    /// A stable per-frame signature: every framebuffer pixel quantised to bytes. Deterministic
    /// decoding makes this an exact fingerprint of a frame's content.
    fn snapshot(fb: &Framebuffer) -> Vec<[u8; 4]> {
        let mut sig = Vec::with_capacity(fb.width() * fb.height());
        for y in 0..fb.height() {
            for x in 0..fb.width() {
                let c = fb.get(x, y);
                sig.push([
                    (c.r * 255.0) as u8,
                    (c.g * 255.0) as u8,
                    (c.b * 255.0) as u8,
                    (c.a * 255.0) as u8,
                ]);
            }
        }
        sig
    }

    fn playback_settings() -> Settings {
        settings(RenderOpts {
            renderer: Some(Renderer::Ascii),
            dither: Some(DitherMode::None),
            ..RenderOpts::default()
        })
    }

    #[test]
    fn bundled_gif_plays_all_frames_in_order() {
        // Drive the bundled 3-frame GIF through the playback abstraction and confirm the exact
        // frame count, that each frame differs from the last (proving order/advance, not a stuck
        // frame), and that the per-frame delays are reported in the GIF's authored order.
        let s = playback_settings();
        let mut src = gif(&s);
        let mut fb = Framebuffer::new(0, 0);

        let mut frames = Vec::new();
        let mut delays = Vec::new();
        while src.next_frame(&mut fb).unwrap() {
            frames.push(snapshot(&fb));
            delays.push(src.next_wait(None));
        }

        // Exactly three frames for a non-looping GIF, then exhausted (stays false).
        assert_eq!(frames.len(), 3, "the GIF has three frames");
        assert!(!src.next_frame(&mut fb).unwrap());
        assert!(!src.next_frame(&mut fb).unwrap());

        // Delays are in authored order: 100 / 200 / 300 ms.
        assert_eq!(
            delays,
            vec![
                Duration::from_millis(100),
                Duration::from_millis(200),
                Duration::from_millis(300),
            ]
        );

        // Consecutive frames differ (the animation actually advances, in order).
        assert_ne!(frames[0], frames[1], "frame 0 -> 1 must change");
        assert_ne!(frames[1], frames[2], "frame 1 -> 2 must change");
    }

    #[test]
    fn fps_override_replaces_native_delay() {
        let s = playback_settings();
        let mut src = gif(&s);
        let mut fb = Framebuffer::new(0, 0);
        src.next_frame(&mut fb).unwrap();
        // Native delay is 100 ms; a 50 fps override yields 20 ms regardless.
        assert_eq!(src.next_wait(Some(50.0)), Duration::from_millis(20));
    }

    #[test]
    fn gif_resize_recomputes_framebuffer_size() {
        // A resize mid-stream rebuilds the source at the new viewport; the reused framebuffer is
        // then sized to the new viewport's pixel field on the next frame.
        let s = playback_settings(); // ascii: 1x1 subpixels
        let mut src = gif(&s);
        let mut fb = Framebuffer::new(0, 0);

        src.resize(Viewport::new(8, 4)).unwrap();
        assert!(src.next_frame(&mut fb).unwrap());
        assert_eq!((fb.width(), fb.height()), (8, 4));

        src.resize(Viewport::new(16, 6)).unwrap();
        assert!(src.next_frame(&mut fb).unwrap());
        assert_eq!((fb.width(), fb.height()), (16, 6));
    }

    // A deterministic mock source for the control state machine: records every mutation.
    #[derive(Default)]
    struct MockSource {
        total: usize,
        produced: usize,
        restarts: usize,
        seeks: Vec<bool>,
        pause_calls: Vec<bool>,
        resizes: Vec<Viewport>,
        can_seek: bool,
        loops: bool,
    }

    impl PlaybackSource for MockSource {
        fn next_frame(&mut self, target: &mut Framebuffer) -> anyhow::Result<bool> {
            if self.produced >= self.total {
                return Ok(false);
            }
            self.produced += 1;
            target.resize(1, 1);
            Ok(true)
        }
        fn next_wait(&self, _override_fps: Option<f64>) -> Duration {
            Duration::from_millis(10)
        }
        fn restart(&mut self) -> anyhow::Result<()> {
            self.restarts += 1;
            self.produced = 0;
            Ok(())
        }
        fn resize(&mut self, viewport: Viewport) -> anyhow::Result<()> {
            self.resizes.push(viewport);
            Ok(())
        }
        fn can_seek(&self) -> bool {
            self.can_seek
        }
        fn seek(&mut self, forward: bool) -> anyhow::Result<()> {
            self.seeks.push(forward);
            Ok(())
        }
        fn set_paused(&mut self, paused: bool) -> anyhow::Result<()> {
            self.pause_calls.push(paused);
            Ok(())
        }
        fn loops(&self) -> bool {
            self.loops
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: rgfx_terminal::KeyModifiers::NONE,
        }
    }

    #[test]
    fn advances_until_exhausted() {
        let mut c = PlaybackController::new(
            MockSource {
                total: 3,
                ..MockSource::default()
            },
            None,
        );
        let mut fb = Framebuffer::new(0, 0);
        let mut count = 0;
        while c.advance(&mut fb).unwrap() {
            count += 1;
        }
        assert_eq!(count, 3);
    }

    #[test]
    fn pause_holds_frames_then_resume_continues() {
        let mut c = PlaybackController::new(
            MockSource {
                total: 5,
                ..MockSource::default()
            },
            None,
        );
        let mut fb = Framebuffer::new(0, 0);

        // Space pauses: while paused, advance is a no-op that produces no source frame.
        assert_eq!(
            c.on_key(key(KeyCode::Char(' '))).unwrap(),
            Outcome::Continue
        );
        assert!(c.is_paused());
        assert!(
            c.advance(&mut fb).unwrap(),
            "paused advance holds the frame"
        );
        assert!(c.advance(&mut fb).unwrap());

        // Space again resumes and asks the loop to reschedule promptly.
        assert_eq!(
            c.on_key(key(KeyCode::Char(' '))).unwrap(),
            Outcome::Reschedule
        );
        assert!(!c.is_paused());
        assert!(c.advance(&mut fb).unwrap());

        // The source only advanced once (while playing); pause was toggled on then off.
        assert_eq!(c.source.produced, 1);
        assert_eq!(c.source.pause_calls, vec![true, false]);
    }

    #[test]
    fn restart_resets_and_preserves_pause() {
        let mut c = PlaybackController::new(
            MockSource {
                total: 3,
                ..MockSource::default()
            },
            None,
        );
        let mut fb = Framebuffer::new(0, 0);
        c.advance(&mut fb).unwrap();
        c.advance(&mut fb).unwrap();
        assert_eq!(c.on_key(key(KeyCode::Char('R'))).unwrap(), Outcome::Redraw);
        assert_eq!(c.source.restarts, 1);
        assert_eq!(c.source.produced, 0);
    }

    #[test]
    fn seek_only_when_supported() {
        // A non-seekable source ignores arrow keys (keeps playing).
        let mut gif_like = PlaybackController::new(
            MockSource {
                total: 1,
                ..MockSource::default()
            },
            None,
        );
        assert_eq!(
            gif_like.on_key(key(KeyCode::Right)).unwrap(),
            Outcome::Continue
        );
        assert!(gif_like.source.seeks.is_empty());

        // A seekable source seeks and asks for a redraw at the new position.
        let mut video_like = PlaybackController::new(
            MockSource {
                total: 100,
                can_seek: true,
                ..MockSource::default()
            },
            None,
        );
        assert_eq!(
            video_like.on_key(key(KeyCode::Right)).unwrap(),
            Outcome::Redraw
        );
        assert_eq!(
            video_like.on_key(key(KeyCode::Left)).unwrap(),
            Outcome::Redraw
        );
        assert_eq!(video_like.source.seeks, vec![true, false]);
    }

    #[test]
    fn fps_keys_adjust_the_override_and_reschedule() {
        let mut c = PlaybackController::new(
            MockSource {
                total: 1,
                ..MockSource::default()
            },
            Some(30.0),
        );
        assert_eq!(
            c.on_key(key(KeyCode::Char('+'))).unwrap(),
            Outcome::Reschedule
        );
        assert_eq!(c.override_fps(), Some(30.0 + FPS_STEP));
        assert_eq!(
            c.on_key(key(KeyCode::Char('-'))).unwrap(),
            Outcome::Reschedule
        );
        assert_eq!(c.override_fps(), Some(30.0));
        // fps never drops below the clamp floor.
        for _ in 0..1000 {
            c.on_key(key(KeyCode::Char('-'))).unwrap();
        }
        assert_eq!(c.override_fps(), Some(FPS_MIN));
    }

    #[test]
    fn quit_keys_report_quit() {
        let mut c = PlaybackController::new(MockSource::default(), None);
        assert_eq!(c.on_key(key(KeyCode::Esc)).unwrap(), Outcome::Quit);
        assert_eq!(c.on_key(key(KeyCode::Char('q'))).unwrap(), Outcome::Quit);
    }

    #[test]
    fn f_toggles_the_options_bar() {
        let mut c = PlaybackController::new(MockSource::default(), None);
        assert!(c.show_ui(), "bar is shown by default");
        assert_eq!(c.on_key(key(KeyCode::Char('f'))).unwrap(), Outcome::Redraw);
        assert!(!c.show_ui(), "F hides the bar");
        assert_eq!(c.on_key(key(KeyCode::Char('F'))).unwrap(), Outcome::Redraw);
        assert!(c.show_ui(), "F shows it again");
    }

    #[test]
    fn resize_recompute_is_forwarded_to_the_source() {
        let mut c = PlaybackController::new(MockSource::default(), None);
        c.resize(Viewport::new(40, 20)).unwrap();
        c.resize(Viewport::new(80, 24)).unwrap();
        assert_eq!(
            c.source.resizes,
            vec![Viewport::new(40, 20), Viewport::new(80, 24)]
        );
    }

    #[test]
    fn playback_viewport_respects_width_and_bounds() {
        let s = settings(RenderOpts {
            width: Some(40),
            ..RenderOpts::default()
        });
        // Width fixes cols; rows scale to the terminal cell aspect (80x24 => 40x12).
        assert_eq!(
            playback_viewport(&s, Viewport::new(80, 24)),
            Viewport::new(40, 12)
        );
        // With no width, the full terminal bounds are used.
        let d = settings(RenderOpts::default());
        assert_eq!(
            playback_viewport(&d, Viewport::new(100, 30)),
            Viewport::new(100, 30)
        );
    }
}
