//! Terminal setup/teardown with guaranteed cleanup.
//!
//! The real terminal I/O (raw mode, alternate screen, cursor hiding) lives in `rgfx-terminal`,
//! which is being built in parallel. Until this crate depends on it (a later task), this module
//! provides the *shape* of the lifecycle: a [`TerminalGuard`] that "enters" on construction and
//! "leaves" on `Drop`, so cleanup runs on every exit path — normal return, `?` error unwinding,
//! and panic. Task 020 owns dispatch, not rendering, so the actual escape sequences are stubbed
//! and only traced.

use rgfx_core::Viewport;

/// A RAII guard around the terminal's raw/alternate-screen state.
///
/// Construct it with [`TerminalGuard::enter`] before rendering; when it drops (including during
/// unwind), [`TerminalGuard::leave`] runs, restoring the terminal. Because teardown is in
/// `Drop`, a panic or an early `?` still restores the terminal.
#[derive(Debug)]
pub struct TerminalGuard {
    viewport: Viewport,
    active: bool,
}

impl TerminalGuard {
    /// Enters "rendering mode".
    ///
    /// In the full implementation this enables raw mode, switches to the alternate screen and
    /// hides the cursor via `rgfx-terminal`. Here it only records the viewport and traces the
    /// transition.
    pub fn enter() -> anyhow::Result<Self> {
        let viewport = query_viewport();
        tracing::debug!(
            cols = viewport.cols,
            rows = viewport.rows,
            "terminal: enter (stub)"
        );
        Ok(TerminalGuard {
            viewport,
            active: true,
        })
    }

    /// The current terminal viewport in character cells.
    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    /// Explicit teardown. Idempotent; `Drop` also calls this.
    pub fn leave(&mut self) {
        if self.active {
            self.active = false;
            tracing::debug!("terminal: leave (stub)");
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.leave();
    }
}

/// Queries the terminal size, falling back to a conventional 80×24 when it is unavailable
/// (not a TTY, or output redirected to a file).
///
/// This is intentionally dependency-free for the skeleton; the real size query moves to
/// `rgfx-terminal` in a later task.
pub fn query_viewport() -> Viewport {
    fn parse_env(key: &str) -> Option<u16> {
        std::env::var(key).ok()?.parse().ok()
    }
    let cols = parse_env("COLUMNS").filter(|&c| c > 0).unwrap_or(80);
    let rows = parse_env("LINES").filter(|&r| r > 0).unwrap_or(24);
    Viewport::new(cols, rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_enters_and_leaves() {
        let mut g = TerminalGuard::enter().unwrap();
        assert!(g.active);
        g.leave();
        assert!(!g.active);
        // Second leave is a no-op.
        g.leave();
        assert!(!g.active);
    }

    #[test]
    fn viewport_has_positive_dimensions() {
        let vp = query_viewport();
        assert!(vp.cols > 0 && vp.rows > 0);
    }
}
