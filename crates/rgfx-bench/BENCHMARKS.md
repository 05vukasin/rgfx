# rgfx benchmarks

Criterion benchmarks for the rgfx rendering pipeline. They establish a baseline so future
optimization work (backlog task 025: Rayon / SIMD / tiling) can be measured against a known
starting point, not guessed at.

The benches live in this `rgfx-bench` crate rather than in `rgfx-3d` / `rgfx-terminal` so that the
measured crates stay free of criterion dev-dependencies and their `cargo test` stays fast. All
input meshes are generated **procedurally** (see `src/lib.rs`) — no large asset files are committed
to the repo, and the generators are unit-tested to produce exact triangle counts so a bench can
never silently drift onto a different-sized workload.

## Running

```sh
# Run the whole suite (slow — several minutes at default sampling):
cargo bench -p rgfx-bench

# Run one stage:
cargo bench -p rgfx-bench --bench transform
cargo bench -p rgfx-bench --bench rasterize
cargo bench -p rgfx-bench --bench braille
cargo bench -p rgfx-bench --bench frame

# Faster, lower-fidelity pass (what the baseline below was captured with):
cargo bench -p rgfx-bench -- --warm-up-time 0.5 --measurement-time 1.5

# Filter to a single case by id:
cargo bench -p rgfx-bench --bench rasterize -- 100000

# Just make sure the benches compile, without running them:
cargo bench -p rgfx-bench --no-run
```

Criterion writes full reports (including HTML with plots, when gnuplot/plotters is available) under
`target/criterion/`. To compare a change against a saved run, use criterion's baselines:

```sh
cargo bench -p rgfx-bench -- --save-baseline before
# ... make your optimization ...
cargo bench -p rgfx-bench -- --baseline before
```

Benches are intentionally **not** on CI's default path — they are slow and machine-sensitive. Run
them locally (or in a dedicated manual workflow) when you touch the transform, rasterizer, or
encoder hot paths.

## What each bench measures

| Bench       | Stage                     | Public API exercised                                              |
| ----------- | ------------------------- | ----------------------------------------------------------------- |
| `transform` | vertex transform → clip   | `transform_positions` (`Mat4 * Vec4` per vertex)                  |
| `rasterize` | triangle rasterization    | `Rasterizer::render` (transform → clip → cull → fill → depth)     |
| `braille`   | framebuffer → cells       | `BrailleEncoder::encode`                                          |
| `frame`     | end-to-end frame          | `render` + `BrailleEncoder::encode` + `AnsiSerializer::serialize` |

The meshes are triangulated grids spanning the view, so every triangle is on-screen and
`rasterize` measures real coverage + per-triangle setup rather than trivially-culled work. The
framebuffer is reused across iterations (no per-frame allocation), matching the engine's
performance rules. Throughput is reported per **vertex** for `transform` and per **triangle** for
`rasterize`/`frame`, and per **cell** for `braille`.

### Sizes

- Mesh sizes: `transform` runs 1k / 10k / 100k / **1M** triangles. `rasterize` and `frame` run
  1k / 10k / 100k — 1M is skipped there because at a fixed framebuffer size its cost is dominated
  by per-triangle setup and it makes a full run needlessly long. The generators are still
  unit-tested at 1M, and `transform` covers the 1M case.
- Viewports (Braille cells → subpixels): `braille` runs 40×20, 100×40, and 240×80 cells
  (i.e. 80×80, 200×160, and 480×320 framebuffer pixels).

### Not benchmarked yet: terminal diff

Task 026 lists a "terminal diff of two frames" bench. There is no frame-engine / diff stage in
`rgfx-terminal` today (backlog task 007 is not implemented), so the `frame` bench ends at ANSI
serialization. Add a diff bench to `benches/frame.rs` once a diffing frame engine lands.

## Baseline

Captured with `--warm-up-time 0.5 --measurement-time 1.5` (the fast pass above), release profile
(`lto = "thin"`, `codegen-units = 1`). **These numbers are hardware-specific** — treat them as a
shape/order-of-magnitude reference, and always re-capture a local baseline before comparing a
change on your own machine.

- Machine: single-threaded CPU rasterizer (no Rayon/SIMD yet), Linux x86-64.
- Times are the criterion median estimate.

### transform (per vertex → clip space)

| Triangles | Vertices | Median time | Throughput   |
| --------- | -------- | ----------- | ------------ |
| 1,000     | 546      | 483 ns      | ~1.13 Gelem/s |
| 10,000    | 5,151    | 4.68 µs     | ~1.10 Gelem/s |
| 100,000   | 50,451   | 48.6 µs     | ~1.04 Gelem/s |
| 1,000,000 | 501,426  | 917 µs      | ~547 Melem/s  |

### rasterize (`Rasterizer::render` into a 200×160 framebuffer)

| Triangles | Median time | Throughput    |
| --------- | ----------- | ------------- |
| 1,000     | 364 µs      | ~2.75 Mtri/s  |
| 10,000    | 1.37 ms     | ~7.3 Mtri/s   |
| 100,000   | 10.3 ms     | ~9.7 Mtri/s   |

### braille (`BrailleEncoder::encode`)

| Viewport (cells) | Cells  | Median time | Throughput   |
| ---------------- | ------ | ----------- | ------------ |
| 40×20            | 800    | 21.6 µs     | ~37 Mcell/s  |
| 100×40           | 4,000  | 106 µs      | ~38 Mcell/s  |
| 240×80           | 19,200 | 512 µs      | ~37 Mcell/s  |

### frame (render + Braille encode + ANSI serialize, 100×40 cells)

| Triangles | Median time | Throughput   |
| --------- | ----------- | ------------ |
| 1,000     | 1.07 ms     | ~0.94 Mtri/s |
| 10,000    | 2.80 ms     | ~3.6 Mtri/s  |
| 100,000   | 16.9 ms     | ~5.9 Mtri/s  |

### Reading the baseline

- **Transform is cheap** (~1 Gelem/s) and not the bottleneck; the drop at 1M reflects the ~8 MB
  result `Vec` spilling out of cache.
- **Rasterization dominates a frame.** Per-triangle throughput *improves* with size (2.7→9.7
  Mtri/s) because fixed per-render costs amortize while fill cost stays roughly constant at a fixed
  framebuffer size — a signal that per-triangle setup, not fill, leads at small sizes.
- **Braille encode is steady** at ~37 Mcell/s regardless of viewport, so a 100×40 frame spends
  ~0.1 ms in the encoder — small next to rasterization but not free.
- These are the numbers task 025 should try to beat; re-run with `--save-baseline` / `--baseline`
  to quantify any optimization.
