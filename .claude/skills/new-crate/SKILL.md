---
name: new-crate
description: Scaffold a new rgfx workspace crate consistently — Cargo.toml wired to workspace deps, lib.rs with the standard lints, and a tests module. Use when a task requires creating one of the crates/rgfx-* crates.
---

# Scaffold an rgfx crate

All crates live under `crates/` and inherit shared dependency versions from the root
`[workspace.dependencies]`. Never pin a version inside a member crate that the workspace
already defines — use `<dep>.workspace = true`.

## 1. Cargo.toml

`crates/rgfx-<name>/Cargo.toml`:

```toml
[package]
name = "rgfx-<name>"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
description = "<one line: what this crate does>"

[dependencies]
rgfx-core = { path = "../rgfx-core" }   # every crate except core
thiserror.workspace = true
# add only what you need, all via .workspace = true when the workspace defines it
```

The binary crate `rgfx-cli` additionally sets `[[bin]] name = "rgfx"` and uses
`anyhow` + `clap` instead of `thiserror`.

## 2. lib.rs header (library crates)

```rust
//! rgfx-<name>: <one line>.
#![warn(missing_docs)]
#![forbid(unsafe_code)] // remove only with a documented reason

// re-export what downstream crates need; keep the public surface small and documented.
```

## 3. Register in the workspace

Add the path to the root `Cargo.toml` `[workspace] members = [...]`. If the crate is one
already listed in the planned members list, it's likely present — verify, don't duplicate.

## 4. Minimum tests

Add a `#[cfg(test)] mod tests` (or `tests/` dir) with at least one meaningful test before you
finish — the finish gate runs `cargo test -p rgfx-<name>`.

## 5. Verify

```
cargo check -p rgfx-<name>
cargo clippy -p rgfx-<name> --all-targets --all-features -- -D warnings
cargo test -p rgfx-<name>
```
