# 034 — Full-redraw staircase in raw mode (3D first frame & image preview)

- **Crate:** `rgfx-terminal` (root cause) · verify via `rgfx-cli` viewers
- **Depends on:** 007 (frame engine), 022 (3D viewer), 032 (image full-screen)
- **Branch:** `task/034-raw-mode-full-redraw-carriage-return`
- **Skill:** invoke **`rgfx-fix`** first (required; the hook blocks `crates/**` edits until then).

## Symptoms (two reports, one cause)
1. **3D viewer**: the first frame is "completely misaligned / doesn't look right", but once you
   orbit the object it snaps to the correct shape and stays correct.
2. **Image preview (full-screen default)**: "every second row of characters is empty" — the image
   is garbled. **`--cat` (inline) looks correct.**

## Root cause
The interactive viewers run in **raw mode** (alternate screen), where the terminal does **not**
translate `\n` into `\r\n`. The `FrameEngine`:
- **full-redraw** path (`emit_full_redraw`) writes `CLEAR_HOME` then the serializer's grid, whose
  rows are joined with **`\n`** — in raw mode the cursor line-feeds without returning to column 0,
  so each row starts further right: a staircase that reads as "every other row empty."
- **diff** path emits an **absolute cursor move** per changed run, so it is correct in raw mode.

That split explains everything: a static **image** only ever full-redraws → stays broken;
**`--cat`** isn't in raw mode → fine; the **3D** first frame is a full redraw (broken), and the
first orbit produces a **diff** (correct) → "fixed when I turn it."

## Fix
Make the full-redraw path position rows explicitly instead of relying on `\n`:
- In `FrameEngine::emit_full_redraw` (rgfx-terminal), write each row preceded by an absolute
  cursor move (`\x1b[{row+1};1H`), matching the diff path — **or** emit `\r\n` between rows.
- Do **not** change `TerminalFrame::to_text` / the file-output path (`--output`), which must keep
  plain `\n` for text files. The CR fix belongs to terminal output only (the `FrameEngine`).
- Secondary hardening (optional, same task): viewers read `session.viewport()` once before their
  loop; re-query at first render (or on the first tick) so a stale initial size can't render the
  first frame at the wrong dimensions.

## Acceptance criteria
- [ ] 3D first frame is correct immediately (no turn needed); image full-screen renders every row.
- [ ] `--cat` and `--output` are unchanged (still plain `\n`).
- [ ] All viewers (image/3D/gif/video) unaffected in the diff path.

## Tests required (the gap that let this ship)
- [ ] Unit test on `FrameEngine` output bytes: a full redraw of a ≥2-row frame contains a
      carriage return or a per-row absolute cursor move for every row (i.e., no bare `\n` that
      would staircase in raw mode). This is a byte-level assertion — a naive PTY capture that
      splits on `\n` masks the bug, so test the emitted bytes directly.
- [ ] Regression: two consecutive identical full redraws still position rows correctly.

## Completion
- [ ] Implemented · [ ] Gate green + byte-level test · [ ] PR opened · [ ] Merged

**Status:** ⬜ NOT STARTED
