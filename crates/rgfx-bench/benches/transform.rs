//! Benchmark: the vertex-transform stage (model/world positions → clip space).
//!
//! Measures [`rgfx_bench::transform_positions`] across the standard mesh sizes, transforming every
//! vertex by an auto-framed camera's view-projection matrix. This is the cheapest per-vertex stage
//! and scales linearly with vertex count.

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use rgfx_bench::{BENCH_TRIANGLE_COUNTS, framed_camera, mesh_with_triangles, transform_positions};
use rgfx_core::Scene;
use std::hint::black_box;

fn bench_transform(c: &mut Criterion) {
    let mut group = c.benchmark_group("transform");
    for &triangles in &BENCH_TRIANGLE_COUNTS {
        let mesh = mesh_with_triangles(triangles);
        let scene = Scene::new("bench", vec![mesh.clone()]);
        let camera = framed_camera(&scene, 16.0 / 9.0);
        let view_proj = camera.view_projection();

        // Throughput is reported per vertex: the transform stage is per-vertex, not per-triangle.
        group.throughput(Throughput::Elements(mesh.vertices.len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(triangles), &mesh, |b, mesh| {
            b.iter(|| transform_positions(black_box(mesh), black_box(view_proj)));
        });
    }
    group.finish();
}

criterion_group!(benches, bench_transform);
criterion_main!(benches);
