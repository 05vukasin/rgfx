//! Benchmark: triangle rasterization (the full [`SceneRenderer`] path into a framebuffer).
//!
//! Measures [`rgfx_3d::Rasterizer`] rendering an auto-framed grid mesh into a reused framebuffer,
//! i.e. transform → near-clip → project → cull → barycentric fill → depth test → shade for every
//! triangle. The framebuffer size is fixed so the varying cost is per-triangle setup and coverage
//! as the mesh grows.
//!
//! [`SceneRenderer`]: rgfx_core::SceneRenderer

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use rgfx_3d::{Rasterizer, ShadingMode};
use rgfx_bench::{braille_render_size, framed_camera, standard_scene};
use rgfx_core::{Framebuffer, SceneRenderer, Viewport};
use std::hint::black_box;

/// Mesh sizes rasterized here. 1M triangles is intentionally omitted: at a fixed framebuffer size
/// its cost is dominated by per-triangle setup and it makes a full `cargo bench` run needlessly
/// long. The transform bench covers the 1M case, and the generators are unit-tested at 1M.
const RASTER_TRIANGLE_COUNTS: [usize; 3] = [1_000, 10_000, 100_000];

fn bench_rasterize(c: &mut Criterion) {
    // A typical interactive terminal viewport (100x40 cells → 200x160 Braille subpixels).
    let viewport = Viewport::new(100, 40);
    let (w, h) = braille_render_size(viewport);

    let mut group = c.benchmark_group("rasterize");
    group.sample_size(20);
    for &triangles in &RASTER_TRIANGLE_COUNTS {
        let scene = standard_scene(triangles);
        let camera = framed_camera(&scene, viewport.aspect(2, 4));
        let mut fb = Framebuffer::new(w, h);
        let mut rasterizer = Rasterizer::new(ShadingMode::Smooth);

        group.throughput(Throughput::Elements(triangles as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(triangles),
            &scene,
            |b, scene| {
                // `render` clears the target itself, so the reused framebuffer needs no manual reset.
                b.iter(|| {
                    rasterizer
                        .render(black_box(scene), black_box(&camera), &mut fb)
                        .expect("rendering a generated bench mesh never errors");
                });
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_rasterize);
criterion_main!(benches);
