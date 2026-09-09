//! rgfx-bench: procedural mesh generators and shared setup for the rgfx pipeline benchmarks.
//!
//! This crate is the home of the [criterion](https://docs.rs/criterion) benchmark suite for the
//! rgfx rendering pipeline (see the `benches/` directory and `BENCHMARKS.md`). The benches measure
//! the stages that matter for future optimization work: mesh transform, triangle rasterization,
//! Braille encoding of a framebuffer, and an end-to-end frame render.
//!
//! To keep the benches honest and free of large committed asset files, all input meshes are
//! generated procedurally at run time. The generators here produce meshes with an *exact* triangle
//! count so a benchmark can never silently drift onto a different-sized workload:
//!
//! - [`grid_mesh`] builds an indexed triangulated grid of `cols * rows * 2` triangles.
//! - [`mesh_with_triangles`] builds a mesh with *any* exact triangle count.
//! - [`BENCH_TRIANGLE_COUNTS`] is the standard set of sizes (1k / 10k / 100k / 1M) the suite runs.
//!
//! The remaining helpers ([`transform_positions`], [`standard_scene`], [`framed_camera`],
//! [`rendered_framebuffer`]) exist so each bench file can set up a realistic, comparable workload
//! from the crate's public API alone.
#![warn(missing_docs)]
#![forbid(unsafe_code)]

use glam::{Mat4, Vec3, Vec4};
use rgfx_3d::{Rasterizer, ShadingMode};
use rgfx_core::{Camera, Framebuffer, Mesh, Scene, SceneRenderer, Vertex, Viewport};
use rgfx_terminal::{SUBPIXEL_X, SUBPIXEL_Y};

/// The standard mesh sizes, in triangles, that the benchmark suite exercises.
///
/// Every entry is even, so [`mesh_with_triangles`] realizes each one as a pure triangulated grid
/// (no odd trailing triangle). Sizes span three orders of magnitude so a baseline captures both
/// per-triangle setup cost and bulk throughput.
pub const BENCH_TRIANGLE_COUNTS: [usize; 4] = [1_000, 10_000, 100_000, 1_000_000];

/// Builds an indexed, triangulated grid mesh in the `z = 0` plane spanning `[-1, 1]` on both axes.
///
/// The grid has `cols * rows` quads, each split into two triangles, for exactly `cols * rows * 2`
/// triangles wound counter-clockwise as seen from `+Z`. Vertices are shared between adjacent quads
/// (so the mesh is `(cols + 1) * (rows + 1)` vertices, not `3 * triangles`), which matches how a
/// real indexed mesh stresses the transform and rasterization stages. Every vertex carries a `+Z`
/// normal.
///
/// A `0` in either dimension yields an empty mesh (no vertices, no triangles).
#[must_use]
pub fn grid_mesh(cols: usize, rows: usize) -> Mesh {
    if cols == 0 || rows == 0 {
        return Mesh::default();
    }
    let (vcols, vrows) = (cols + 1, rows + 1);
    let mut vertices = Vec::with_capacity(vcols * vrows);
    for j in 0..vrows {
        let y = -1.0 + 2.0 * (j as f32 / rows as f32);
        for i in 0..vcols {
            let x = -1.0 + 2.0 * (i as f32 / cols as f32);
            vertices.push(Vertex::new(Vec3::new(x, y, 0.0), Vec3::Z));
        }
    }
    let mut indices = Vec::with_capacity(cols * rows * 6);
    let idx = |i: usize, j: usize| (j * vcols + i) as u32;
    for j in 0..rows {
        for i in 0..cols {
            let (v00, v10, v11, v01) = (idx(i, j), idx(i + 1, j), idx(i + 1, j + 1), idx(i, j + 1));
            // CCW winding seen from +Z (x right, y up).
            indices.extend_from_slice(&[v00, v10, v11, v00, v11, v01]);
        }
    }
    Mesh::new(vertices, indices)
}

/// Factors `quads` into a near-square `(cols, rows)` grid whose product is exactly `quads`.
///
/// Picks the largest divisor of `quads` that is `<= sqrt(quads)` as `rows`, giving the most
/// square-like grid available for a given quad count. Returns `(0, 0)` for `quads == 0`.
fn grid_dims(quads: usize) -> (usize, usize) {
    if quads == 0 {
        return (0, 0);
    }
    let mut rows = 1;
    let mut d = 1;
    while d * d <= quads {
        if quads % d == 0 {
            rows = d;
        }
        d += 1;
    }
    (quads / rows, rows)
}

/// Builds a mesh with *exactly* `triangles` triangles.
///
/// Even counts are realized as a pure triangulated [`grid_mesh`] (the near-square factorization of
/// `triangles / 2` quads). An odd count adds a single independent trailing triangle so any count,
/// even or odd, is reproduced exactly. `0` yields an empty mesh.
///
/// This exactness is what lets the benchmark suite label a workload "100k triangles" and be right.
#[must_use]
pub fn mesh_with_triangles(triangles: usize) -> Mesh {
    let quads = triangles / 2;
    let (cols, rows) = grid_dims(quads);
    let mut mesh = grid_mesh(cols, rows);
    if triangles % 2 == 1 {
        // Append one standalone triangle to hit an odd target exactly.
        let base = mesh.vertices.len() as u32;
        mesh.vertices
            .push(Vertex::new(Vec3::new(-1.0, -1.0, 0.0), Vec3::Z));
        mesh.vertices
            .push(Vertex::new(Vec3::new(1.0, -1.0, 0.0), Vec3::Z));
        mesh.vertices
            .push(Vertex::new(Vec3::new(0.0, 1.0, 0.0), Vec3::Z));
        mesh.indices.extend_from_slice(&[base, base + 1, base + 2]);
    }
    mesh
}

/// Transforms every vertex position of `mesh` by `matrix`, returning the homogeneous clip-space
/// coordinates.
///
/// This isolates the vertex-transform stage of the pipeline (model/world → clip) from clipping,
/// projection, and rasterization, so it can be benchmarked on its own. The position is extended to
/// a `Vec4` with `w = 1.0` before the multiply, matching the rasterizer's own transform.
#[must_use]
pub fn transform_positions(mesh: &Mesh, matrix: Mat4) -> Vec<Vec4> {
    mesh.vertices
        .iter()
        .map(|v| matrix * v.position.extend(1.0))
        .collect()
}

/// A single-mesh [`Scene`] holding a grid mesh with exactly `triangles` triangles.
#[must_use]
pub fn standard_scene(triangles: usize) -> Scene {
    Scene::new("bench", vec![mesh_with_triangles(triangles)])
}

/// A perspective camera auto-framed to fit `scene`'s bounding sphere at the given aspect ratio.
///
/// Falls back to a default perspective camera when the scene has no geometry.
#[must_use]
pub fn framed_camera(scene: &Scene, aspect: f32) -> Camera {
    let mut cam = Camera::perspective(aspect, 60_f32.to_radians());
    if let Some(bb) = scene.bounding_box() {
        rgfx_3d::frame_camera(&mut cam, &bb.bounding_sphere(), aspect);
    }
    cam
}

/// The framebuffer pixel size a Braille encoder needs for a `viewport` of terminal cells.
#[must_use]
pub fn braille_render_size(viewport: Viewport) -> (usize, usize) {
    viewport.render_size(SUBPIXEL_X, SUBPIXEL_Y)
}

/// Renders `scene` (smooth-shaded) once into a fresh framebuffer sized for `viewport`, returning
/// the populated framebuffer.
///
/// Handy for the Braille-encode benchmark, which needs a realistic, non-empty framebuffer to
/// encode but should not pay the rasterization cost inside its measured loop.
///
/// # Panics
///
/// Panics only if the rasterizer reports an error, which for a valid generated mesh cannot happen;
/// this is bench-only setup code, not a library invariant.
#[must_use]
pub fn rendered_framebuffer(scene: &Scene, viewport: Viewport) -> Framebuffer {
    let (w, h) = braille_render_size(viewport);
    let mut fb = Framebuffer::new(w, h);
    let camera = framed_camera(scene, viewport.aspect(SUBPIXEL_X, SUBPIXEL_Y));
    let mut rasterizer = Rasterizer::new(ShadingMode::Smooth);
    rasterizer
        .render(scene, &camera, &mut fb)
        .expect("rendering a generated bench mesh never errors");
    fb
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_mesh_has_exact_triangle_count() {
        // cols * rows * 2 triangles, and shared vertices => (cols+1)*(rows+1) vertices.
        for &(c, r) in &[(1usize, 1usize), (25, 20), (7, 3), (100, 50)] {
            let mesh = grid_mesh(c, r);
            assert_eq!(
                mesh.triangle_count(),
                c * r * 2,
                "grid {c}x{r} triangle count"
            );
            assert_eq!(
                mesh.vertices.len(),
                (c + 1) * (r + 1),
                "grid {c}x{r} vertex count"
            );
            assert_eq!(mesh.indices.len() % 3, 0, "index count divisible by 3");
        }
    }

    #[test]
    fn empty_dimension_yields_empty_mesh() {
        assert_eq!(grid_mesh(0, 10).triangle_count(), 0);
        assert_eq!(grid_mesh(10, 0).triangle_count(), 0);
        assert!(grid_mesh(0, 0).vertices.is_empty());
    }

    #[test]
    fn grid_dims_multiply_back_to_quads() {
        for quads in [0usize, 1, 2, 3, 500, 5_000, 50_000, 500_000, 499_999] {
            let (c, r) = grid_dims(quads);
            assert_eq!(c * r, quads, "grid_dims({quads}) must factor exactly");
        }
    }

    #[test]
    fn standard_counts_are_exact() {
        for &n in &BENCH_TRIANGLE_COUNTS {
            let mesh = mesh_with_triangles(n);
            assert_eq!(mesh.triangle_count(), n, "mesh_with_triangles({n})");
        }
    }

    #[test]
    fn arbitrary_counts_including_odd_are_exact() {
        for n in [0usize, 1, 2, 3, 7, 999, 1_001, 12_345, 250_001] {
            let mesh = mesh_with_triangles(n);
            assert_eq!(mesh.triangle_count(), n, "mesh_with_triangles({n})");
            assert_eq!(mesh.indices.len(), n * 3, "index count for {n} triangles");
        }
    }

    #[test]
    fn generated_mesh_indices_are_all_in_range() {
        // A mesh whose indices point out of range would panic or error in the rasterizer; verify
        // the generators never produce a dangling index.
        for n in [2usize, 7, 1_000, 4_321] {
            let mesh = mesh_with_triangles(n);
            let max = mesh.vertices.len() as u32;
            assert!(
                mesh.indices.iter().all(|&i| i < max),
                "all indices in range for {n} triangles"
            );
        }
    }

    #[test]
    fn transform_positions_applies_matrix_to_every_vertex() {
        let mesh = grid_mesh(2, 2);
        let out = transform_positions(&mesh, Mat4::IDENTITY);
        assert_eq!(out.len(), mesh.vertices.len());
        // Identity transform leaves xyz unchanged with w = 1.
        for (v, p) in mesh.vertices.iter().zip(&out) {
            assert_eq!(p.truncate(), v.position);
            assert_eq!(p.w, 1.0);
        }
    }

    #[test]
    fn rendered_framebuffer_is_non_empty_and_sized_to_viewport() {
        let scene = standard_scene(1_000);
        let vp = Viewport::new(40, 20);
        let fb = rendered_framebuffer(&scene, vp);
        assert_eq!((fb.width(), fb.height()), braille_render_size(vp));
        // The framed grid faces the camera, so at least some pixels are covered.
        let covered = fb.color().iter().any(|c| c.a > 0.0);
        assert!(covered, "framed grid mesh must cover some pixels");
    }
}
