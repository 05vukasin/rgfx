# rgfx backlog

One file per task, named `NNN-slug.md`. Each task declares the crate it owns, its
dependencies, acceptance criteria, required tests, and a **Completion** section used to track
status. Agents pick up a task, work in an isolated worktree/branch (`task/NNN-slug`), and open
a PR against `main`. The integrator (you) merges.

## Status legend
`⬜ NOT STARTED` · `🟡 IN PROGRESS` · `🔵 IN REVIEW (PR open)` · `✅ DONE (merged)` · `⛔ BLOCKED`

## How to run the swarm
1. **Foundation first, solo:** `001` must be merged before anything else — it defines the
   shared `rgfx-core` vocabulary every other crate compiles against. `002` (repo meta + CI)
   can land alongside it.
2. **Then fan out, one agent per crate.** Tasks that share a crate are ordered by their
   `Depends on` edges and run sequentially within that crate; different crates run in parallel.
3. Each agent runs the finish gate (fmt/clippy/check/test) — enforced by the SubagentStop hook.

## Waves (respecting dependency edges)
- **Wave 0 (solo):** 001, then 002
- **Wave 1 (parallel):** 003 (terminal), 008 (image), 011 (3d math), 020 (cli skeleton)
- **Wave 2 (parallel):** 004, 005, 006, 007 (terminal) · 009, 010 (image) · 012, 013 (3d)
- **Wave 3 (parallel):** 014, 015, 016, 017 (3d) · 018 (video) · 021 (image viewer)
- **Wave 4 (parallel):** 019 (video controls) · 022 (3d viewer) · 023 (gif/video wiring) · 024 (info) · 025 (stdin)
- **Wave 5 (parallel, cross-cutting):** 026 (benches) · 027 (packaging) · 028 (ratatui) · 029 (avatar api)

## Dependency graph
```
001 core ──┬─► 003 terminal-backend ─┬─► 004 braille
           │                         ├─► 005 ascii/blocks
           │                         ├─► 006 ansi-color
           │                         └─► 007 frame-engine
           ├─► 008 image-pipeline ───┬─► 009 dithering
           │                         └─► 010 formats+gif
           ├─► 011 3d-math-camera ───┬─► 012 wireframe
           │                         └─► 013 rasterizer ─► 014 lighting
           │                             015 obj / 016 stl / 017 gltf (need 013)
           ├─► 018 video (needs 008) ─► 019 video-controls
           └─► 020 cli-skeleton ─────┬─► 021 image-viewer  (needs 008,009,004,005)
                                     ├─► 022 3d-viewer     (needs 011-016,003,004,007)
                                     ├─► 023 gif/video      (needs 010 or 018,019)
                                     ├─► 024 info-command
                                     └─► 025 stdin-stream
026 benches · 027 packaging · 028 rgfx-ratatui · 029 avatar-api  (after their subjects exist)
```

## Status table
| # | Task | Crate | Depends on | Status |
|---|------|-------|-----------|--------|
| 001 | workspace-and-core-contracts | rgfx-core | — | ✅ |
| 002 | repo-meta-and-ci | (root/.github) | 001 | ✅ |
| 003 | terminal-backend | rgfx-terminal | 001 | ✅ |
| 004 | braille-encoder | rgfx-terminal | 003 | ✅ |
| 005 | ascii-and-block-encoders | rgfx-terminal | 003 | ✅ |
| 006 | ansi-color | rgfx-terminal | 003 | ✅ |
| 007 | frame-engine-diffing | rgfx-terminal | 003 | ✅ |
| 008 | image-pipeline | rgfx-image | 001 | ✅ |
| 009 | dithering-and-tone | rgfx-image | 008 | ✅ |
| 010 | image-formats-and-gif | rgfx-image | 008 | ✅ |
| 011 | 3d-math-and-camera | rgfx-3d | 001 | ✅ |
| 012 | wireframe-renderer | rgfx-3d | 011 | ✅ |
| 013 | triangle-rasterizer-zbuffer | rgfx-3d | 011 | ✅ |
| 014 | lighting-and-shading | rgfx-3d | 013 | ✅ |
| 015 | obj-loader | rgfx-3d | 013 | ✅ |
| 016 | stl-loader | rgfx-3d | 013 | ✅ |
| 017 | gltf-glb-loader | rgfx-3d | 013 | ✅ |
| 018 | video-ffmpeg-source | rgfx-video | 001, 008 | ✅ |
| 019 | video-playback-controls | rgfx-video | 018 | ✅ |
| 020 | cli-skeleton-and-config | rgfx-cli | 001 | ✅ |
| 021 | cli-image-viewer | rgfx-cli | 020, 008, 009, 004, 005 | ✅ |
| 022 | cli-3d-viewer | rgfx-cli | 020, 011-016, 003, 004, 007 | ✅ |
| 023 | cli-gif-video-playback | rgfx-cli | 020, 010, 018, 019 | ✅ |
| 024 | cli-info-command | rgfx-cli | 020, 015, 016, 017 | ✅ |
| 025 | cli-stdin-streaming | rgfx-cli | 020, 008 | ✅ |
| 026 | benchmarks | benches | 013, 004 | ✅ |
| 027 | packaging-and-release | (root) | 002, 022 | ✅ |
| 028 | rgfx-ratatui-integration | rgfx-ratatui | 007, 004 | ✅ |
| 029 | avatar-animation-api | rgfx-core/rgfx-terminal | 004, 007 | ✅ |
| 030 | responsive-inline-rendering | rgfx-terminal/rgfx-cli | 021 | ✅ |
| 031 | 3d-viewer-improvements | rgfx-3d/rgfx-cli | 022 | ✅ |
| 032 | fullscreen-image-preview | rgfx-cli | 021, 030, 007 | ✅ |
| 033 | viewer-chrome-and-options-bar | rgfx-cli | 032, 023, 022 | ✅ |
| 034 | raw-mode-full-redraw-carriage-return | rgfx-terminal | 007, 022, 032 | ✅ |
| 035 | light-controls-and-menu | rgfx-cli / rgfx-3d | 014, 022, 033 | ✅ |
| 036 | large-mesh-performance | rgfx-3d / rgfx-cli | 013, 022, 015, 017 | ⬜ |
| 037 | free-360-rotation | rgfx-3d / rgfx-cli | 011, 031, 022 | ⬜ |
| 038 | gltf-animation-playback | rgfx-3d / rgfx-cli | 017, 022, 035 | ⬜ |

Keep this table in sync as tasks progress (or regenerate from the per-file Completion sections).
