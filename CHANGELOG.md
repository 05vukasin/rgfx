# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/05vukasin/rgfx/commits/main
