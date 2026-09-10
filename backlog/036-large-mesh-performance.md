# 036 — Large-mesh performance (simplify + adaptive + parallel)

- **Crate:** `rgfx-3d` (simplification + parallel raster) · `rgfx-cli` (adaptive render + status)
- **Depends on:** 013 (rasterizer), 022 (viewer), 015/017 (loaders)
- **Branch:** `task/036-large-mesh-performance`
- **Skill:** invoke **`rgfx-fix`** first (required; hook-enforced).

## Motivation (user report)
A 705k-face OBJ (45 MB) is very laggy — load + one frame ≈ 1.4 s single-threaded, and every
orbit re-rasterizes all 705k triangles. The user suggested simplifying the mesh. (A 4.7k-face
model renders in 0.03 s, so the problem is triangle count, not the pipeline per se.)

## Design (three levers, in priority order)
1. **Mesh simplification / decimation** (biggest win, user-requested): when a scene exceeds a
   triangle budget (e.g. > ~150k tris, tunable), decimate to a target count on load. Start with
   fast, dependency-free **vertex-clustering** (grid-snap vertices to a resolution chosen from the
   bounding box + target ratio, weld, drop degenerate triangles) — robust and O(n). (Quadric-error
   decimation is a nicer-but-heavier follow-up.) New `rgfx-3d` `simplify` module:
   `simplify_scene(&Scene, target_tris) -> Scene` (pure, tested). Keep the original for `info`.
   Surface `--simplify <ratio|target>` / `--no-simplify` CLI flags; auto-simplify on by default
   above the budget, and **log/show "simplified N→M tris"** in the status bar (no silent change).
2. **Adaptive interaction resolution** (rgfx-cli): while the user is actively rotating/zooming,
   render at a reduced framebuffer size (e.g. half), then re-render at full resolution once input
   goes idle. Keeps interaction responsive on heavy meshes.
3. **Parallel rasterization** (rgfx-3d, optional/after the above): use `rayon` to parallelize the
   triangle loop or use tiled/scanline parallelism into the framebuffer. Guard correctness
   (z-buffer races) — e.g. parallelize over screen tiles, each tile owning its pixels. Gate behind
   a feature or enable once profiled; must not change output vs the serial path (snapshot-equal).

## Acceptance criteria
- [ ] A >150k-tri mesh auto-simplifies on load to the target budget; `info` still reports the
      original counts; the status bar shows "simplified N→M".
- [ ] `--no-simplify` keeps full detail; `--simplify 0.25` hits ~25% of triangles (±tolerance).
- [ ] Simplified mesh stays watertight enough to render without holes/NaNs; bounds preserved.
- [ ] Interaction on the 705k mesh is materially faster (measure load+frame and a re-render).
- [ ] (If parallel raster landed) output is identical to the serial rasterizer on a fixed scene
      (snapshot-equal). clippy `-D warnings` clean.

## Tests required
- [ ] `simplify_scene`: a dense grid mesh decimates to ≤ target tris, keeps the bounding box
      within tolerance, produces no degenerate/zero-area triangles, and is deterministic.
- [ ] CLI flag parsing (`--simplify`, `--no-simplify`) + default-budget decision.
- [ ] (If parallel) parallel vs serial rasterize produce the same framebuffer for a known scene.
- [ ] A generated ~500k-tri mesh is handled without panics; (optional) a `#[ignore]` timing test.

## Out of scope
GPU rendering. LOD streaming. Skinned-mesh handling. Quadric decimation (future upgrade).

## Completion
- [x] Implemented (levers 1 + 2; lever 3 rayon left as optional follow-up) · [x] Gate green +
      timing check · [ ] PR opened · [ ] Merged

**Status:** 🟩 IN REVIEW (branch `task/036-perf-b`)

### Implementation notes
- **Vertex-clustering simplification** — new `rgfx-3d` `simplify` module with pure, tested
  `simplify_scene(&Scene, target_tris) -> Scene`: uniform grid from the scene bbox, weld vertices
  per cell to their mean, drop degenerate/zero-area triangles; binary-search the grid resolution
  for the finest fit `<= target`. Deterministic (first-seen cluster ids + mean accumulation).
  Auto-simplify on load above a tunable budget (`three_d.simplify_budget`, default 150k);
  CLI `--simplify <ratio|target>` / `--no-simplify`; status bar shows `N→M tris (simplified)`;
  `rgfx info` loads independently so it still reports the original counts.
- **Adaptive resolution** — the mesh viewer renders into a reduced framebuffer while the user is
  interacting (orbit/zoom/resize), upscaling into the full framebuffer, then snaps back to full
  resolution ~120 ms after input goes idle. Reuses one scratch framebuffer (no per-frame alloc).
- **Measured** (720k-tri generated mesh, 240×120 px, release): 215.9 ms/frame → 25.7 ms/frame,
  **8.39× per-frame**; including the one-time 442 ms simplify the 30-frame total drops 6.48 s →
  1.21 s (5.3×). Run `cargo test -p rgfx-3d --release -- --ignored --nocapture render_speedup`.
