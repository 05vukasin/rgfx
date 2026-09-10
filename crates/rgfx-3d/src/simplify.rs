//! Fast, dependency-free mesh simplification by **vertex clustering**.
//!
//! Large meshes (hundreds of thousands of triangles) are expensive to rasterize every frame. When
//! a scene exceeds a triangle budget the viewer decimates it on load with [`simplify_scene`]: a
//! uniform grid is laid over the scene's bounding box, every vertex is snapped to the cell it falls
//! in, all vertices sharing a cell are welded to their average, and triangles that collapse to a
//! line or a point (two or three corners in the same cell, or three welded corners left collinear)
//! are dropped. This is the classic Rossignac–Borrel vertex-clustering scheme: robust, `O(n)` per
//! pass, and it never fails on non-manifold or self-intersecting input.
//!
//! The result is a *new* [`Scene`] — the original is left untouched so callers can keep reporting
//! its true counts (e.g. `rgfx info`). Simplification is pure geometry: it knows nothing about the
//! renderer, the terminal, or shading.
//!
//! # Determinism
//!
//! For a fixed `(scene, target_tris)` the output is byte-for-byte identical run to run: cluster
//! ids are assigned in first-seen vertex order, and each cluster representative is the arithmetic
//! mean of its members accumulated in that same order, so no hash-iteration order leaks into the
//! result.
//!
//! # Example
//!
//! ```
//! use rgfx_3d::{primitives, simplify_scene};
//! use rgfx_core::Scene;
//!
//! let scene = Scene::new("cube", vec![primitives::cube(1.0)]);
//! let simplified = simplify_scene(&scene, 4);
//! assert!(simplified.triangle_count() <= scene.triangle_count());
//! ```

use std::collections::HashMap;

use glam::Vec3;
use rgfx_core::{Mesh, Scene, Vertex};

/// The finest grid (cells along the longest axis) the search will ever try. Bounds the work done
/// for meshes whose target is very close to the original count; past this the mesh is returned as
/// simplified as this resolution allows (still `<= target_tris`).
const MAX_DIVISIONS: u32 = 4096;

/// Simplifies every mesh in `scene` by vertex clustering so the returned scene has **at most**
/// `target_tris` triangles, choosing the finest uniform grid that still meets the budget.
///
/// The scene's overall axis-aligned bounding box sets the grid extent, so all meshes are clustered
/// against one shared, cube-celled grid (each mesh still welds only its own vertices — meshes stay
/// separate). The returned scene:
///
/// - has `triangle_count() <= target_tris` (and `<= scene.triangle_count()`);
/// - contains no degenerate triangles (every surviving triangle has strictly positive area);
/// - has vertex positions that are averages of the originals, so its bounding box is contained in
///   the original's (never larger) and shrinks by at most about one cell per side.
///
/// When `scene` already fits the budget (or `target_tris == 0`, or the scene has no finite extent)
/// the scene is returned cloned unchanged.
pub fn simplify_scene(scene: &Scene, target_tris: usize) -> Scene {
    let total = scene.triangle_count();
    if target_tris == 0 || total <= target_tris {
        return scene.clone();
    }

    // The grid spans the whole scene; a degenerate (zero-extent) or empty scene cannot be gridded.
    let Some(bbox) = scene.bounding_box() else {
        return scene.clone();
    };
    let size = bbox.size();
    let longest = size.x.max(size.y).max(size.z);
    if longest <= 0.0 || longest.is_nan() {
        return scene.clone();
    }

    // Find the finest division count whose clustered scene still fits the budget. Finer grids keep
    // more distinct cells and therefore more triangles, so the triangle count rises with the
    // division count; binary-search the largest count that stays `<= target_tris`.
    //
    // Grow an upper bound until it overshoots the budget (or hits the cap), so the search brackets
    // the target instead of assuming a fixed ceiling.
    let mut hi = 2u32;
    while hi < MAX_DIVISIONS
        && cluster_scene(scene, bbox.min, longest, hi).triangle_count() <= target_tris
    {
        hi = (hi * 2).min(MAX_DIVISIONS);
    }

    // `divisions == 1` welds almost everything together (≈ 0 triangles), so it is always a valid
    // lower bound and seeds `best` with a scene that already satisfies the budget.
    let mut best = cluster_scene(scene, bbox.min, longest, 1);
    let (mut lo, mut hi) = (1u32, hi);
    while lo <= hi {
        let mid = lo + (hi - lo) / 2;
        let candidate = cluster_scene(scene, bbox.min, longest, mid);
        if candidate.triangle_count() <= target_tris {
            best = candidate;
            lo = mid + 1;
        } else {
            if mid == 1 {
                break;
            }
            hi = mid - 1;
        }
    }
    best
}

/// Clusters every mesh in the scene against a shared grid of `divisions` cube cells along the
/// longest axis (cell size `longest / divisions`), anchored at `origin` (the scene bbox min).
fn cluster_scene(scene: &Scene, origin: Vec3, longest: f32, divisions: u32) -> Scene {
    let cell = longest / divisions as f32;
    let inv_cell = 1.0 / cell;
    let meshes = scene
        .meshes
        .iter()
        .map(|m| cluster_mesh(m, origin, inv_cell))
        .collect();
    Scene::new(scene.name.clone(), meshes)
}

/// Clusters a single mesh: welds vertices sharing a grid cell to their average and rebuilds the
/// index list, dropping any triangle that collapsed to a line or point.
fn cluster_mesh(mesh: &Mesh, origin: Vec3, inv_cell: f32) -> Mesh {
    // Map each original vertex to a cluster id (assigned in first-seen order for determinism) while
    // accumulating each cluster's position/normal sum and member count.
    let mut cell_to_cluster: HashMap<[i64; 3], u32> = HashMap::new();
    let mut remap: Vec<u32> = Vec::with_capacity(mesh.vertices.len());
    let mut pos_sum: Vec<Vec3> = Vec::new();
    let mut nrm_sum: Vec<Vec3> = Vec::new();
    let mut counts: Vec<u32> = Vec::new();

    for v in &mesh.vertices {
        let key = cell_key(v.position, origin, inv_cell);
        let id = *cell_to_cluster.entry(key).or_insert_with(|| {
            let id = pos_sum.len() as u32;
            pos_sum.push(Vec3::ZERO);
            nrm_sum.push(Vec3::ZERO);
            counts.push(0);
            id
        });
        let idx = id as usize;
        pos_sum[idx] += v.position;
        nrm_sum[idx] += v.normal;
        counts[idx] += 1;
        remap.push(id);
    }

    // Finalize each cluster to the mean position and averaged (renormalized) normal.
    let vertices: Vec<Vertex> = pos_sum
        .iter()
        .zip(&nrm_sum)
        .zip(&counts)
        .map(|((&p, &n), &c)| {
            let inv = 1.0 / c as f32;
            Vertex::new(p * inv, (n * inv).normalize_or_zero())
        })
        .collect();

    // Rebuild triangles through the remap, dropping degenerate ones (repeated corner) and any that
    // are left with zero area (three distinct-but-collinear cluster representatives).
    let mut indices: Vec<u32> = Vec::with_capacity(mesh.indices.len());
    for tri in mesh.indices.chunks_exact(3) {
        let (a, b, c) = (
            remap[tri[0] as usize],
            remap[tri[1] as usize],
            remap[tri[2] as usize],
        );
        if a == b || b == c || a == c {
            continue;
        }
        let pa = vertices[a as usize].position;
        let pb = vertices[b as usize].position;
        let pc = vertices[c as usize].position;
        if (pb - pa).cross(pc - pa).length_squared() <= f32::MIN_POSITIVE {
            continue;
        }
        indices.extend_from_slice(&[a, b, c]);
    }

    Mesh::new(vertices, indices)
}

/// The integer grid cell a point falls in, for cell size `1 / inv_cell` anchored at `origin`.
#[inline]
fn cell_key(p: Vec3, origin: Vec3, inv_cell: f32) -> [i64; 3] {
    let g = (p - origin) * inv_cell;
    [g.x.floor() as i64, g.y.floor() as i64, g.z.floor() as i64]
}

#[cfg(test)]
mod tests {
    use super::*;
    use rgfx_core::BoundingBox;

    /// A dense, axis-aligned grid mesh in the `z = 0` plane spanning `[0, 1]^2`, subdivided into
    /// `n * n` quads (two triangles each). Vertices are shared between adjacent quads.
    fn grid_mesh(n: usize) -> Mesh {
        let stride = n + 1;
        let mut vertices = Vec::with_capacity(stride * stride);
        for j in 0..stride {
            for i in 0..stride {
                let x = i as f32 / n as f32;
                let y = j as f32 / n as f32;
                vertices.push(Vertex::new(Vec3::new(x, y, 0.0), Vec3::Z));
            }
        }
        let mut indices = Vec::with_capacity(n * n * 6);
        for j in 0..n {
            for i in 0..n {
                let a = (j * stride + i) as u32;
                let b = a + 1;
                let c = a + stride as u32;
                let d = c + 1;
                indices.extend_from_slice(&[a, b, c, b, d, c]);
            }
        }
        Mesh::new(vertices, indices)
    }

    fn grid_scene(n: usize) -> Scene {
        Scene::new("grid", vec![grid_mesh(n)])
    }

    /// Every triangle in the scene has strictly positive area (no degenerate output).
    fn assert_no_zero_area(scene: &Scene) {
        for mesh in &scene.meshes {
            for tri in mesh.indices.chunks_exact(3) {
                let a = mesh.vertices[tri[0] as usize].position;
                let b = mesh.vertices[tri[1] as usize].position;
                let c = mesh.vertices[tri[2] as usize].position;
                assert!(tri[0] != tri[1] && tri[1] != tri[2] && tri[0] != tri[2]);
                assert!(
                    (b - a).cross(c - a).length() > 0.0,
                    "triangle {tri:?} is zero-area"
                );
            }
        }
    }

    #[test]
    fn simplify_hits_at_most_target_and_reduces() {
        let scene = grid_scene(80); // 6400 quads -> 12800 triangles
        let original = scene.triangle_count();
        for &target in &[50usize, 200, 1000, 4000] {
            let out = simplify_scene(&scene, target);
            assert!(
                out.triangle_count() <= target,
                "target {target}: got {}",
                out.triangle_count()
            );
            assert!(out.triangle_count() < original, "must actually decimate");
            assert_no_zero_area(&out);
        }
    }

    #[test]
    fn simplify_is_deterministic() {
        let scene = grid_scene(60);
        let a = simplify_scene(&scene, 500);
        let b = simplify_scene(&scene, 500);
        assert_eq!(a.triangle_count(), b.triangle_count());
        // Compare full geometry, not just counts.
        assert_eq!(a.meshes.len(), b.meshes.len());
        for (ma, mb) in a.meshes.iter().zip(&b.meshes) {
            assert_eq!(ma.indices, mb.indices);
            assert_eq!(ma.vertices.len(), mb.vertices.len());
            for (va, vb) in ma.vertices.iter().zip(&mb.vertices) {
                assert_eq!(va.position, vb.position);
                assert_eq!(va.normal, vb.normal);
            }
        }
    }

    #[test]
    fn simplify_preserves_bounds_within_tolerance() {
        let scene = grid_scene(64);
        let orig = scene.bounding_box().unwrap();
        let out = simplify_scene(&scene, 300);
        let simp = out.bounding_box().unwrap();

        // Cluster representatives are averages of original vertices, so the simplified box is
        // contained in the original (never larger).
        let eps = 1e-5;
        assert!(simp.min.x >= orig.min.x - eps && simp.min.y >= orig.min.y - eps);
        assert!(simp.max.x <= orig.max.x + eps && simp.max.y <= orig.max.y + eps);

        // And it should not have collapsed: it still covers most of the original extent (within a
        // couple of grid cells of the full size).
        let orig_size = orig.size();
        let simp_size = simp.size();
        let tol = orig_size * 0.25;
        assert!(simp_size.x >= orig_size.x - tol.x, "x extent collapsed");
        assert!(simp_size.y >= orig_size.y - tol.y, "y extent collapsed");
    }

    #[test]
    fn already_small_scene_is_returned_unchanged() {
        let scene = grid_scene(4); // 32 triangles
        let out = simplify_scene(&scene, 10_000);
        assert_eq!(out.triangle_count(), scene.triangle_count());
    }

    #[test]
    fn zero_target_returns_clone() {
        let scene = grid_scene(8);
        let out = simplify_scene(&scene, 0);
        assert_eq!(out.triangle_count(), scene.triangle_count());
    }

    #[test]
    fn degenerate_zero_extent_scene_is_safe() {
        // All vertices at one point: no finite extent, nothing to grid; returned unchanged.
        let mesh = Mesh::new(vec![Vertex::from_position(Vec3::ZERO); 3], vec![0, 1, 2]);
        let scene = Scene::new("point", vec![mesh]);
        let out = simplify_scene(&scene, 1);
        assert_eq!(out.meshes.len(), 1);
    }

    #[test]
    fn large_mesh_simplifies_without_panic() {
        // A ~500k-triangle generated mesh must simplify to the budget without panicking and within
        // the target. grid_mesh(n) yields 2*n*n triangles; n = 500 -> 500_000 triangles.
        let scene = grid_scene(500);
        assert_eq!(scene.triangle_count(), 500_000);
        let out = simplify_scene(&scene, 50_000);
        assert!(out.triangle_count() <= 50_000);
        assert!(out.triangle_count() > 0);
        assert_no_zero_area(&out);
    }

    /// Amortized per-frame render speedup from simplification on a large mesh. Ignored by default
    /// (timing-sensitive); run with `cargo test -p rgfx-3d -- --ignored --nocapture render_speedup`.
    #[test]
    #[ignore]
    fn render_speedup_on_large_mesh() {
        use rgfx_core::{BoundingBox, Camera, Framebuffer, SceneRenderer};
        use std::time::Instant;

        let scene = grid_scene(600); // 720k triangles, like the 705k user report
        let bbox = scene.bounding_box().unwrap();
        let sphere = BoundingBox::bounding_sphere(&bbox);

        let mut cam = Camera::perspective(2.0, 60_f32.to_radians());
        crate::frame_camera(&mut cam, &sphere, 2.0);

        let mut fb = Framebuffer::new(240, 120);
        let mut ras = crate::Rasterizer::new(crate::ShadingMode::Flat);
        let frames = 30;

        // Full-detail: what an orbit frame costs today.
        let t = Instant::now();
        for _ in 0..frames {
            ras.render(&scene, &cam, &mut fb).unwrap();
        }
        let full = t.elapsed();

        // Simplify once (amortized over the interaction), then render.
        let t = Instant::now();
        let simplified = simplify_scene(&scene, 150_000);
        let simplify_cost = t.elapsed();
        let t = Instant::now();
        for _ in 0..frames {
            ras.render(&simplified, &cam, &mut fb).unwrap();
        }
        let simp = t.elapsed();

        println!(
            "full: {} tris, {frames} frames in {full:?} ({:?}/frame)",
            scene.triangle_count(),
            full / frames
        );
        println!("simplify once: {simplify_cost:?}");
        println!(
            "simplified: {} tris, {frames} frames in {simp:?} ({:?}/frame)",
            simplified.triangle_count(),
            simp / frames
        );
        println!(
            "per-frame speedup: {:.2}x",
            full.as_secs_f64() / simp.as_secs_f64()
        );
    }

    #[test]
    fn bounding_box_helper_available() {
        // Sanity: the test helper's box matches the unit square (guards the fixture).
        let bb =
            BoundingBox::from_points(grid_mesh(2).vertices.iter().map(|v| v.position)).unwrap();
        assert_eq!(bb.min, Vec3::ZERO);
        assert_eq!(bb.max, Vec3::new(1.0, 1.0, 0.0));
    }
}
