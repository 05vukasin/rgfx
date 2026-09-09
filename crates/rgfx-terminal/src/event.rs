//! rgfx-local input events and the mapping from `crossterm`'s event types.
//!
//! Downstream crates (the CLI, interactive previews) consume [`Event`] and never see a
//! `crossterm` type. Keeping the vocabulary local means the terminal backend can be swapped
//! without touching every call site, and event handling stays unit-testable without a TTY —
//! [`map_event`] is a pure function over constructed `crossterm` values.

use crossterm::event as ct;

/// A keyboard modifier set, flattened from `crossterm`'s bitflags into plain booleans.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyModifiers {
    /// The Shift key was held.
    pub shift: bool,
    /// The Control key was held.
    pub ctrl: bool,
    /// The Alt (Option) key was held.
    pub alt: bool,
}

impl KeyModifiers {
    /// A modifier set with nothing held.
    pub const NONE: Self = Self {
        shift: false,
        ctrl: false,
        alt: false,
    };

    fn from_crossterm(m: ct::KeyModifiers) -> Self {
        Self {
            shift: m.contains(ct::KeyModifiers::SHIFT),
            ctrl: m.contains(ct::KeyModifiers::CONTROL),
            alt: m.contains(ct::KeyModifiers::ALT),
        }
    }
}

/// A logical key, independent of the `crossterm` representation.
///
/// The common editing and navigation keys are named explicitly; anything rgfx does not model
/// (media keys, modifier-only presses, lock keys, …) maps to [`KeyCode::Unknown`] so callers can
/// pattern-match exhaustively without depending on the full `crossterm` surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyCode {
    /// A printable character.
    Char(char),
    /// The Enter/Return key.
    Enter,
    /// The Escape key.
    Esc,
    /// The Backspace key.
    Backspace,
    /// The Tab key.
    Tab,
    /// A back-tab (Shift+Tab).
    BackTab,
    /// The Delete (forward-delete) key.
    Delete,
    /// The Insert key.
    Insert,
    /// The Left arrow.
    Left,
    /// The Right arrow.
    Right,
    /// The Up arrow.
    Up,
    /// The Down arrow.
    Down,
    /// The Home key.
    Home,
    /// The End key.
    End,
    /// The Page Up key.
    PageUp,
    /// The Page Down key.
    PageDown,
    /// A function key, e.g. `F(1)` for F1.
    F(u8),
    /// A null byte key event.
    Null,
    /// Any key rgfx does not model explicitly.
    Unknown,
}

/// A keyboard event: which key, and which modifiers were held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    /// The logical key that was pressed.
    pub code: KeyCode,
    /// The modifier keys held during the press.
    pub modifiers: KeyModifiers,
}

/// A mouse button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    /// The left button.
    Left,
    /// The right button.
    Right,
    /// The middle button.
    Middle,
}

/// The kind of a mouse interaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseEventKind {
    /// A button was pressed.
    Down(MouseButton),
    /// A button was released.
    Up(MouseButton),
    /// A button is held and the pointer moved.
    Drag(MouseButton),
    /// The pointer moved with no button held.
    Moved,
    /// The wheel scrolled up.
    ScrollUp,
    /// The wheel scrolled down.
    ScrollDown,
    /// The wheel scrolled left.
    ScrollLeft,
    /// The wheel scrolled right.
    ScrollRight,
}

/// A mouse event at a cell coordinate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MouseEvent {
    /// What happened.
    pub kind: MouseEventKind,
    /// The column (0-based) where it happened.
    pub col: u16,
    /// The row (0-based) where it happened.
    pub row: u16,
    /// The modifier keys held during the event.
    pub modifiers: KeyModifiers,
}

/// An input event delivered by [`Terminal::poll_event`].
///
/// This is the rgfx-local mirror of the `crossterm` event set, minus the events rgfx does not
/// act on (key releases are filtered out so a single physical press yields exactly one event).
///
/// [`Terminal::poll_event`]: crate::Terminal::poll_event
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// A key was pressed (or auto-repeated).
    Key(KeyEvent),
    /// A mouse interaction occurred.
    Mouse(MouseEvent),
    /// The terminal was resized, carrying the new `(cols, rows)`.
    Resize(u16, u16),
    /// The terminal window gained focus.
    FocusGained,
    /// The terminal window lost focus.
    FocusLost,
    /// A bracketed-paste delivered the given text.
    Paste(String),
}

fn map_key_code(code: ct::KeyCode) -> KeyCode {
    match code {
        ct::KeyCode::Char(c) => KeyCode::Char(c),
        ct::KeyCode::Enter => KeyCode::Enter,
        ct::KeyCode::Esc => KeyCode::Esc,
        ct::KeyCode::Backspace => KeyCode::Backspace,
        ct::KeyCode::Tab => KeyCode::Tab,
        ct::KeyCode::BackTab => KeyCode::BackTab,
        ct::KeyCode::Delete => KeyCode::Delete,
        ct::KeyCode::Insert => KeyCode::Insert,
        ct::KeyCode::Left => KeyCode::Left,
        ct::KeyCode::Right => KeyCode::Right,
        ct::KeyCode::Up => KeyCode::Up,
        ct::KeyCode::Down => KeyCode::Down,
        ct::KeyCode::Home => KeyCode::Home,
        ct::KeyCode::End => KeyCode::End,
        ct::KeyCode::PageUp => KeyCode::PageUp,
        ct::KeyCode::PageDown => KeyCode::PageDown,
        ct::KeyCode::F(n) => KeyCode::F(n),
        ct::KeyCode::Null => KeyCode::Null,
        _ => KeyCode::Unknown,
    }
}

fn map_mouse_button(b: ct::MouseButton) -> MouseButton {
    match b {
        ct::MouseButton::Left => MouseButton::Left,
        ct::MouseButton::Right => MouseButton::Right,
        ct::MouseButton::Middle => MouseButton::Middle,
    }
}

fn map_mouse_kind(kind: ct::MouseEventKind) -> MouseEventKind {
    match kind {
        ct::MouseEventKind::Down(b) => MouseEventKind::Down(map_mouse_button(b)),
        ct::MouseEventKind::Up(b) => MouseEventKind::Up(map_mouse_button(b)),
        ct::MouseEventKind::Drag(b) => MouseEventKind::Drag(map_mouse_button(b)),
        ct::MouseEventKind::Moved => MouseEventKind::Moved,
        ct::MouseEventKind::ScrollUp => MouseEventKind::ScrollUp,
        ct::MouseEventKind::ScrollDown => MouseEventKind::ScrollDown,
        ct::MouseEventKind::ScrollLeft => MouseEventKind::ScrollLeft,
        ct::MouseEventKind::ScrollRight => MouseEventKind::ScrollRight,
    }
}

/// Maps a `crossterm` event into an rgfx-local [`Event`].
///
/// Returns `None` for events rgfx intentionally ignores — currently key *release* events, so a
/// single physical key press produces exactly one [`Event::Key`]. This function is pure and does
/// not touch the terminal, which is what makes the input layer testable without a real TTY.
pub fn map_event(event: ct::Event) -> Option<Event> {
    match event {
        ct::Event::Key(k) => {
            // Only surface presses and auto-repeats; drop releases to avoid double events.
            if matches!(k.kind, ct::KeyEventKind::Release) {
                return None;
            }
            Some(Event::Key(KeyEvent {
                code: map_key_code(k.code),
                modifiers: KeyModifiers::from_crossterm(k.modifiers),
            }))
        }
        ct::Event::Mouse(m) => Some(Event::Mouse(MouseEvent {
            kind: map_mouse_kind(m.kind),
            col: m.column,
            row: m.row,
            modifiers: KeyModifiers::from_crossterm(m.modifiers),
        })),
        ct::Event::Resize(cols, rows) => Some(Event::Resize(cols, rows)),
        ct::Event::FocusGained => Some(Event::FocusGained),
        ct::Event::FocusLost => Some(Event::FocusLost),
        ct::Event::Paste(s) => Some(Event::Paste(s)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_char_key_with_modifiers() {
        let ev = ct::Event::Key(ct::KeyEvent::new(
            ct::KeyCode::Char('q'),
            ct::KeyModifiers::CONTROL,
        ));
        let mapped = map_event(ev).expect("press should map");
        assert_eq!(
            mapped,
            Event::Key(KeyEvent {
                code: KeyCode::Char('q'),
                modifiers: KeyModifiers {
                    ctrl: true,
                    ..KeyModifiers::NONE
                },
            })
        );
    }

    #[test]
    fn maps_named_and_function_keys() {
        assert_eq!(
            map_event(ct::Event::Key(ct::KeyEvent::new(
                ct::KeyCode::Esc,
                ct::KeyModifiers::NONE
            ))),
            Some(Event::Key(KeyEvent {
                code: KeyCode::Esc,
                modifiers: KeyModifiers::NONE
            }))
        );
        assert_eq!(
            map_event(ct::Event::Key(ct::KeyEvent::new(
                ct::KeyCode::F(5),
                ct::KeyModifiers::NONE
            ))),
            Some(Event::Key(KeyEvent {
                code: KeyCode::F(5),
                modifiers: KeyModifiers::NONE
            }))
        );
    }

    #[test]
    fn unmodeled_key_maps_to_unknown() {
        let ev = ct::Event::Key(ct::KeyEvent::new(
            ct::KeyCode::CapsLock,
            ct::KeyModifiers::NONE,
        ));
        assert_eq!(
            map_event(ev),
            Some(Event::Key(KeyEvent {
                code: KeyCode::Unknown,
                modifiers: KeyModifiers::NONE
            }))
        );
    }

    #[test]
    fn key_release_is_filtered_out() {
        let ev = ct::Event::Key(ct::KeyEvent::new_with_kind(
            ct::KeyCode::Char('a'),
            ct::KeyModifiers::NONE,
            ct::KeyEventKind::Release,
        ));
        assert_eq!(map_event(ev), None);
    }

    #[test]
    fn maps_resize() {
        assert_eq!(
            map_event(ct::Event::Resize(120, 40)),
            Some(Event::Resize(120, 40))
        );
    }

    #[test]
    fn maps_mouse_down_and_scroll() {
        let down = ct::Event::Mouse(ct::MouseEvent {
            kind: ct::MouseEventKind::Down(ct::MouseButton::Left),
            column: 10,
            row: 3,
            modifiers: ct::KeyModifiers::NONE,
        });
        assert_eq!(
            map_event(down),
            Some(Event::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                col: 10,
                row: 3,
                modifiers: KeyModifiers::NONE,
            }))
        );

        let scroll = ct::Event::Mouse(ct::MouseEvent {
            kind: ct::MouseEventKind::ScrollUp,
            column: 0,
            row: 0,
            modifiers: ct::KeyModifiers::NONE,
        });
        assert!(matches!(
            map_event(scroll),
            Some(Event::Mouse(MouseEvent {
                kind: MouseEventKind::ScrollUp,
                ..
            }))
        ));
    }

    #[test]
    fn maps_focus_and_paste() {
        assert_eq!(map_event(ct::Event::FocusGained), Some(Event::FocusGained));
        assert_eq!(map_event(ct::Event::FocusLost), Some(Event::FocusLost));
        assert_eq!(
            map_event(ct::Event::Paste("hi".to_string())),
            Some(Event::Paste("hi".to_string()))
        );
    }
}
