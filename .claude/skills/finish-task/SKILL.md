---
name: finish-task
description: The exact procedure to finish an rgfx backlog task — run the CI-parity gate, commit on a task branch, push, and open a PR against main with the standard template. Use when your implementation is complete and you are ready to hand the work off.
---

# Finish an rgfx task

Never commit to `main`. Every task lands via a PR from its own branch. If you were spawned in
a git worktree, you already have an isolated branch — use it.

## 1. Run the gate locally (the SubagentStop hook also enforces this)

```
cargo fmt --all
cargo check -p <crate> --all-targets
cargo clippy -p <crate> --all-targets --all-features -- -D warnings
cargo test -p <crate>
cargo doc -p <crate> --no-deps
```
All must pass with zero warnings. Do NOT silence clippy with blanket `#[allow]` or delete
tests to get green — fix the underlying issue.

## 2. Branch + commit

Branch name: `task/<NN>-<short-slug>` (matches the backlog task id).

```
git checkout -b task/<NN>-<slug>    # skip if already on your worktree branch
git add -A
git commit -m "<type>(<crate>): <summary>

<what and why, referencing backlog/<NN>-*.md>

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01RRrZ3bCQPBvVUo2fNCj7YJ"
```
`<type>` is one of feat/fix/test/docs/refactor/chore.

## 3. Push + open the PR

```
git push -u origin task/<NN>-<slug>
gh pr create --base main --title "<type>(<crate>): <summary>" --body "<see template>"
```

PR body template:

```markdown
## Task
Implements `backlog/<NN>-<slug>.md`.

## What changed
- bullet summary of the crate/functionality added

## Contract compliance
- [ ] Only touched my assigned crate (`rgfx-<name>`)
- [ ] Types match the rgfx-architecture contract (no forked Framebuffer/traits)
- [ ] No stdout in library code; errors via thiserror/Result
- [ ] Public items documented

## Testing
- [ ] `cargo test -p rgfx-<name>` passes
- [ ] clippy clean with `-D warnings`
- describe the tests you added (unit / snapshot / parser-robustness)

## Notes for the integrator
- any rgfx-core changes you needed but did NOT make (flag, don't edit)
- any follow-up tasks this unblocks
```

## 4. Report back

Your final message to the orchestrator must state: the branch name, the PR URL, whether the
gate passed, test count, and anything the integrator must know before merging.
