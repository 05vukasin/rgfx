# rgfx — engineering conventions (read this first)

`rgfx` is a universal terminal graphics engine in Rust: images, video, animation, and
interactive 3D previews, all rendered into an internal framebuffer and then encoded into
terminal cells. This file is loaded into every agent's context. Follow it exactly.

## The one architectural law

> **Nothing renders directly to the terminal. Every media source produces a `Framebuffer`.
> Only the terminal layer turns a framebuffer into text.**

```
OBJ / STL / glTF ┐
Image ───────────┼──► Framebuffer ──► TerminalEncoder ──► Terminal
Video ───────────┤
Animation ───────┘
```

Concretely:
- Loaders/decoders do **not** know about Braille, ASCII, or blocks.
- The Braille encoder does **not** know about OBJ, images, or video.
- The 3D rasterizer does **not** call `println!` or touch stdout.
- All sources converge on `rgfx_core::Framebuffer`. All output goes through a `TerminalEncoder`.

If you find yourself importing a loader crate into an encoder crate (or vice-versa), stop —
you are crossing a boundary. Route through the framebuffer instead.

## Workspace & crate boundaries

```
rgfx/
├── Cargo.toml            # [workspace] — shared deps live here
└── crates/
    ├── rgfx-core/        # Framebuffer, color, traits, error types. Depends on NOTHING internal.
    ├── rgfx-terminal/    # Braille / ASCII / block encoders, ANSI, terminal I/O. Depends on core.
    ├── rgfx-image/       # image decode → framebuffer, dithering, resize. Depends on core.
    ├── rgfx-3d/          # mesh loaders, software rasterizer, camera. Depends on core.
    ├── rgfx-video/       # ffmpeg frame decode → framebuffer. Depends on core.
    └── rgfx-cli/         # the `rgfx` binary. Depends on all of the above.
```

Dependency direction is strict and one-way: **everything depends on `rgfx-core`; nothing
depends on `rgfx-cli`; sibling library crates do not depend on each other.** `rgfx-core`
defines the shared vocabulary (framebuffer, color, traits, errors) so parallel crates compose.

## Rust standards

- **Edition 2024**, MSRV = the repo's `rust-version` (1.85+). Use `rust-toolchain` if pinning.
- **Errors:** library crates use `thiserror` and return a crate-local `Error`/`Result`. The
  `rgfx-cli` crate uses `anyhow`. **Libraries never `.unwrap()`/`.expect()`/`panic!` on
  input-derived data** — return errors. `unwrap` is only acceptable on provably-infallible
  invariants, and then with a `// invariant:` comment.
- **No stdout in libraries.** Diagnostics go through `tracing`, never `println!`/`eprintln!`.
  Only `rgfx-cli` writes to the terminal.
- **Public API is documented.** Every public item gets a `///` doc comment. Crates set
  `#![warn(missing_docs)]`.
- **Deny warnings.** Code must pass `cargo clippy --all-targets --all-features -- -D warnings`.
- **Format with rustfmt defaults.** Saves are auto-formatted by a hook; do not hand-fight it.
- **Performance rules from the spec:** reuse buffers (no per-frame allocation), keep pixel
  memory contiguous, keep terminal-cell dimensions separate from framebuffer dimensions.
  Reach for `rayon` only after profiling.

## Testing (required, not optional)

Every crate ships tests. Aim for:
- Unit tests for pure logic (Braille bit masks, framebuffer indexing, resize/aspect math,
  luminance, clipping, depth test, barycentric coords).
- Snapshot/golden tests where output is deterministic (framebuffer → exact Braille string;
  a known cube → known silhouette).
- Parser robustness tests (malformed OBJ/STL/glTF must error, not panic).
- `cargo test -p <your-crate>` must pass before you finish. This is enforced by a hook.

## Definition of Done (every task)

1. Code compiles: `cargo check -p <crate>`.
2. Lint clean: `cargo clippy -p <crate> --all-targets --all-features -- -D warnings`.
3. Formatted: `cargo fmt --all` (the save hook mostly handles this).
4. Tests pass: `cargo test -p <crate>`.
5. Public items documented; `cargo doc -p <crate>` has no warnings.
6. Commit on your task branch, push, open a PR against `main`. **Never commit to `main`.**

Use the `finish-task` skill for the exact commit/push/PR procedure, the `new-crate` skill to
scaffold a crate, and the `rgfx-architecture` skill for the detailed type/trait contracts.

## Parallel-work discipline

- You own **one crate / one task**. Stay inside it. Do not refactor another crate's public
  API — if you need a change in `rgfx-core`, note it in your PR description instead of editing
  it, unless your task explicitly owns core.
- Never edit the root `Cargo.toml` `[workspace.dependencies]` to change a shared version
  without flagging it; version drift breaks other agents' branches.
- Work in your assigned git worktree/branch. Rebase on `main` is the integrator's job, not
  yours — keep your branch focused and small.
