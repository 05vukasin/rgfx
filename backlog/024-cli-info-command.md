# 024 — `rgfx info` command

- **Crate:** `rgfx-cli`
- **Depends on:** 020, 015, 016, 017 (mesh stats); 008/010/018 (media stats)
- **Blocks:** —
- **Branch:** `task/024-cli-info-command`
- **Skills:** `finish-task`

## Goal
Implement `rgfx info <FILE>`: inspect a file and print a concise, non-interactive metadata
report (no alt screen, no rendering) — the spec's `info` example.

## Scope / deliverables
1. `info` subcommand routing by media kind (reuse 020's detection).
2. For meshes: type, mesh count, vertices, triangles, materials, animations, bounding box size
   (from 015–017 loaders' stats).
3. For images/gif: format, dimensions, frame count, color type. For video: container, codec,
   resolution, duration, fps (from 018's `probe`).
4. Human-readable output (aligned key/value); optional `--json` for machine consumption.

## Contracts / API
Read-only; prints to stdout (this is the CLI binary, so stdout is allowed here). No terminal
raw mode. `anyhow::Result`.

## Acceptance criteria
- [ ] `rgfx info cube.obj` prints correct counts matching the loader.
- [ ] `rgfx info some.png` and (if ffmpeg) `rgfx info clip.mp4` print correct metadata.
- [ ] `--json` emits valid JSON. Unknown/corrupt file → clean error, no panic. Clippy `-D warnings`.

## Tests required
- [ ] info-for-mesh output for a bundled OBJ/STL/GLB (assert key fields).
- [ ] `--json` schema/round-trip test.
- [ ] Image info for an embedded PNG.

## Out of scope
Interactive viewing (021–023).

## Completion
- [ ] Implemented
- [ ] Finish gate green
- [ ] PR opened: <!-- url -->
- [ ] Merged to `main`

**Status:** ⬜ NOT STARTED
