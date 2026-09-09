//! Embed an rgfx viewport inside a larger `ratatui` UI.
//!
//! This runnable example builds a normal `ratatui` layout — a title bar, a bordered panel, and a
//! footer — and renders an animated rgfx [`Framebuffer`] into the panel as an [`RgfxWidget`]. A
//! coloured disc bounces around the framebuffer; the flagship [`BrailleEncoder`] turns it into
//! Braille cells that are copied straight into `ratatui`'s buffer, so the graphic composes with
//! the surrounding chrome instead of owning the whole screen.
//!
//! Run it with:
//!
//! ```text
//! cargo run -p rgfx-ratatui --example ratatui_embed
//! ```
//!
//! Press `q` or `Esc` to quit.

use std::io;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{self, Event, KeyCode};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::{DefaultTerminal, Frame};

use rgfx_core::{Color, Framebuffer};
use rgfx_ratatui::RgfxWidget;
use rgfx_terminal::{BrailleEncoder, BrailleOptions, ColorMode};

/// The animated scene: a coloured disc bouncing inside the framebuffer.
struct Scene {
    /// Disc center, in framebuffer pixels.
    pos: (f32, f32),
    /// Disc velocity, in framebuffer pixels per second.
    vel: (f32, f32),
    /// Elapsed seconds, used to cycle the disc's hue.
    t: f32,
}

impl Scene {
    fn new() -> Self {
        Self {
            pos: (20.0, 20.0),
            vel: (48.0, 33.0),
            t: 0.0,
        }
    }

    /// Advances the simulation by `dt` seconds within a `w × h` pixel field.
    fn step(&mut self, dt: f32, w: usize, h: usize) {
        self.t += dt;
        let (w, h) = (w as f32, h as f32);
        let r = 8.0;
        self.pos.0 += self.vel.0 * dt;
        self.pos.1 += self.vel.1 * dt;
        if self.pos.0 < r || self.pos.0 > (w - r).max(r) {
            self.vel.0 = -self.vel.0;
            self.pos.0 = self.pos.0.clamp(r, (w - r).max(r));
        }
        if self.pos.1 < r || self.pos.1 > (h - r).max(r) {
            self.vel.1 = -self.vel.1;
            self.pos.1 = self.pos.1.clamp(r, (h - r).max(r));
        }
    }

    /// Draws the current frame into `fb` (already sized to the target render resolution).
    fn draw(&self, fb: &mut Framebuffer) {
        fb.clear(Color::BLACK);
        let (w, h) = (fb.width(), fb.height());
        if w == 0 || h == 0 {
            return;
        }
        let r = 8.0;
        // A slowly cycling hue keeps the truecolor path visible.
        let hue = |phase: f32| 0.5 + 0.5 * (self.t + phase).sin();
        let disc = Color::rgb(hue(0.0), hue(2.0), hue(4.0));
        let (cx, cy) = self.pos;
        let (x0, x1) = ((cx - r).max(0.0) as usize, ((cx + r) as usize).min(w - 1));
        let (y0, y1) = ((cy - r).max(0.0) as usize, ((cy + r) as usize).min(h - 1));
        for y in y0..=y1 {
            for x in x0..=x1 {
                let (dx, dy) = (x as f32 - cx, y as f32 - cy);
                if dx * dx + dy * dy <= r * r {
                    fb.set(x, y, disc);
                }
            }
        }
    }
}

fn main() -> io::Result<()> {
    let terminal = ratatui::init();
    let result = run(terminal);
    ratatui::restore();
    result
}

fn run(mut terminal: DefaultTerminal) -> io::Result<()> {
    // A truecolor Braille encoder; the framebuffer is reused across frames (never per-frame alloc).
    let encoder = BrailleEncoder::with_options(BrailleOptions {
        color: ColorMode::TrueColor,
        ..Default::default()
    });
    let mut fb = Framebuffer::new(0, 0);
    let mut scene = Scene::new();
    let mut last = Instant::now();

    loop {
        let now = Instant::now();
        let dt = (now - last).as_secs_f32();
        last = now;

        terminal.draw(|frame| ui(frame, &encoder, &mut fb, &mut scene, dt))?;

        // ~60 fps cap; quit on q/Esc.
        if event::poll(Duration::from_millis(16))? {
            if let Event::Key(key) = event::read()? {
                if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) {
                    return Ok(());
                }
            }
        }
    }
}

fn ui(
    frame: &mut Frame,
    encoder: &BrailleEncoder,
    fb: &mut Framebuffer,
    scene: &mut Scene,
    dt: f32,
) {
    let [title, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    frame.render_widget(
        Paragraph::new(Line::from("rgfx inside ratatui").bold()).centered(),
        title,
    );
    frame.render_widget(
        Paragraph::new(Line::from("press q or Esc to quit").dim()).centered(),
        footer,
    );

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" rgfx viewport ")
        .style(Style::default());
    let inner = block.inner(body);
    frame.render_widget(block, body);

    // Size the reused framebuffer to fill the inner Rect at Braille density, then render.
    let widget = RgfxWidget::new(encoder, fb).color_mode(ColorMode::TrueColor);
    let (w, h) = widget.render_size(inner);
    if (fb.width(), fb.height()) != (w, h) {
        fb.resize(w, h);
    }
    scene.step(dt.min(0.1), w, h);
    scene.draw(fb);

    // Rebuild the widget borrowing the now-filled framebuffer and render it into the panel.
    frame.render_widget(
        RgfxWidget::new(encoder, fb).color_mode(ColorMode::TrueColor),
        inner,
    );
}
