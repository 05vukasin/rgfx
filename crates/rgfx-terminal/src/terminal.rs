//! The RAII terminal lifecycle guard.
//!
//! [`Terminal`] enters "graphics mode" on construction (raw mode → alternate screen → hidden
//! cursor → optional mouse capture) and guarantees the reverse teardown on **every** exit path:
//! normal drop, an error return during setup, and unwinding from a panic (its [`Drop`] runs while
//! the stack unwinds). An optional process-wide panic hook additionally restores the terminal
//! before the panic message is printed, so a crash never leaves the user in a broken raw-mode
//! shell.
//!
//! The mode-control operations are abstracted behind the [`Backend`] trait. Production code uses
//! [`CrosstermBackend`]; tests use a recording mock, so the entire lifecycle — including the
//! "restore exactly once" and "roll back partial setup" guarantees — is verifiable without a real
//! TTY.

use std::io::{self, Stdout, Write};
use std::sync::Once;
use std::time::Duration;

use rgfx_core::{Result, Viewport};

use crate::event::{Event, map_event};
use crate::writer::BufferedWriter;

/// The low-level terminal mode operations [`Terminal`] drives.
///
/// Each method mirrors one reversible piece of terminal state. Implementations should perform the
/// action and report success/failure; [`Terminal`] tracks which steps succeeded so teardown only
/// reverses those. All methods return [`io::Result`] because that is what the underlying terminal
/// APIs surface; [`Terminal`] maps failures into [`rgfx_core::Error`].
pub trait Backend {
    /// Enables raw (unbuffered, no-echo) input mode.
    fn enable_raw_mode(&mut self) -> io::Result<()>;
    /// Restores cooked input mode.
    fn disable_raw_mode(&mut self) -> io::Result<()>;
    /// Switches to the alternate screen buffer.
    fn enter_alternate_screen(&mut self) -> io::Result<()>;
    /// Returns to the primary screen buffer.
    fn leave_alternate_screen(&mut self) -> io::Result<()>;
    /// Hides the text cursor.
    fn hide_cursor(&mut self) -> io::Result<()>;
    /// Shows the text cursor.
    fn show_cursor(&mut self) -> io::Result<()>;
    /// Starts reporting mouse events.
    fn enable_mouse_capture(&mut self) -> io::Result<()>;
    /// Stops reporting mouse events.
    fn disable_mouse_capture(&mut self) -> io::Result<()>;
    /// Reads the current terminal size as an rgfx [`Viewport`] (columns × rows).
    fn size(&self) -> io::Result<Viewport>;
}

/// The production [`Backend`], implemented with `crossterm` against the real terminal.
#[derive(Debug, Default)]
pub struct CrosstermBackend;

impl Backend for CrosstermBackend {
    fn enable_raw_mode(&mut self) -> io::Result<()> {
        crossterm::terminal::enable_raw_mode()
    }

    fn disable_raw_mode(&mut self) -> io::Result<()> {
        crossterm::terminal::disable_raw_mode()
    }

    fn enter_alternate_screen(&mut self) -> io::Result<()> {
        crossterm::execute!(io::stdout(), crossterm::terminal::EnterAlternateScreen)
    }

    fn leave_alternate_screen(&mut self) -> io::Result<()> {
        crossterm::execute!(io::stdout(), crossterm::terminal::LeaveAlternateScreen)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        crossterm::execute!(io::stdout(), crossterm::cursor::Hide)
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        crossterm::execute!(io::stdout(), crossterm::cursor::Show)
    }

    fn enable_mouse_capture(&mut self) -> io::Result<()> {
        crossterm::execute!(io::stdout(), crossterm::event::EnableMouseCapture)
    }

    fn disable_mouse_capture(&mut self) -> io::Result<()> {
        crossterm::execute!(io::stdout(), crossterm::event::DisableMouseCapture)
    }

    fn size(&self) -> io::Result<Viewport> {
        let (cols, rows) = crossterm::terminal::size()?;
        Ok(Viewport::new(cols, rows))
    }
}

/// Best-effort current terminal size in character cells, usable **without** entering graphics
/// mode (no raw mode, no alternate screen).
///
/// Resolution order, first success wins: the controlling terminal's reported size, then the
/// `COLUMNS`/`LINES` environment variables, then a conventional `80×24` fallback. This never
/// fails, so inline rendering always gets a usable viewport even over SSH, in a pipe, or under a
/// terminal that does not answer the size query.
pub fn terminal_size() -> Viewport {
    if let Ok((cols, rows)) = crossterm::terminal::size() {
        if cols > 0 && rows > 0 {
            return Viewport::new(cols, rows);
        }
    }
    let env_dim = |key: &str| {
        std::env::var(key)
            .ok()
            .and_then(|v| v.trim().parse::<u16>().ok())
            .filter(|n| *n > 0)
    };
    if let (Some(cols), Some(rows)) = (env_dim("COLUMNS"), env_dim("LINES")) {
        return Viewport::new(cols, rows);
    }
    Viewport::new(80, 24)
}

/// Configuration for how a [`Terminal`] enters graphics mode.
#[derive(Debug, Clone, Copy)]
pub struct TerminalOptions {
    /// Capture and report mouse events. Off by default.
    pub mouse_capture: bool,
    /// Install a process-wide panic hook that restores the terminal before the panic prints.
    /// On by default. Ignored by [`Terminal::with_backend`], which never touches global hooks.
    pub install_panic_hook: bool,
}

impl Default for TerminalOptions {
    fn default() -> Self {
        Self {
            mouse_capture: false,
            install_panic_hook: true,
        }
    }
}

/// Tracks which reversible steps actually succeeded, so teardown reverses exactly those.
#[derive(Debug, Default, Clone, Copy)]
struct ActiveState {
    raw_mode: bool,
    alt_screen: bool,
    cursor_hidden: bool,
    mouse: bool,
}

/// A safe terminal session.
///
/// Construct with [`Terminal::new`] (real terminal) or [`Terminal::with_backend`] (custom/mock
/// backend). While alive it owns the terminal's graphics mode; dropping it — or unwinding through
/// its scope on panic — restores the terminal to how it was found, in reverse setup order.
///
/// Output goes through an owned [`BufferedWriter`] over stdout: use [`present_raw`](Self::present_raw)
/// for a one-shot batched write, or [`writer_mut`](Self::writer_mut) to let a higher layer (the
/// diff engine) queue bytes and flush once per frame. Nothing here writes to stdout except through
/// that writer.
#[derive(Debug)]
pub struct Terminal<B: Backend = CrosstermBackend> {
    backend: B,
    writer: BufferedWriter<Stdout>,
    options: TerminalOptions,
    active: ActiveState,
    restored: bool,
}

impl Terminal<CrosstermBackend> {
    /// Enters graphics mode on the real terminal using the given options.
    ///
    /// Enables raw mode, switches to the alternate screen, and hides the cursor (plus mouse
    /// capture if requested). If [`TerminalOptions::install_panic_hook`] is set, a panic hook is
    /// installed first. On any setup failure, whatever was already enabled is rolled back and the
    /// error is returned, so a failed constructor never leaks terminal state.
    pub fn new(options: TerminalOptions) -> Result<Self> {
        if options.install_panic_hook {
            install_panic_hook();
        }
        Self::with_backend(CrosstermBackend, options)
    }
}

impl<B: Backend> Terminal<B> {
    /// Enters graphics mode using a caller-provided [`Backend`].
    ///
    /// This is the seam used by tests (with a recording mock) and by any embedder that wants to
    /// drive the lifecycle against something other than the real terminal. Unlike [`Terminal::new`],
    /// it never installs a global panic hook. Partial setup is rolled back on failure.
    pub fn with_backend(backend: B, options: TerminalOptions) -> Result<Self> {
        let mut term = Self {
            backend,
            writer: BufferedWriter::new(io::stdout()),
            options,
            active: ActiveState::default(),
            restored: false,
        };
        if let Err(e) = term.setup() {
            // Roll back anything that did succeed before surfacing the error.
            term.restore();
            return Err(e.into());
        }
        Ok(term)
    }

    fn setup(&mut self) -> io::Result<()> {
        self.backend.enable_raw_mode()?;
        self.active.raw_mode = true;
        self.backend.enter_alternate_screen()?;
        self.active.alt_screen = true;
        self.backend.hide_cursor()?;
        self.active.cursor_hidden = true;
        if self.options.mouse_capture {
            self.backend.enable_mouse_capture()?;
            self.active.mouse = true;
        }
        Ok(())
    }

    /// Restores the terminal, reversing setup order. Idempotent: runs its effects at most once.
    ///
    /// Best-effort — teardown errors are swallowed, because this runs from [`Drop`] (and possibly
    /// during unwinding) where returning an error is impossible and leaving *some* state restored
    /// is strictly better than aborting on the first failure.
    fn restore(&mut self) {
        if self.restored {
            return;
        }
        self.restored = true;
        if self.active.mouse {
            let _ = self.backend.disable_mouse_capture();
        }
        if self.active.cursor_hidden {
            let _ = self.backend.show_cursor();
        }
        if self.active.alt_screen {
            let _ = self.backend.leave_alternate_screen();
        }
        if self.active.raw_mode {
            let _ = self.backend.disable_raw_mode();
        }
    }

    /// The current terminal size as a [`Viewport`] in character cells.
    ///
    /// These are terminal-cell dimensions, kept deliberately separate from framebuffer pixel
    /// dimensions; use [`Viewport::render_size`] to derive the pixel size an encoder needs.
    pub fn size(&self) -> Result<Viewport> {
        Ok(self.backend.size()?)
    }

    /// Polls for the next input event, waiting up to `timeout`.
    ///
    /// Returns `Ok(None)` if no event arrived within the timeout, or if the event was one rgfx
    /// filters out (e.g. a key release). Events are mapped into the rgfx-local [`Event`] enum so
    /// callers never depend on `crossterm` types.
    pub fn poll_event(&mut self, timeout: Duration) -> Result<Option<Event>> {
        if crossterm::event::poll(timeout)? {
            let raw = crossterm::event::read()?;
            Ok(map_event(raw))
        } else {
            Ok(None)
        }
    }

    /// Writes `s` to the terminal as a single batched frame: queue then flush once.
    pub fn present_raw(&mut self, s: &str) -> Result<()> {
        self.writer.queue(s);
        self.writer.flush()?;
        Ok(())
    }

    /// Mutable access to the buffered output writer.
    ///
    /// The frame-diff engine (task 007) queues its per-cell escape sequences here and calls
    /// [`BufferedWriter::flush`] exactly once per frame.
    pub fn writer_mut(&mut self) -> &mut BufferedWriter<Stdout> {
        &mut self.writer
    }

    /// Whether mouse capture is currently active.
    pub fn mouse_captured(&self) -> bool {
        self.active.mouse
    }
}

impl<B: Backend> Drop for Terminal<B> {
    fn drop(&mut self) {
        // Best-effort flush of anything still queued, then restore the terminal. Runs on normal
        // drop and while unwinding from a panic.
        let _ = self.writer.flush();
        self.restore();
    }
}

/// Installs a process-wide panic hook that best-effort restores the terminal before printing.
///
/// Chains to the previously installed hook, so panic messages/backtraces still appear — but only
/// after raw mode, the alternate screen, the cursor, and mouse capture have been restored, so the
/// message is legible and the shell is usable. Installed at most once per process (subsequent
/// calls are no-ops). [`Terminal::new`] calls this for you unless disabled via
/// [`TerminalOptions::install_panic_hook`].
pub fn install_panic_hook() {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let mut stdout = io::stdout();
            let _ = crossterm::execute!(
                stdout,
                crossterm::event::DisableMouseCapture,
                crossterm::cursor::Show,
                crossterm::terminal::LeaveAlternateScreen,
            );
            let _ = crossterm::terminal::disable_raw_mode();
            let _ = stdout.flush();
            previous(info);
        }));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{self, AssertUnwindSafe};
    use std::sync::{Arc, Mutex};

    /// A recording backend: every operation appends an [`Op`] to a shared log, so tests can
    /// assert ordering and call counts without a TTY. Optionally fails at a chosen step to
    /// exercise the partial-setup rollback path.
    #[derive(Clone)]
    struct MockBackend {
        log: Arc<Mutex<Vec<Op>>>,
        fail_at: Option<Op>,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Op {
        EnableRaw,
        EnterAlt,
        HideCursor,
        EnableMouse,
        DisableMouse,
        ShowCursor,
        LeaveAlt,
        DisableRaw,
    }

    impl MockBackend {
        fn new() -> Self {
            Self {
                log: Arc::new(Mutex::new(Vec::new())),
                fail_at: None,
            }
        }

        fn failing_at(op: Op) -> Self {
            Self {
                log: Arc::new(Mutex::new(Vec::new())),
                fail_at: Some(op),
            }
        }

        fn record(&self, op: Op) -> io::Result<()> {
            self.log.lock().unwrap().push(op);
            if self.fail_at == Some(op) {
                return Err(io::Error::other("simulated failure"));
            }
            Ok(())
        }
    }

    impl Backend for MockBackend {
        fn enable_raw_mode(&mut self) -> io::Result<()> {
            self.record(Op::EnableRaw)
        }
        fn disable_raw_mode(&mut self) -> io::Result<()> {
            self.record(Op::DisableRaw)
        }
        fn enter_alternate_screen(&mut self) -> io::Result<()> {
            self.record(Op::EnterAlt)
        }
        fn leave_alternate_screen(&mut self) -> io::Result<()> {
            self.record(Op::LeaveAlt)
        }
        fn hide_cursor(&mut self) -> io::Result<()> {
            self.record(Op::HideCursor)
        }
        fn show_cursor(&mut self) -> io::Result<()> {
            self.record(Op::ShowCursor)
        }
        fn enable_mouse_capture(&mut self) -> io::Result<()> {
            self.record(Op::EnableMouse)
        }
        fn disable_mouse_capture(&mut self) -> io::Result<()> {
            self.record(Op::DisableMouse)
        }
        fn size(&self) -> io::Result<Viewport> {
            Ok(Viewport::new(80, 24))
        }
    }

    fn opts(mouse: bool) -> TerminalOptions {
        TerminalOptions {
            mouse_capture: mouse,
            install_panic_hook: false,
        }
    }

    #[test]
    fn setup_order_is_raw_alt_cursor() {
        let backend = MockBackend::new();
        let log = backend.log.clone();
        let term = Terminal::with_backend(backend, opts(false)).unwrap();
        assert_eq!(
            *log.lock().unwrap(),
            vec![Op::EnableRaw, Op::EnterAlt, Op::HideCursor]
        );
        drop(term);
    }

    #[test]
    fn mouse_capture_setup_included_when_requested() {
        let backend = MockBackend::new();
        let log = backend.log.clone();
        let term = Terminal::with_backend(backend, opts(true)).unwrap();
        assert_eq!(
            *log.lock().unwrap(),
            vec![Op::EnableRaw, Op::EnterAlt, Op::HideCursor, Op::EnableMouse]
        );
        assert!(term.mouse_captured());
        drop(term);
    }

    #[test]
    fn teardown_reverses_setup_order_on_drop() {
        let backend = MockBackend::new();
        let log = backend.log.clone();
        let term = Terminal::with_backend(backend, opts(true)).unwrap();
        drop(term);
        assert_eq!(
            *log.lock().unwrap(),
            vec![
                // setup
                Op::EnableRaw,
                Op::EnterAlt,
                Op::HideCursor,
                Op::EnableMouse,
                // teardown, reversed
                Op::DisableMouse,
                Op::ShowCursor,
                Op::LeaveAlt,
                Op::DisableRaw,
            ]
        );
    }

    #[test]
    fn restore_runs_exactly_once_across_drop() {
        let backend = MockBackend::new();
        let log = backend.log.clone();
        let term = Terminal::with_backend(backend, opts(false)).unwrap();
        drop(term);
        let ops = log.lock().unwrap();
        // Each teardown op appears exactly once even though Drop and the internal guard both run.
        assert_eq!(ops.iter().filter(|o| **o == Op::DisableRaw).count(), 1);
        assert_eq!(ops.iter().filter(|o| **o == Op::ShowCursor).count(), 1);
        assert_eq!(ops.iter().filter(|o| **o == Op::LeaveAlt).count(), 1);
    }

    #[test]
    fn restore_runs_exactly_once_on_panic_unwind() {
        let backend = MockBackend::new();
        let log = backend.log.clone();
        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            let _term = Terminal::with_backend(backend, opts(false)).unwrap();
            panic!("boom");
        }));
        assert!(result.is_err(), "the closure should have panicked");
        let ops = log.lock().unwrap();
        // Terminal dropped during unwinding -> teardown ran once.
        assert_eq!(ops.iter().filter(|o| **o == Op::DisableRaw).count(), 1);
        assert_eq!(ops.iter().filter(|o| **o == Op::ShowCursor).count(), 1);
        assert_eq!(ops.iter().filter(|o| **o == Op::LeaveAlt).count(), 1);
    }

    #[test]
    fn failed_setup_rolls_back_and_leaks_nothing() {
        // Fail while entering the alternate screen: raw mode is already on and must be undone.
        let backend = MockBackend::failing_at(Op::EnterAlt);
        let log = backend.log.clone();
        let err = Terminal::with_backend(backend, opts(false));
        assert!(err.is_err(), "setup should surface the error");
        let ops = log.lock().unwrap();
        assert_eq!(
            *ops,
            vec![
                Op::EnableRaw,  // succeeded
                Op::EnterAlt,   // attempted, failed
                Op::DisableRaw  // rolled back the one step that had succeeded
            ]
        );
        // No cursor/alt teardown because those were never enabled -> no leak, no spurious ops.
        assert!(!ops.contains(&Op::ShowCursor));
        assert!(!ops.contains(&Op::LeaveAlt));
    }

    #[test]
    fn failed_mouse_setup_rolls_back_prior_steps() {
        let backend = MockBackend::failing_at(Op::EnableMouse);
        let log = backend.log.clone();
        let err = Terminal::with_backend(backend, opts(true));
        assert!(err.is_err());
        assert_eq!(
            *log.lock().unwrap(),
            vec![
                Op::EnableRaw,
                Op::EnterAlt,
                Op::HideCursor,
                Op::EnableMouse, // failed
                // full rollback in reverse
                Op::ShowCursor,
                Op::LeaveAlt,
                Op::DisableRaw,
            ]
        );
    }

    #[test]
    fn size_reads_from_backend() {
        let term = Terminal::with_backend(MockBackend::new(), opts(false)).unwrap();
        assert_eq!(term.size().unwrap(), Viewport::new(80, 24));
    }
}
