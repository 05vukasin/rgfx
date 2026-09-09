//! Benchmark: an end-to-end frame render.
//!
//! Measures the whole terminal-3D pipeline for one frame: rasterize the scene into a framebuffer
//! ([`rgfx_3d::Rasterizer`]), encode it to Braille cells ([`rgfx_terminal::BrailleEncoder`]), and
//! serialize those cells to an ANSI byte stream ([`rgfx_terminal::AnsiSerializer`]). The
//! framebuffer is reused across iterations, matching the engine's no-allocation-per-frame rule.
//!
//! There is no terminal-diff stage in this bench: the frame engine / diff (backlog task 007) does
//! not exist in `rgfx-terminal` yet, so the pipeline ends at serialization. Add a diff bench here
//! once that lands.

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use rgfx_3d::{Rasterizer, ShadingMode};
use rgfx_bench::{braille_render_size, framed_camera, standard_scene};
use rgfx_core::{Framebuffer, SceneRenderer, TerminalEncoder, Viewport};
use rgfx_terminal::{AnsiSerializer, BrailleEncoder, ColorMode};
use std::hint::black_box;

/// Mesh sizes for the end-to-end frame. 1M is omitted for the same reason as the rasterize bench:
/// it dominates total `cargo bench` time without changing the shape of the result.
const FRAME_TRIANGLE_COUNTS: [usize; 3] = [1_000, 10_000, 100_000];

fn bench_frame(c: &mut Criterion) {
    let viewport = Viewport::new(100, 40);
    let (w, h) = braille_render_size(viewport);

    let mut group = c.benchmark_group("frame_render");
    group.sample_size(20);
    for &triangles in &FRAME_TRIANGLE_COUNTS {
        let scene = standard_scene(triangles);
        let camera = framed_camera(&scene, viewport.aspect(2, 4));
        let mut fb = Framebuffer::new(w, h);
        let mut rasterizer = Rasterizer::new(ShadingMode::Smooth);
        let encoder = BrailleEncoder::new();
        let serializer = AnsiSerializer::new(ColorMode::None);

        group.throughput(Throughput::Elements(triangles as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(triangles),
            &scene,
            |b, scene| {
                b.iter(|| {
                    rasterizer
                        .render(black_box(scene), black_box(&camera), &mut fb)
                        .expect("rendering a generated bench mesh never errors");
                    let terminal_frame = encoder.encode(&fb, viewport);
                    black_box(serializer.serialize(&terminal_frame))
                });
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_frame);
criterion_main!(benches);
