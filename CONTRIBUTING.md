# Contributing to rgfx

Thanks for your interest in improving `rgfx`. This document summarizes the
engineering conventions (see `CLAUDE.md` for the authoritative version) and the
pull-request flow.

## The one architectural law

> **Nothing renders directly to the terminal. Every media source produces a
> `Framebuffer`. Only the terminal layer turns a framebuffer into text.**

- Loaders/decoders do **not** know about Braille, ASCII, or blocks.
- The terminal encoders do **not** know about OBJ, images, or video.
- The 3D rasterizer never calls `println!` or touches stdout.
- All sources converge on `rgfx_core::Framebuffer`; all output goes through a
  `TerminalEncoder`.

If you find yourself importing a loader crate into an encoder crate (or
vice-versa), stop — route through the framebuffer instead.

## Workspace layout

```
rgfx/
├── Cargo.toml            # [workspace] — shared deps live here
└── crates/
    ├── rgfx-core/        # Framebuffer, color, traits, errors. Depends on nothing internal.
    ├── rgfx-terminal/    # Braille / ASCII / block encoders, ANSI, terminal I/O.
    ├── rgfx-image/       # image decode → framebuffer, dithering, resize.
    ├── rgfx-3d/          # mesh loaders, software rasterizer, camera.
    ├── rgfx-video/       # ffmpeg frame decode → framebuffer (feature-gated).
    └── rgfx-cli/         # the `rgfx` binary.
```

Dependency direction is strict and one-way: **everything depends on `rgfx-core`;
nothing depends on `rgfx-cli`; sibling library crates do not depend on each
other.**

## Rust standards

- **Edition 2024**, MSRV = the repo's `rust-version` (1.85+).
- **Errors:** library crates use `thiserror` and return a crate-local
  `Error`/`Result`; `rgfx-cli` uses `anyhow`. Libraries never
  `.unwrap()`/`.expect()`/`panic!` on input-derived data — return errors.
  `unwrap` is only acceptable on provably-infallible invariants, documented with
  a `// invariant:` comment.
- **No stdout in libraries.** Diagnostics go through `tracing`, never
  `println!`/`eprintln!`. Only `rgfx-cli` writes to the terminal.
- **Public API is documented.** Every public item gets a `///` doc comment; crates
  set `#![warn(missing_docs)]`.
- **Deny warnings.** Code must pass
  `cargo clippy --all-targets --all-features -- -D warnings`.
- **Format with rustfmt defaults.** Do not hand-fight the formatter.
- **Performance rules:** reuse buffers (no per-frame allocation), keep pixel
  memory contiguous, keep terminal-cell dimensions separate from framebuffer
  dimensions. Reach for `rayon` only after profiling.

## Testing

Every crate ships tests:

- Unit tests for pure logic (Braille bit masks, framebuffer indexing, resize/
  aspect math, luminance, clipping, depth test, barycentric coords).
- Snapshot/golden tests where output is deterministic.
- Parser-robustness tests (malformed OBJ/STL/glTF must error, not panic).

## Definition of Done

1. Code compiles: `cargo check --workspace`.
2. Lint clean: `cargo clippy --all-targets --all-features -- -D warnings`.
3. Formatted: `cargo fmt --all`.
4. Tests pass: `cargo test --workspace`.
5. Public items documented; `cargo doc` has no warnings.

CI runs exactly this gate on every push and pull request to `main`.

## Pull-request flow

1. Never commit to `main`. Branch from `main` using `task/<short-slug>` (or a
   descriptive `feat/…`, `fix/…` name).
2. Make focused, single-purpose changes. Stay inside the crate you are working on;
   do not refactor another crate's public API.
3. Run the full gate locally before pushing.
4. Open a PR against `main` and fill in the pull-request template. Do not merge
   your own PR — the maintainers/integrator handle merges.
5. Use [Conventional Commits](https://www.conventionalcommits.org/) for commit and
   PR titles: `feat(rgfx-core): …`, `fix(rgfx-terminal): …`, `docs: …`,
   `test(rgfx-image): …`, `refactor: …`, `chore: …`.

## Code of Conduct

This project follows the [Contributor Covenant](CODE_OF_CONDUCT.md). By
participating you agree to uphold it.

## License

By contributing, you agree that your contributions will be dual-licensed under
the MIT and Apache-2.0 licenses, matching the project's licensing.
