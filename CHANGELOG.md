# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Docs
- README: added a terminal demo GIF, a Highlights section, and an interactive-controls quick reference.

### Changed
- Still images now open a full-screen preview with a bottom options bar (renderer/dither/invert/color toggles), matching the 3D viewer; `-c`/`--cat` prints inline like `cat` and returns. (#032)

### Added
- 3D viewer: opens at a 3/4 angle (fixes the flat "poorly loaded" first frame), adds a roll axis (`z`/`x`), mouse drag-to-orbit + wheel-zoom, and `.blend` support via headless Blender export. (#031)

### Fixed
- Still images now render **inline** at the terminal size and return to the shell by default; the full-screen viewer moved behind `--interactive`. `--output`/inline width now follows the real terminal instead of a fixed 80 columns. (#030)

### Added

- Cargo workspace scaffolding and the `rgfx-core` crate (framebuffer, color,
  geometry, camera, and the encoder/renderer/frame-source trait contracts).
- Open-source project foundation: dual MIT/Apache-2.0 licensing, `README.md`,
  `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, `SECURITY.md`, and this changelog.
- Continuous integration (`ci.yml`) mirroring the local finish gate — `cargo fmt`,
  `clippy -D warnings`, `cargo test --workspace`, and `cargo build --workspace`
  with cargo caching.
- Release automation (`release.yml`) that cross-builds Linux `x86_64` and
  `aarch64` binaries, packages `rgfx-linux-<arch>.tar.gz`, and publishes
  `SHA256SUMS` on tagged releases.
- Issue and pull-request templates and Dependabot configuration for Cargo and
  GitHub Actions.
- One-line `install.sh` that detects OS/architecture, resolves the latest
  release, downloads the matching `rgfx-linux-<arch>.tar.gz`, verifies its
  SHA-256 against `SHA256SUMS`, installs to `~/.local/bin`, and prints PATH
  guidance.
- AUR packaging templates under `packaging/aur/` for `rgfx` (source build),
  `rgfx-bin` (prebuilt binary), and `rgfx-git` (VCS), plus a packaging README.
- crates.io publishing metadata (`keywords`, `categories`, `readme`, and a
  shared `homepage`) on every publishable crate; `rgfx-bench` is marked
  `publish = false`.
- Install documentation in `README.md` for the one-line installer and the AUR
  packages.

[Unreleased]: https://github.com/05vukasin/rgfx/commits/main
