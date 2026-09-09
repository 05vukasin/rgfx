//! Benchmark: Braille encoding of a framebuffer into a terminal cell grid.
//!
//! Measures [`rgfx_terminal::BrailleEncoder::encode`] turning a pre-rendered framebuffer into a
//! [`TerminalFrame`] across several viewport sizes. The framebuffer is rendered once, outside the
//! measured loop, so the benchmark isolates the encode cost (which scales with the cell count).
//!
//! [`TerminalFrame`]: rgfx_core::TerminalFrame

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use rgfx_bench::{rendered_framebuffer, standard_scene};
use rgfx_core::{TerminalEncoder, Viewport};
use rgfx_terminal::BrailleEncoder;
use std::hint::black_box;

/// Viewport sizes in terminal cells: small, a typical terminal, and a large one.
const VIEWPORTS: [(u16, u16); 3] = [(40, 20), (100, 40), (240, 80)];

fn bench_braille(c: &mut Criterion) {
    // A moderately dense scene so a realistic fraction of subpixels are lit.
    let scene = standard_scene(10_000);
    let encoder = BrailleEncoder::new();

    let mut group = c.benchmark_group("braille_encode");
    for &(cols, rows) in &VIEWPORTS {
        let viewport = Viewport::new(cols, rows);
        let fb = rendered_framebuffer(&scene, viewport);

        group.throughput(Throughput::Elements(cols as u64 * rows as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{cols}x{rows}")),
            &fb,
            |b, fb| {
                b.iter(|| encoder.encode(black_box(fb), black_box(viewport)));
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_braille);
criterion_main!(benches);
