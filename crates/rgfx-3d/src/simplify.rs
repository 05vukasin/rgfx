//! Mesh simplification by vertex clustering.
//!
//! Large meshes (hundreds of thousands of triangles) are slow to rasterize every frame. This
//! module decimates a [`Scene`] down to a triangle budget with a fast, dependency-free
//! **vertex-clustering** algorithm:
//!
//! 1. Overlay a uniform grid over the scene's bounding box.
//! 2. Snap every vertex to the cell it falls in, welding all vertices that share a cell into a
//!    single representative vertex (the average of the cell's members, so the result stays inside
//!    the original bounds).
//! 3. Remap the triangles to the welded vertices and drop any triangle that became degenerate (two
//!    or more corners welded together, or a zero-area face).
//!
//! A coarse grid welds aggressively (few triangles survive); a fine grid barely changes the mesh.
//! [`simplify_scene`] binary-searches the grid resolution so the result lands at or below the
//! requested triangle budget while preserving as much detail as the budget allows. The algorithm
//! is pure (no I/O), `O(n)` per grid trial, and deterministic: the same scene and budget always
//! produce the same output.
//!
//! # Example
//!
//! ```
//! use rgfx_core::Scene;
//! use rgfx_3d::simplify::simplify_scene;
//!
//! # fn demo(dense: &Scene) {
//! let simplified = simplify_scene(dense, 10_000);
//! assert!(simplified.triangle_count() <= 10_000);
//! # }
//! ```

use std::collections::HashMap;

use glam::Vec3;
use rgfx_core::{BoundingBox, Mesh, Scene, Vertex};

/// The largest grid resolution (cells along the longest bounding-box axis) the resolution search
/// will consider. Past this, welding is so weak that the mesh is effectively unchanged, so there
/// is no point searching finer.
const MAX_DIVISIONS: i32 = 4096;

/// Whether a simplification request should run, and the triangle budget it should target.
///
/// This is the pure decision that backs the CLI's `--simplify` / `--no-simplify` flags and the
/// automatic on-load budget, factored out so it can be unit-tested without a terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SimplifyDecision {
    /// Leave the mesh at full detail.
    Skip,
    /// Simplify down to this many triangles.
    To(usize),
}

/// Decides whether to simplify a mesh with `original_tris` triangles, and to what budget.
///
/// The inputs mirror the CLI surface:
/// - `no_simplify`: the `--no-simplify` flag; when set, always [`SimplifyDecision::Skip`].
/// - `request`: the `--simplify <VALUE>` flag. A value in `(0.0, 1.0]` is a *ratio* of the original
///   triangle count; a value `> 1.0` is an *absolute* target count. A non-positive value is
///   ignored (treated as absent).
/// - `auto_budget`: the automatic on-load threshold. With no explicit request, a mesh above this
///   budget is simplified down to it; a mesh at or below it is left alone.
///
/// An explicit request that would not actually reduce the mesh (target `>=` original) yields
/// [`SimplifyDecision::Skip`], so `info`-style full detail is never needlessly rebuilt. The target
/// is always clamped to at least one triangle.
pub fn decide_simplify(
    original_tris: usize,
    request: Option<f32>,
    no_simplify: bool,
    auto_budget: usize,
) -> SimplifyDecision {
    if no_simplify {
        return SimplifyDecision::Skip;
    }
    if let Some(value) = request {
        if value <= 0.0 {
            // Nonsensical request: fall through to the automatic policy below.
        } else {
            let target = if value <= 1.0 {
                (original_tris as f32 * value).round() as usize
            } else {
                value as usize
            }
            .max(1);
            return if target < original_tris {
                SimplifyDecision::To(target)
            } else {
                SimplifyDecision::Skip
            };
        }
    }
    if original_tris > auto_budget {
        SimplifyDecision::To(auto_budget.max(1))
    } else {
        SimplifyDecision::Skip
    }
}

/// Simplifies `scene` so its total triangle count is at most `target_tris`, using vertex
/// clustering over the scene's bounding box.
///
/// If the scene already has `target_tris` triangles or fewer (or has no geometry), it is returned
/// unchanged. Otherwise a grid resolution is chosen by binary search so the welded result lands at
/// or below the budget, and every mesh is rebuilt against that grid. The returned scene keeps the
/// original's name; the original is never mutated (callers keep it for `info`).
///
/// The output is deterministic and free of degenerate (repeated-corner or zero-area) triangles.
/// Because each welded vertex is the average of its cell's members, the simplified mesh stays
/// within the original bounding box.
pub fn simplify_scene(scene: &Scene, target_tris: usize) -> Scene {
    let target = target_tris.max(1);
    if scene.triangle_count() <= target {
        return scene.clone();
    }
    let Some(bbox) = scene.bounding_box() else {
        return scene.clone();
    };

    let divisions = choose_divisions(scene, &bbox, target);
    let grid = Grid::new(&bbox, divisions);
    let meshes = scene
        .meshes
        .iter()
        .map(|m| simplify_mesh(m, &grid))
        .collect();
    Scene::new(scene.name.clone(), meshes)
}

/// A uniform axis-aligned clustering grid: `divisions` cells along each axis of the bounding box.
struct Grid {
    min: Vec3,
    /// Reciprocal cell size per axis (`divisions / extent`), or `0.0` on a degenerate (flat) axis
    /// so every vertex lands in cell `0` on that axis instead of dividing by zero.
    inv_cell: Vec3,
    divisions: i32,
}

impl Grid {
    /// Builds a grid with `divisions` (clamped to at least 1) cells per axis over `bbox`.
    fn new(bbox: &BoundingBox, divisions: i32) -> Self {
        let divisions = divisions.max(1);
        let extent = bbox.size();
        let axis_inv = |e: f32| {
            if e > f32::EPSILON {
                divisions as f32 / e
            } else {
                0.0
            }
        };
        Self {
            min: bbox.min,
            inv_cell: Vec3::new(axis_inv(extent.x), axis_inv(extent.y), axis_inv(extent.z)),
            divisions,
        }
    }

    /// The integer cell coordinates a point snaps to, clamped to `0..divisions` per axis.
    #[inline]
    fn cell(&self, p: Vec3) -> (i32, i32, i32) {
        let rel = (p - self.min) * self.inv_cell;
        let last = self.divisions - 1;
        let clamp = |v: f32| (v.floor() as i32).clamp(0, last);
        (clamp(rel.x), clamp(rel.y), clamp(rel.z))
    }
}

/// Assigns each of a mesh's vertices a cluster id (cells are numbered in first-seen scan order, so
/// the mapping is deterministic), returning the per-vertex ids and the number of clusters.
fn cluster_vertices(mesh: &Mesh, grid: &Grid) -> (Vec<u32>, usize) {
    let mut cells: HashMap<(i32, i32, i32), u32> = HashMap::new();
    let mut ids = Vec::with_capacity(mesh.vertices.len());
    for v in &mesh.vertices {
        let next = cells.len() as u32;
        let id = *cells.entry(grid.cell(v.position)).or_insert(next);
        ids.push(id);
    }
    (ids, cells.len())
}

/// Counts the triangles of `mesh` that survive welding under `ids` on a purely topological basis
/// (all three corners in distinct clusters). Used by the resolution search; the final build
/// additionally drops zero-area faces, so the built count is always `<=` this count.
fn surviving_tris_topological(mesh: &Mesh, ids: &[u32]) -> usize {
    mesh.indices
        .chunks_exact(3)
        .filter(|t| {
            let (a, b, c) = (ids[t[0] as usize], ids[t[1] as usize], ids[t[2] as usize]);
            a != b && b != c && a != c
        })
        .count()
}

/// Binary-searches the grid resolution for the largest `divisions` whose welded (topological)
/// triangle count is still at most `target`, preserving as much detail as the budget allows.
///
/// The surviving-triangle count is monotonically non-decreasing in `divisions` (a finer grid only
/// ever splits clusters, never merges them), which is what makes the search valid. At `divisions
/// == 1` the whole scene collapses into one cluster, so zero triangles survive — the budget is
/// always satisfiable.
fn choose_divisions(scene: &Scene, bbox: &BoundingBox, target: usize) -> i32 {
    let (mut lo, mut hi, mut best) = (1, MAX_DIVISIONS, 1);
    while lo <= hi {
        let mid = lo + (hi - lo) / 2;
        let grid = Grid::new(bbox, mid);
        let total: usize = scene
            .meshes
            .iter()
            .map(|m| {
                let (ids, _) = cluster_vertices(m, &grid);
                surviving_tris_topological(m, &ids)
            })
            .sum();
        if total <= target {
            best = mid;
            lo = mid + 1;
        } else {
            hi = mid - 1;
        }
    }
    best
}

/// Rebuilds one mesh against `grid`: welds its vertices to per-cell averages and keeps only the
/// non-degenerate, non-zero-area triangles.
fn simplify_mesh(mesh: &Mesh, grid: &Grid) -> Mesh {
    let (ids, cluster_count) = cluster_vertices(mesh, grid);

    // Accumulate each cluster's representative as the mean of its member positions/normals, so the
    // welded vertex stays inside the original geometry's bounds.
    let mut sum_pos = vec![Vec3::ZERO; cluster_count];
    let mut sum_normal = vec![Vec3::ZERO; cluster_count];
    let mut members = vec![0u32; cluster_count];
    for (i, v) in mesh.vertices.iter().enumerate() {
        let c = ids[i] as usize;
        sum_pos[c] += v.position;
        sum_normal[c] += v.normal;
        members[c] += 1;
    }

    let vertices: Vec<Vertex> = (0..cluster_count)
        .map(|c| {
            let count = members[c].max(1) as f32;
            Vertex::new(sum_pos[c] / count, sum_normal[c].normalize_or_zero())
        })
        .collect();

    // Relative area floor: a triangle is dropped when its doubled area is negligible next to the
    // mesh's overall scale, so the test is robust to very large or very small models.
    let scale_sq = (grid.min).length_squared().max(1.0);
    let area_eps = 1e-10 * scale_sq;

    let mut indices = Vec::with_capacity(mesh.indices.len());
    for t in mesh.indices.chunks_exact(3) {
        let (a, b, c) = (ids[t[0] as usize], ids[t[1] as usize], ids[t[2] as usize]);
        if a == b || b == c || a == c {
            continue; // two corners welded together: degenerate
        }
        let pa = vertices[a as usize].position;
        let pb = vertices[b as usize].position;
        let pc = vertices[c as usize].position;
        if (pb - pa).cross(pc - pa).length_squared() <= area_eps * area_eps {
            continue; // collinear representatives: zero-area face
        }
        indices.extend_from_slice(&[a, b, c]);
    }

    Mesh::new(vertices, indices)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rgfx_core::Vertex;

    /// Builds a dense triangulated grid mesh in the `z = 0` plane spanning `[0, 1]^2` with
    /// `n x n` quads (so `2 * n * n` triangles and `(n+1)^2` vertices). A predictable stress mesh
    /// for the clustering search.
    fn dense_grid(n: usize) -> Scene {
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
                let v = (j * stride + i) as u32;
                let right = v + 1;
                let down = v + stride as u32;
                let down_right = down + 1;
                indices.extend_from_slice(&[v, right, down, right, down_right, down]);
            }
        }
        Scene::new("grid", vec![Mesh::new(vertices, indices)])
    }

    fn no_zero_area_triangles(scene: &Scene) -> bool {
        scene.meshes.iter().all(|m| {
            m.indices.chunks_exact(3).all(|t| {
                let pa = m.vertices[t[0] as usize].position;
                let pb = m.vertices[t[1] as usize].position;
                let pc = m.vertices[t[2] as usize].position;
                (pb - pa).cross(pc - pa).length_squared() > 0.0
                    && t[0] != t[1]
                    && t[1] != t[2]
                    && t[0] != t[2]
            })
        })
    }

    #[test]
    fn simplify_hits_budget_and_drops_degenerate_triangles() {
        let scene = dense_grid(40); // 3200 triangles
        assert_eq!(scene.triangle_count(), 3200);
        let target = 800;
        let simplified = simplify_scene(&scene, target);

        assert!(
            simplified.triangle_count() <= target,
            "result {} must be within budget {target}",
            simplified.triangle_count()
        );
        assert!(
            simplified.triangle_count() > 0,
            "a nonzero budget must keep some triangles"
        );
        assert!(
            no_zero_area_triangles(&simplified),
            "no degenerate or zero-area triangles may survive"
        );
    }

    #[test]
    fn simplify_preserves_bounds_within_tolerance() {
        let scene = dense_grid(50);
        let original = scene.bounding_box().unwrap();
        let simplified = simplify_scene(&scene, 500);
        let result = simplified.bounding_box().unwrap();

        // Welded vertices are averages of members, so the result can only shrink inward; it must
        // stay inside the original box and still cover most of it (one grid cell of slack).
        let extent = original.size();
        let tol = extent * 0.1 + Vec3::splat(1e-4);
        assert!(
            (result.min.x >= original.min.x - 1e-4) && (result.min.x <= original.min.x + tol.x),
            "min x drifted: {:?} vs {:?}",
            result.min,
            original.min
        );
        assert!(
            (result.max.x <= original.max.x + 1e-4) && (result.max.x >= original.max.x - tol.x),
            "max x drifted: {:?} vs {:?}",
            result.max,
            original.max
        );
        assert!(result.min.y >= original.min.y - 1e-4 && result.max.y <= original.max.y + 1e-4);
    }

    #[test]
    fn simplify_is_deterministic() {
        let scene = dense_grid(48);
        let a = simplify_scene(&scene, 1000);
        let b = simplify_scene(&scene, 1000);
        assert_eq!(a.triangle_count(), b.triangle_count());
        assert_eq!(a.meshes.len(), b.meshes.len());
        for (ma, mb) in a.meshes.iter().zip(&b.meshes) {
            assert_eq!(ma.indices, mb.indices, "indices must be identical");
            assert_eq!(ma.vertices.len(), mb.vertices.len());
            for (va, vb) in ma.vertices.iter().zip(&mb.vertices) {
                assert_eq!(
                    va.position, vb.position,
                    "welded positions must be identical"
                );
            }
        }
    }

    #[test]
    fn below_budget_scene_is_returned_unchanged() {
        let scene = dense_grid(4); // 32 triangles
        let simplified = simplify_scene(&scene, 1000);
        assert_eq!(simplified.triangle_count(), scene.triangle_count());
    }

    #[test]
    fn larger_budget_keeps_at_least_as_many_triangles() {
        let scene = dense_grid(60);
        let small = simplify_scene(&scene, 500).triangle_count();
        let large = simplify_scene(&scene, 4000).triangle_count();
        assert!(small <= 500 && large <= 4000);
        assert!(
            large >= small,
            "a larger budget must not keep fewer triangles ({large} < {small})"
        );
    }

    #[test]
    fn half_million_triangle_mesh_does_not_panic() {
        // ~500k triangles (500 x 500 quads = 500_000 tris). Must simplify without panicking and
        // land within budget.
        let scene = dense_grid(500);
        assert!(scene.triangle_count() >= 500_000);
        let simplified = simplify_scene(&scene, 50_000);
        assert!(simplified.triangle_count() <= 50_000);
        assert!(simplified.triangle_count() > 0);
        assert!(no_zero_area_triangles(&simplified));
    }

    /// Manual timing check (not run by default): measures the per-frame rasterization speedup from
    /// simplifying a ~705k-triangle mesh down to the 150k auto-budget, which is the interactive
    /// win (load + simplify happen once, then many frames re-render). Run with:
    /// `cargo test -p rgfx-3d --release simplify_render_speedup -- --ignored --nocapture`.
    #[test]
    #[ignore = "timing benchmark; run manually with --release --ignored --nocapture"]
    fn simplify_render_speedup() {
        use rgfx_core::{Camera, Framebuffer, SceneRenderer};
        use std::time::Instant;

        use crate::{Rasterizer, ShadingMode};

        let scene = dense_grid(594); // ~705k triangles
        let simplified = simplify_scene(&scene, 150_000);

        let mut cam = Camera::perspective(1.0, 60_f32.to_radians());
        cam.position = Vec3::new(0.5, 0.5, 2.5);
        cam.target = Vec3::new(0.5, 0.5, 0.0);
        cam.near = 0.1;
        cam.far = 10.0;

        let render_all = |s: &Scene, dim: usize, frames: usize| -> f64 {
            let mut fb = Framebuffer::new(dim, dim);
            let mut ras = Rasterizer::new(ShadingMode::Flat);
            ras.cull = crate::Cull::None;
            let start = Instant::now();
            for _ in 0..frames {
                ras.render(s, &cam, &mut fb).unwrap();
            }
            start.elapsed().as_secs_f64() / frames as f64 * 1000.0
        };

        let full_ms = render_all(&scene, 240, 20);
        let simp_ms = render_all(&simplified, 240, 20);
        // Adaptive resolution during motion: simplified mesh at half the framebuffer dimensions
        // (a quarter of the pixels).
        let simp_half_ms = render_all(&simplified, 120, 20);
        println!(
            "per-frame render (240px): full ({} tris) {full_ms:.2} ms, simplified ({} tris) {simp_ms:.2} ms => {:.2}x",
            scene.triangle_count(),
            simplified.triangle_count(),
            full_ms / simp_ms
        );
        println!(
            "interacting (simplified + half-res 120px): {simp_half_ms:.2} ms => {:.2}x vs full",
            full_ms / simp_half_ms
        );
    }

    #[test]
    fn empty_scene_is_handled() {
        let empty = Scene::new("empty", vec![Mesh::default()]);
        let simplified = simplify_scene(&empty, 100);
        assert_eq!(simplified.triangle_count(), 0);
    }

    // --- decision logic -----------------------------------------------------------------------

    #[test]
    fn no_simplify_flag_always_skips() {
        assert_eq!(
            decide_simplify(1_000_000, Some(0.1), true, 150_000),
            SimplifyDecision::Skip
        );
        assert_eq!(
            decide_simplify(1_000_000, None, true, 150_000),
            SimplifyDecision::Skip
        );
    }

    #[test]
    fn explicit_ratio_targets_fraction_of_original() {
        assert_eq!(
            decide_simplify(1000, Some(0.25), false, 150_000),
            SimplifyDecision::To(250)
        );
        // A ratio of 1.0 (or more) would not reduce, so it is skipped.
        assert_eq!(
            decide_simplify(1000, Some(1.0), false, 150_000),
            SimplifyDecision::Skip
        );
    }

    #[test]
    fn explicit_absolute_target_above_one_is_a_count() {
        assert_eq!(
            decide_simplify(1_000_000, Some(40_000.0), false, 150_000),
            SimplifyDecision::To(40_000)
        );
        // A count at or above the original does not reduce.
        assert_eq!(
            decide_simplify(30_000, Some(40_000.0), false, 150_000),
            SimplifyDecision::Skip
        );
    }

    #[test]
    fn automatic_budget_applies_only_above_threshold() {
        assert_eq!(
            decide_simplify(705_000, None, false, 150_000),
            SimplifyDecision::To(150_000)
        );
        assert_eq!(
            decide_simplify(4_700, None, false, 150_000),
            SimplifyDecision::Skip
        );
        // A non-positive explicit request is ignored and falls back to the automatic policy.
        assert_eq!(
            decide_simplify(705_000, Some(0.0), false, 150_000),
            SimplifyDecision::To(150_000)
        );
    }
}
