#!/usr/bin/env bash
# PreToolUse hook for Write|Edit. Enforces "invoke the rgfx-fix skill first" before any edit to
# rgfx source under crates/. The skill's step 0 creates the marker below; once present, edits are
# allowed for the rest of the session/worktree. Edits to docs, backlog, .claude, and other files
# are never blocked.
#
# Mechanism: exit code 2 blocks the tool call and surfaces stderr to the model.
set -uo pipefail

input="$(cat)"
file="$(printf '%s' "$input" | jq -r '.tool_input.file_path // .tool_input.path // empty' 2>/dev/null || true)"
[ -z "$file" ] && exit 0

# Only gate Rust source under a crates/ directory.
case "$file" in
  *crates/*.rs) ;;
  *) exit 0 ;;
esac

root="${CLAUDE_PROJECT_DIR:-$(git rev-parse --show-toplevel 2>/dev/null || echo .)}"
marker="$root/.claude/.skill-active"
[ -f "$marker" ] && exit 0

cat >&2 <<'MSG'
rgfx policy: invoke the `rgfx-fix` skill (via the Skill tool) before editing source under crates/.
It sets up the required reproduce → backlog task → implement → verify (PTY + gate) → PR workflow,
and its first step arms the marker that unblocks these edits. Run the skill, then retry the edit.
MSG
exit 2
