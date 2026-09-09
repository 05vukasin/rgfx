# 028 — rgfx-ratatui integration

- **Crate:** `rgfx-ratatui` (creates the crate, optional integration)
- **Depends on:** 007 (frame engine), 004 (braille), 001
- **Blocks:** —
- **Branch:** `task/028-rgfx-ratatui-integration`
- **Skills:** `new-crate`, `rgfx-architecture`, `finish-task`

## Goal
Provide an optional crate that lets other TUI apps embed an `rgfx` viewport as a `ratatui`
widget, so a framebuffer can render inside a larger `ratatui` layout.

## Scope / deliverables
1. Scaffold `crates/rgfx-ratatui` (`ratatui` dep, depends on `rgfx-core` + `rgfx-terminal`).
2. An `RgfxWidget` implementing `ratatui::widgets::Widget`/`StatefulWidget`: takes a
   `Framebuffer` (or a `TerminalEncoder` + framebuffer) and renders encoded cells into the
   widget's `Rect` on the ratatui `Buffer`, mapping our cells/colors to ratatui cells.
3. Handle the area/viewport sizing (widget `Rect` → framebuffer render size + subpixel factor).
4. A runnable `examples/ratatui_embed.rs` showing an image or spinning cube inside a ratatui UI.

## Contracts / API
Convert `rgfx` `TerminalFrame` cells → ratatui `Buffer` cells (incl. color). Don't fork the
encoders — reuse `rgfx-terminal`.

## Acceptance criteria
- [ ] `RgfxWidget` renders a known framebuffer into a ratatui `Buffer` with expected glyphs/colors at the right positions.
- [ ] Respects the target `Rect` (clips/sizes correctly).
- [ ] The example compiles and runs. Clippy `-D warnings`; documented.

## Tests required
- [ ] Render into an off-screen ratatui `Buffer` and assert cell contents for a small framebuffer.
- [ ] Rect sizing → framebuffer render-size mapping.

## Out of scope
Full event integration / input routing (the host app owns input).

## Completion
- [ ] Implemented
- [ ] Finish gate green
- [ ] PR opened: <!-- url -->
- [ ] Merged to `main`

**Status:** ⬜ NOT STARTED
