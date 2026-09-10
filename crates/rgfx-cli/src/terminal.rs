//! Interactive terminal session for the viewers.
//!
//! [`Session`] wraps [`rgfx_terminal::Terminal`] — the RAII lifecycle guard that enters raw mode
//! plus the alternate screen on open and restores the terminal on every exit path (normal drop,
//! error, or panic). Viewers use it to query the current [`Viewport`], present an encoded frame,
//! and wait for input, without depending on `crossterm` types directly.
//!
//! Event handling is split into two pure, TTY-free functions (`is_quit` and `signal_for`) so
//! the input policy is unit-testable; only [`Session`] itself needs a real terminal.

use std::time::Duration;

use rgfx_core::{TerminalFrame, Viewport};
use rgfx_terminal::{Event, FrameEngine, KeyCode, KeyEvent, Terminal, TerminalOptions};

/// What a polled input event means for a viewer's render loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    /// The user asked to quit; the loop should exit and restore the terminal.
    Quit,
    /// The viewport changed; the loop should re-render.
    Redraw,
    /// Nothing actionable happened; keep idling.
    Idle,
}

/// Whether a key event is a quit request: `q`/`Q`, `Esc`, or `Ctrl+C`.
pub(crate) fn is_quit(key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Esc => true,
        KeyCode::Char('q') | KeyCode::Char('Q') => true,
        KeyCode::Char('c') | KeyCode::Char('C') => key.modifiers.ctrl,
        _ => false,
    }
}

/// Maps an input [`Event`] to the render-loop [`Signal`] it implies.
pub(crate) fn signal_for(event: Event) -> Signal {
    match event {
        Event::Key(key) if is_quit(key) => Signal::Quit,
        Event::Resize(_, _) => Signal::Redraw,
        _ => Signal::Idle,
    }
}

/// A live interactive terminal session that restores the terminal when dropped.
#[derive(Debug)]
pub struct Session {
    term: Terminal,
}

impl Session {
    /// Enters graphics mode on the real terminal (raw mode, alternate screen, hidden cursor).
    ///
    /// Installs the process-wide panic hook so a crash still restores the terminal. Fails cleanly
    /// (returning an error, never a panic) when there is no controlling TTY.
    pub fn open() -> anyhow::Result<Self> {
        Self::open_with(false)
    }

    /// Like [`Session::open`] but also captures mouse events (drag, wheel) for viewers that use
    /// them — e.g. the 3D viewer's drag-to-orbit. Capturing the mouse takes over the terminal's
    /// native text selection while the session is live.
    pub fn open_with_mouse() -> anyhow::Result<Self> {
        Self::open_with(true)
    }

    fn open_with(mouse_capture: bool) -> anyhow::Result<Self> {
        let term = Terminal::new(TerminalOptions {
            mouse_capture,
            ..TerminalOptions::default()
        })?;
        Ok(Self { term })
    }

    /// The current terminal size in character cells.
    pub fn viewport(&self) -> anyhow::Result<Viewport> {
        Ok(self.term.size()?)
    }

    /// Presents an encoded frame: homes the cursor and clears the screen, then writes the text as
    /// a single batched flush so the frame appears without tearing.
    pub fn present(&mut self, text: &str) -> anyhow::Result<()> {
        self.term.present_raw(&format!("\x1b[H\x1b[2J{text}"))?;
        Ok(())
    }

    /// Waits up to `timeout` for an input event and classifies it. `Idle` on timeout.
    pub fn wait(&mut self, timeout: Duration) -> anyhow::Result<Signal> {
        Ok(match self.term.poll_event(timeout)? {
            Some(event) => signal_for(event),
            None => Signal::Idle,
        })
    }

    /// Waits up to `timeout` for the next raw input [`Event`], returning `None` on timeout.
    ///
    /// Unlike [`Session::wait`], this hands the full event to the caller so an interactive viewer
    /// (e.g. the 3D viewer) can act on individual keys rather than the reduced [`Signal`] set.
    pub fn poll_event(&mut self, timeout: Duration) -> anyhow::Result<Option<Event>> {
        Ok(self.term.poll_event(timeout)?)
    }

    /// Presents `frame` through the caller-owned [`FrameEngine`], writing only the cells that
    /// changed since the previous frame (a full redraw on the first frame or after a resize).
    ///
    /// The engine and its buffers are owned by the caller and reused across frames, so steady-state
    /// rendering performs no per-frame allocation.
    pub fn render_frame(
        &mut self,
        engine: &mut FrameEngine,
        frame: &TerminalFrame,
    ) -> anyhow::Result<()> {
        engine.render(frame, self.term.writer_mut().get_mut())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rgfx_terminal::KeyModifiers;

    fn key(code: KeyCode, ctrl: bool) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers {
                ctrl,
                ..KeyModifiers::NONE
            },
        }
    }

    #[test]
    fn quit_keys_are_recognized() {
        assert!(is_quit(key(KeyCode::Esc, false)));
        assert!(is_quit(key(KeyCode::Char('q'), false)));
        assert!(is_quit(key(KeyCode::Char('Q'), false)));
        assert!(is_quit(key(KeyCode::Char('c'), true)));
        // Plain 'c' without Ctrl is not a quit.
        assert!(!is_quit(key(KeyCode::Char('c'), false)));
        assert!(!is_quit(key(KeyCode::Char('x'), false)));
    }

    #[test]
    fn resize_signals_redraw_and_keys_map_through() {
        assert_eq!(signal_for(Event::Resize(120, 40)), Signal::Redraw);
        assert_eq!(
            signal_for(Event::Key(key(KeyCode::Esc, false))),
            Signal::Quit
        );
        assert_eq!(
            signal_for(Event::Key(key(KeyCode::Char('x'), false))),
            Signal::Idle
        );
        assert_eq!(signal_for(Event::FocusGained), Signal::Idle);
    }
}
