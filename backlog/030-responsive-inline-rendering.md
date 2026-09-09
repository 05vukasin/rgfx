# 030 — Responsive inline rendering (bugfix)

- **Crate:** `rgfx-terminal` + `rgfx-cli`
- **Depends on:** 021 (image viewer), 003/004/005 (terminal)
- **Branch:** `task/030-responsive-inline-rendering`

## Problem (reported)
`rgfx image.png` (1) takes over the whole screen and requires `q` to exit, and (2) renders
misaligned by ~4 rows. Root causes:
1. The still-image default is a full-screen **alternate-screen** interactive viewer, not an
   inline print-and-return.
2. Terminal size has a single brittle probe and the only stdout path (`--output`) hard-codes
   **80 columns** regardless of terminal size (confirmed: 80-col output in a 100-col terminal).
3. Inline height must reserve a row or the returning shell prompt scrolls the top off — the
   perceived misalignment. Never caught because there was no real-PTY test.

## Fix
- **rgfx-terminal**: `terminal_size() -> Viewport`, usable without entering graphics mode.
  Fallback chain: real TTY size → `$COLUMNS`/`$LINES` → `80×24`. Exported from the crate.
- **rgfx-cli image viewer**: default becomes an inline `run_inline` — detect size, fit within
  `(cols, rows-1)` (headroom), print to stdout, return. No raw mode / alt-screen / `q`.
- Move the live full-screen viewer behind `--interactive`. `--output` (and inline) use the
  detected terminal width when `--width` is absent (remove the hard-coded 80).

## Acceptance
- [ ] `rgfx image.png` prints inline sized to the terminal and returns to the prompt.
- [ ] Output width tracks the real terminal width (verified under a PTY at 40/100/200 cols).
- [ ] No vertical misalignment / scroll (printed lines ≤ terminal rows).
- [ ] `--interactive` still opens the live viewer; `--width` still overrides.
- [ ] Real-PTY regression test asserts line count == rows and per-line width == cols.

## Completion
- [ ] Implemented
- [ ] Finish gate green + real-PTY manual check
- [ ] PR opened / merged

**Status:** ✅ DONE (merged)
