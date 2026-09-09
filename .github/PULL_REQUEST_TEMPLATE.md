<!--
Thanks for contributing to rgfx! Please fill in the sections below.
PR titles should follow Conventional Commits, e.g. `feat(rgfx-core): ...`.
-->

## Task

<!-- Link the backlog task or issue this implements, e.g. `backlog/NN-slug.md` or #123. -->

## What changed

- <!-- bullet summary of the functionality added / changed -->

## Contract compliance

- [ ] Only touched the crate(s) I own — no unrelated public-API refactors
- [ ] Types match the rgfx-core contract (no forked `Framebuffer` / traits)
- [ ] No stdout in library code; errors returned via `thiserror`/`Result`
- [ ] Public items documented (`#![warn(missing_docs)]` clean)

## Testing

- [ ] `cargo fmt --all --check` passes
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo build --workspace` passes (no FFmpeg required for the base build)
- Describe the tests you added (unit / snapshot / parser-robustness):

## Notes for the integrator

<!--
- Any rgfx-core changes you needed but did NOT make (flag, don't edit).
- Any follow-up tasks this unblocks.
- CHANGELOG.md updated under [Unreleased]? (for user-facing changes)
-->
