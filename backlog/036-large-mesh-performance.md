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
- [x] Implemented (simplification + adaptive resolution) · [x] Gate green + timing check · [ ] PR opened · [ ] Merged

## Implementation notes
- **Vertex-clustering simplification** landed in `rgfx-3d::simplify` (`simplify_scene`,
  `decide_simplify`, `SimplifyDecision`). Grid resolution is binary-searched so the welded result
  lands at/under the triangle budget; welded vertices are per-cell averages (bounds preserved) and
  degenerate/zero-area faces are dropped. Auto-applies above `AUTO_SIMPLIFY_BUDGET` (150k), with
  `--simplify <ratio|target>` / `--no-simplify` flags and a "simplified N→M" status readout.
  `rgfx info` loads separately, so it keeps the originals.
- **Adaptive resolution** in `mesh_viewer`: while orbiting/zooming the scene renders into a reused
  half-size scratch buffer and is upscaled to fill the screen; a full-resolution frame is redrawn
  once input settles (`IDLE_SETTLE`).
- **Measured** (705,672-tri mesh → 149,058 after auto-simplify, per-frame rasterization only):
  ~4–5x faster per frame (e.g. 131 ms → 27 ms). Adaptive half-res adds interaction headroom on top
  (most when fill dominates).
- **Rayon tiled parallel rasterization** (optional) intentionally deferred: it is a larger, riskier
  change (z-buffer races / snapshot-equality) and the two required levers already deliver the win.

**Status:** ✅ IMPLEMENTED (PR open)
