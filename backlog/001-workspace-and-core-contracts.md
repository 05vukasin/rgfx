# 001 — Workspace & core contracts

- **Crate:** `rgfx-core`
- **Depends on:** — (this is the foundation; **must merge before any other task**)
- **Blocks:** everything
- **Branch:** `task/001-workspace-and-core-contracts`
- **Skills:** `rgfx-architecture`, `new-crate`, `finish-task`

## Goal
Turn the single-crate scaffold into a Cargo **workspace** and implement `rgfx-core`, the crate
that defines the shared vocabulary every other crate builds against: `Framebuffer`, `Color`,
`Error`/`Result`, the `FrameSource` / `TerminalEncoder` / `SceneRenderer` traits, `Viewport`,
`TerminalFrame`, and placeholder `Scene` / `Camera` types. These contracts are **frozen** once
merged — other agents depend on them, so get them right.

## Scope / deliverables
1. Root `Cargo.toml` as `[workspace]` with `resolver = "2"`, `members = ["crates/*"]`, and a
   `[workspace.package]` (version `0.1.0`, edition `2024`, `rust-version = "1.85"`, license
   `MIT OR Apache-2.0`, repository URL) plus `[workspace.dependencies]` pinning the shared
   crates from the README stack (`thiserror`, `anyhow`, `glam`, `image`, `crossterm`, `clap`,
   `tobj`, `stl_io`, `gltf`, `rayon`, `serde`, `toml`, `tracing`). Verify current compatible
   versions with `cargo add --dry-run` — do **not** trust the README's example versions.
2. Delete the old top-level `src/main.rs` scaffold; the binary now lives in `rgfx-cli` (created
   later — you only create `rgfx-core` here, but leave the workspace buildable).
3. `crates/rgfx-core/` implementing the contracts in the `rgfx-architecture` skill:
   `Color`, `Framebuffer` (color+depth, contiguous, `new`/`resize`(reuse alloc)/`clear`/`get`/
   `set`/`luma`/accessors), `Error`+`Result` (thiserror), `Viewport`, `TerminalFrame` (encoded
   cell grid: chars + optional per-cell fg/bg color + dims), the three traits, and minimal
   `Scene`/`Camera`/mesh types (`Mesh` with vertices/indices/normals, bounding box/sphere).

## Contracts / API
Follow the `rgfx-architecture` skill **exactly** — signatures there are the spec. If you must
deviate or add, document why in the PR; downstream crates assume those signatures.

## Acceptance criteria
- [ ] `cargo build --workspace` and `cargo test --workspace` succeed.
- [ ] `#![warn(missing_docs)]` + `#![forbid(unsafe_code)]`; every public item documented.
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings` clean.
- [ ] `Framebuffer::resize` reuses the backing `Vec` when shrinking/same and only grows when needed (assert no realloc on same-size in a test via capacity check).
- [ ] No `println!`/`eprintln!` anywhere; no panics on constructor edge cases (0×0 allowed).

## Tests required
- [ ] Framebuffer indexing round-trip (`set` then `get`), row-major order.
- [ ] `resize` preserves capacity semantics; `clear` resets color and depth (depth→1.0).
- [ ] `luma` matches expected perceptual weights for pure R/G/B/white/black.
- [ ] Bounding box/sphere computed correctly for a known mesh.

## Out of scope
Any encoder, loader, or rendering logic. This task only establishes contracts + primitives.

## Completion
- [ ] Implemented
- [ ] Finish gate green (fmt / clippy `-D warnings` / check / test)
- [ ] `cargo doc` no warnings
- [ ] PR opened: <!-- url -->
- [ ] Merged to `main`

**Status:** ✅ DONE (merged)
