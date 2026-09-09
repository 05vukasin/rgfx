# 027 — Packaging & release

- **Crate:** root (`install.sh`, packaging, crates.io metadata)
- **Depends on:** 002 (CI/release workflow), 022 (a working `rgfx` binary to ship)
- **Blocks:** —
- **Branch:** `task/027-packaging-and-release`
- **Skills:** `finish-task`

## Goal
Make `rgfx` installable the way the spec describes: a one-line installer, GitHub Release
binaries with checksums, crates.io-ready metadata, and AUR packaging stubs.

## Scope / deliverables
1. `install.sh` finalized from the spec's example: detect OS/arch, resolve latest release,
   download `rgfx-linux-<arch>.tar.gz`, **verify SHA256** against `SHA256SUMS`, install to
   `~/.local/bin/rgfx`, PATH guidance. Shellcheck-clean.
2. Ensure `release.yml` (from 002) actually produces the archives + `SHA256SUMS` the installer
   expects (align names). Add checksum generation if missing.
3. crates.io readiness: every publishable crate has `description`, `license`, `repository`,
   `keywords`, `categories`, `readme`; verify with `cargo publish --dry-run` per crate in
   dependency order.
4. AUR: provide `packaging/aur/` `PKGBUILD` templates for `rgfx` (source build), `rgfx-bin`
   (release binary), `rgfx-git` (VCS), plus a short packaging README. (Publishing to AUR is
   manual/out of scope — just the templates.)
5. `CHANGELOG.md` updated for the release; document the install methods in `README.md`.

## Contracts / API
No library code. Don't break the base build.

## Acceptance criteria
- [ ] `bash -n install.sh` + shellcheck clean; a dry-run against a real/tagged release downloads + verifies checksum.
- [ ] `cargo publish --dry-run` succeeds for each publishable crate.
- [ ] PKGBUILD templates are syntactically valid (`makepkg --printsrcinfo` parses, if available).
- [ ] Release workflow archive names match installer expectations.

## Tests required
- [ ] A CI/local check that `install.sh` passes `bash -n` and shellcheck.
- [ ] (If practical) a smoke test that `cargo publish --dry-run` runs in CI for the workspace.

## Out of scope
Actually publishing to crates.io / AUR (manual, owner-gated). macOS/Windows targets (later).

## Completion
- [ ] Implemented
- [ ] Finish gate green
- [ ] PR opened: <!-- url -->
- [ ] Merged to `main`

**Status:** ⬜ NOT STARTED
