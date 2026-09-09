#!/usr/bin/env bash
# SubagentStop / Stop hook. Runs the CI-parity gate and, if anything fails, blocks the agent
# from finishing by returning a JSON decision that feeds the failure back for a fix.
#
# It scopes work to the crate(s) the agent is touching to stay fast:
#   - if RGFX_TASK_CRATE is set (exported in the agent's prompt), gate only that crate
#   - otherwise gate the whole workspace
#
# Emits {"decision":"block","reason":"..."} on failure (agent keeps working) or nothing on pass.
set -uo pipefail

ROOT="$(git rev-parse --show-toplevel 2>/dev/null || echo .)"
cd "$ROOT" || exit 0

# No workspace manifest yet (very early repo) → nothing to gate.
[ -f Cargo.toml ] || exit 0

crate="${RGFX_TASK_CRATE:-}"
if [ -n "$crate" ]; then
  scope=(-p "$crate")
  label="crate '$crate'"
else
  scope=(--workspace)
  label="workspace"
fi

out=""
run() {
  local name="$1"; shift
  local log
  if ! log="$("$@" 2>&1)"; then
    out+=$'\n### '"$name"$' failed\n```\n'"$(printf '%s' "$log" | tail -n 40)"$'\n```\n'
    return 1
  fi
  return 0
}

fail=0
run "cargo fmt --check"  cargo fmt --all --check || fail=1
run "cargo check"        cargo check "${scope[@]}" --all-targets || fail=1
run "cargo clippy"       cargo clippy "${scope[@]}" --all-targets --all-features -- -D warnings || fail=1
run "cargo test"         cargo test "${scope[@]}" || fail=1

if [ "$fail" -eq 0 ]; then
  exit 0
fi

reason="CI-parity gate failed for ${label}. Fix these before finishing (do not skip tests or add #[allow] to silence clippy):${out}"
# jq -Rs turns the multiline reason into a valid JSON string.
printf '{"decision":"block","reason":%s}\n' "$(printf '%s' "$reason" | jq -Rs .)"
exit 0
