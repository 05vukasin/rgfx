# 003 — Terminal backend

- **Crate:** `rgfx-terminal` (creates the crate)
- **Depends on:** 001
- **Blocks:** 004, 005, 006, 007, 022
- **Branch:** `task/003-terminal-backend`
- **Skills:** `new-crate`, `rgfx-architecture`, `finish-task`

## Goal
Create `rgfx-terminal` and implement the safe terminal lifecycle + input layer on top of
`crossterm`: raw mode, alternate screen, cursor hide/show, keyboard/mouse/resize events, and
**guaranteed cleanup** on every exit path. This is the crate every encoder (004–007) extends.

## Scope / deliverables
1. Scaffold `crates/rgfx-terminal` per the `new-crate` skill (`crossterm.workspace = true`).
2. A `Terminal` type that, on construction, enables raw mode + enters alternate screen + hides
   cursor (+ optional mouse capture), and **restores all of it in `Drop`** and on
   normal/error/`Ctrl+C`/panic paths (install a panic hook or use a guard). Restoration order
   must reverse setup (README "Terminal Safety").
3. `size() -> Viewport` reading current columns/rows.
4. An input abstraction: `poll_event(timeout) -> Option<Event>` mapping crossterm events into
   an `rgfx`-local `enum Event { Key(...), Resize(cols,rows), Mouse(...) , ... }` so the CLI
   doesn't depend on crossterm's types directly.
5. A raw write path: `present_raw(&str)` / batched writer using a buffered stdout + a single
   flush per frame (no per-cell `print!`). Actual diffing lives in 007; expose the buffered
   writer it will build on.

## Contracts / API
Return `rgfx_core::Viewport` from `size()`. Keep terminal-cell dims separate from framebuffer
dims. Library must not `println!`; all output goes through the buffered writer.

## Acceptance criteria
- [ ] Constructing then dropping `Terminal` leaves the terminal in its original mode (manually verifiable; unit-test the guard logic where possible without a real TTY).
- [ ] No leaked raw mode / alt screen on simulated error return.
- [ ] Clippy `-D warnings` clean; public items documented.
- [ ] Works headlessly enough that tests don't require an interactive TTY (guard TTY-only paths).

## Tests required
- [ ] Event mapping: crossterm key/resize/mouse → local `Event` variants.
- [ ] Cleanup guard invoked exactly once (use a mock/flag) on drop and on panic path.
- [ ] Buffered writer batches writes and flushes once (assert via an in-memory writer).

## Out of scope
Braille/ASCII/block encoding (004/005), color (006), diff engine internals (007).

## Completion
- [ ] Implemented
- [ ] Finish gate green
- [ ] PR opened: <!-- url -->
- [ ] Merged to `main`

**Status:** ⬜ NOT STARTED
