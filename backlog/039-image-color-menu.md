# 039 — Image color menu (ANSI color for the image preview)

- **Crate:** `rgfx-cli` (image viewer) — reuses `rgfx-terminal` color (task 006)
- **Depends on:** 032 (full-screen image preview), 006 (ANSI color), 033 (viewer_chrome panel)
- **Branch:** `task/039-image-color-menu`
- **Skill:** invoke **`rgfx-fix`** first (required; the hook blocks `crates/**` edits until then).

## Motivation (user request)
Add a **color menu** to the image preview so the user can turn on ANSI color (and pick the
fidelity) — at minimum to read contours/regions better than pure grayscale.

## Context
The encoders already support color: `BrailleOptions`/`AsciiOptions`/`BlockOptions` carry a
`color: ColorMode` (None/Ansi16/Ansi256/TrueColor), the `AnsiSerializer` + `FrameEngine` emit the
escapes, and `detect_color_mode()` picks the terminal's capability. The block encoder gives the
richest color (fg=top pixel, bg=bottom pixel → two colors per cell). Today the image viewer only
exposes a `C` on/off toggle at the terminal's detected mode; make it a discoverable menu with
fidelity control, and re-render live.

## Design — a modal color menu (mirror the light menu, task 035)
- **`C` opens/closes a modal Color menu** (replacing today's quick `C` toggle; the on/off moves
  inside). Drawn via `viewer_chrome::overlay_panel`. While open, keys drive color; while closed,
  the other image controls (`R` renderer, `D` dither, `I` invert, `F` ui) behave as now.
  - `O`/Space: color on/off
  - `M`: cycle color mode — Ansi16 → Ansi256 → TrueColor (clamp to what the terminal supports;
    show the detected max)
  - `B`: quick-pick the **blocks** renderer (best color: fg/bg per cell) — optional convenience
  - `+`/`-`: contrast (helps contours) — reuse the existing tone stage
  - `R` reset · `Esc`/`C` close
- Wire the chosen `ColorMode` into both the encoder options AND the `FrameEngine`'s serializer
  mode; when the mode changes, rebuild/retune the engine and force a full redraw so the new
  escapes take effect. When color is off, fall back to the grayscale path (byte-identical to
  today). Status bar shows `color:<mode|off>`.
- Keep `--color` (and, if easy, add `--color-mode <16|256|true>`) as the non-interactive seed.

## Acceptance criteria
- [x] `C` opens a color menu; toggling on renders the image in ANSI color; `M` cycles fidelity;
      blocks renderer shows two colors per cell; `Esc`/`C` closes; other controls resume.
- [x] Color off is byte-identical to the current grayscale output.
- [x] Mode is clamped to the terminal's detected capability (no truecolor escapes on a 16-color TERM).
- [x] `--output` stays deterministic; clippy `-D warnings` clean.

## Tests required
- [x] Menu state machine: open routes keys to color (on/off, mode cycle, contrast); closed leaves
      the existing image controls; reset restores defaults.
- [x] Encoder + engine receive the selected `ColorMode`; a small colored fixture renders cells
      with fg/bg color when on, none when off.
- [x] Mode clamp against a detected capability (e.g. requesting TrueColor on an Ansi16 terminal
      yields Ansi16 escapes).

## Out of scope
Palette editing, custom LUTs, dithering-in-color specifics (existing dither still applies to luma).

## Completion
- [x] Implemented · [x] Gate green + PTY check · [x] PR opened · [ ] Merged

**Status:** 🟦 IN REVIEW (branch `task/039-color-b`)

### Notes
- `rgfx-cli`: replaced the image viewer's `color: bool` with a `ColorState` (on / requested
  fidelity / detected ceiling / menu_open) on `ImageState`. `ImageState::engine_mode()` returns
  the effective `ColorMode` (clamped fidelity when on, `None` when off) which feeds both the
  encoder and the `FrameEngine` serializer; `run_fullscreen` rebuilds the engine + forces a full
  redraw whenever that mode changes (only on a keypress, never in the steady-state path).
- Modal color menu: `C` opens/closes; while open `O`/Space on/off, `M` cycle
  `Ansi16→Ansi256→TrueColor` (clamped to `detect_color_mode()`), `B` quick-picks blocks, `+/-`
  contrast, `R` reset (keeps menu open), `Esc`/`C` close. Ctrl+C still quits. Panel drawn via
  `viewer_chrome::overlay_panel`. Status bar shows `color:<mode|off>`.
- CLI: kept `--color`; added `--color-mode <16|256|true>` (maps to `ColorMode`, seeds the menu's
  requested fidelity; unset starts at the detected capability). `--output`/`--cat` stay grayscale
  and deterministic (no env-dependent detection).
- Clamp is a local `None<Ansi16<Ansi256<TrueColor` rank; requesting more than detected degrades.
