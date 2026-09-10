---
name: rgfx-fix
description: The MANDATORY workflow for fixing a bug or adding a feature in rgfx — reproduce (under a real PTY for viewer/terminal issues), write a backlog task, implement on a branch, verify with the gate + a PTY check, and open a PR. Invoke this FIRST, before editing any file under crates/. A PreToolUse hook blocks edits to crates/**/*.rs until this skill has run.
---

# rgfx-fix — fix/feature workflow (required before editing source)

Invoke this skill at the very start of any rgfx bug fix or feature. **Step 0 unblocks the
editing hook**, so do it before any Write/Edit to `crates/`.

## 0. Arm the skill marker (unblocks the edit hook)
Run exactly this first (creates the per-worktree marker the `require-skill` hook checks):

```bash
mkdir -p "$CLAUDE_PROJECT_DIR/.claude" && touch "$CLAUDE_PROJECT_DIR/.claude/.skill-active"
```

If `$CLAUDE_PROJECT_DIR` is unset, use the repo root: `touch .claude/.skill-active`.

## 1. Reproduce — and for viewer/terminal bugs, reproduce under a real PTY
Unit tests run headless and **miss terminal-timing and first-frame bugs**. For anything about
the interactive viewers (image/3D/gif/video), drive the built binary under a pseudo-terminal:

```python
# /tmp/pty.py — run a viewer at a fixed size, capture output, send keys, check the frame
import os, pty, struct, fcntl, termios, select, time
pid, fd = pty.fork()
if pid == 0: os.execvp("./target/release/rgfx", ["rgfx", "<file>"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
# read frame 1, optionally os.write(fd, b"\x1b[C") to orbit, read again, os.write(fd, b"q")
```

Assert on geometry: printed line count vs rows, per-line width vs cols, and whether frame 1
equals the steady-state frame. A known trap: a viewer that reads `session.viewport()` **once**
before its loop renders frame 1 at a stale size (before the alternate screen settles) and only
corrects on a later resize/keypress — re-query the size at render time or force a correct first
paint.

## 2. Write a backlog task
Add `backlog/NNN-slug.md` (next free number) describing the symptom, the reproduction, the root
cause, the fix, acceptance criteria, and required tests. Mark its status.

## 3. Implement on a branch
`task/NNN-slug`. Own the minimal surface. Import shared types from `rgfx_core`; follow
`CLAUDE.md`. Add a regression test that would have caught the bug (a PTY-driven test for
terminal/first-frame issues, or a pure-logic test for the extracted decision).

## 4. Verify — gate + behavior
- `cargo fmt --all` · `cargo clippy -p <crate> --all-targets --all-features -- -D warnings`
  · `cargo test -p <crate>` (the SubagentStop gate also enforces this).
- Re-run the PTY reproduction and confirm the symptom is gone (frame 1 correct, etc.).

## 5. Open a PR (never merge)
Commit (conventional message + the Co-Authored-By / Claude-Session trailers), push
`task/NNN-slug`, `gh pr create --base main` with the `finish-task` template. Report the branch,
PR URL, gate result, what you reproduced, and how you verified the fix.
