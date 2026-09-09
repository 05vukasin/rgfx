# 020 — CLI skeleton, dispatch & config

- **Crate:** `rgfx-cli` (creates the crate + the `rgfx` binary)
- **Depends on:** 001
- **Blocks:** 021, 022, 023, 024, 025
- **Branch:** `task/020-cli-skeleton-and-config`
- **Skills:** `new-crate`, `finish-task`

## Goal
Create `rgfx-cli` with the `rgfx` binary: argument parsing (`clap`), input-type auto-detection,
config loading, terminal setup/teardown, and a dispatch layer that routes a file to the right
viewer. Viewers themselves are stubbed here and filled in by 021–025.

## Scope / deliverables
1. Scaffold `crates/rgfx-cli` with `[[bin]] name = "rgfx"`, deps on `rgfx-core` +
   `rgfx-terminal` (others added by later tasks), `clap` (derive), `anyhow`, `toml`, `serde`,
   `tracing`+`tracing-subscriber`.
2. CLI per the spec: positional `<FILE>` (or `-` for stdin), `--renderer <braille|ascii|blocks|
   auto>`, `--width <N>`, `--fps <N>`, `--wireframe`, `--shading <mode>`, `--color`,
   `--output <file>`, and an `info <FILE>` subcommand. `--help` complete.
3. **Input-type detection** from extension + magic bytes → `enum MediaKind { Image, Gif, Video,
   Mesh(ObjStlGltf), Unknown }`. Route to the matching viewer trait (stubs return "not yet
   implemented" cleanly until 021–025 land).
4. **Config**: load `~/.config/rgfx/config.toml` (serde/toml), defaults per spec, CLI flags
   override config. `Config` struct with `[three_d]`/`[video]` sub-tables.
5. Terminal setup/teardown via `rgfx-terminal` `Terminal`, with guaranteed cleanup on all exit
   paths incl. Ctrl+C and panic.

## Contracts / API
`anyhow::Result` in the binary. Central `App`/`run()` that owns the terminal and dispatches.
Keep viewer logic behind traits so 021–025 plug in without touching dispatch.

## Acceptance criteria
- [ ] `rgfx --help` lists all flags/subcommands; parse tests pass.
- [ ] Media detection classifies `.png/.jpg/.gif/.mp4/.obj/.stl/.glb` correctly (ext + magic bytes).
- [ ] Config file is loaded and merged; CLI flags override; missing config uses defaults.
- [ ] Clean teardown on simulated error/interrupt. Clippy `-D warnings`.

## Tests required
- [ ] clap parse tests for representative arg sets (+ conflicts/defaults).
- [ ] Media-kind detection table (extension + magic-byte fixtures).
- [ ] Config merge precedence (defaults < file < flags).

## Out of scope
Actual rendering of each media type (021–025). Packaging (027).

## Completion
- [ ] Implemented
- [ ] Finish gate green
- [ ] `rgfx <unsupported>` and `rgfx --help` behave gracefully
- [ ] PR opened: <!-- url -->
- [ ] Merged to `main`

**Status:** ✅ DONE (merged)
