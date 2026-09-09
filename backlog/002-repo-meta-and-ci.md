# 002 — Repo meta & CI/CD

- **Crate:** none (root, `.github/`, docs)
- **Depends on:** 001 (CI runs against the real workspace)
- **Blocks:** 027 (packaging)
- **Branch:** `task/002-repo-meta-and-ci`
- **Skills:** `finish-task`

## Goal
Give the project a complete open-source foundation and a CI pipeline that mirrors the local
finish gate, so every downstream PR lands green against a known bar.

## Scope / deliverables
1. **Dual license:** `LICENSE-MIT` and `LICENSE-APACHE` (standard texts), copyright
   "The rgfx authors". Reference both in README.
2. **Docs:** promote `README_rgfx.md` content into `README.md` (keep the spec intact, add
   badges, build/run/install sections, license note). Add `CONTRIBUTING.md` (summarize the
   `CLAUDE.md` conventions + PR flow), `CODE_OF_CONDUCT.md` (Contributor Covenant),
   `SECURITY.md`, `CHANGELOG.md` (Keep a Changelog, `Unreleased`).
3. **CI** `.github/workflows/ci.yml`: on push/PR to `main`, matrix stable Rust, run
   `cargo fmt --all --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
   `cargo test --workspace`, `cargo build --workspace`. Cache cargo registry + target.
   Install `ffmpeg` in CI so `rgfx-video` tests can run (guard video tests behind a feature so
   the base build stays FFmpeg-optional).
4. **Release** `.github/workflows/release.yml`: on tag `v*`, cross-build
   `x86_64-unknown-linux-gnu` + `aarch64-unknown-linux-gnu`, package
   `rgfx-linux-<arch>.tar.gz`, generate `SHA256SUMS`, upload to the GitHub Release.
5. **Issue/PR templates:** `.github/ISSUE_TEMPLATE/` (bug, feature) + `PULL_REQUEST_TEMPLATE.md`
   mirroring the `finish-task` PR checklist. `.github/dependabot.yml` for cargo.

## Acceptance criteria
- [ ] `cargo fmt --all --check` and the full clippy/test/build commands pass locally (CI mirrors them).
- [ ] `ci.yml` and `release.yml` are valid workflow YAML (`gh workflow view` or actionlint if available).
- [ ] Both LICENSE files present; README states dual licensing.
- [ ] Base `cargo build --workspace` does **not** require FFmpeg installed.

## Tests required
- [ ] N/A for code, but add a `tests/meta.rs` (or doc test) asserting the workspace has the
      expected member crates listed, so structural drift is caught. (Optional but preferred.)

## Out of scope
Actual release binaries / crates.io publish / AUR — that's task 027.

## Completion
- [ ] Implemented
- [ ] CI workflows validated
- [ ] PR opened: <!-- url -->
- [ ] Merged to `main`

**Status:** ✅ DONE (merged)
