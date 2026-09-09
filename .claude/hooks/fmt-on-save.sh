#!/usr/bin/env bash
# PostToolUse hook for Write|Edit. Auto-formats any saved .rs file with rustfmt so that
# parallel agents never produce formatting-only diffs and CI's `cargo fmt --check` stays green.
# Reads the tool-call JSON on stdin; exits 0 always (formatting is best-effort, never blocking).
set -euo pipefail

input="$(cat)"
file="$(printf '%s' "$input" | jq -r '.tool_input.file_path // .tool_input.path // empty' 2>/dev/null || true)"

[ -z "$file" ] && exit 0
case "$file" in
  *.rs) ;;
  *) exit 0 ;;
esac
[ -f "$file" ] || exit 0

# Format in place using edition 2024; ignore failures (partial/uncompilable files are fine).
rustfmt --edition 2024 "$file" >/dev/null 2>&1 || true
exit 0
