//! Fast, dependency-free mesh simplification via **vertex clustering**.
//!
//! Large meshes (hundreds of thousands of triangles) are expensive to rasterize every frame in the
//! CPU pipeline. [`simplify_scene`] decimates a [`Scene`] to a target triangle budget by snapping
//! vertices onto a regular grid sized from the scene's bounding box, welding all vertices that fall
//! in the same cell into one (their average position and normal), and dropping the triangles that
//! collapse to a degenerate edge or a zero-area sliver. This is O(n) in the vertex/triangle count,
//! needs no external crate, and is fully deterministic (cell membership and triangle order are
//! fixed by the input order, not by hash iteration).
//!
//! The grid resolution is chosen by a bounded binary search for the finest grid whose clustered
//! triangle count still fits the budget, so the result is always `<= target_tris` while keeping as
//! much detail as the budget allows.
//!
//! This is the robust first lever; quadric-error decimation is a nicer-but-heavier future upgrade.

use std::collections::HashMap;

use glam::Vec3;
use rgfx_core::{BoundingBox, Mesh, Scene, Vertex};

/// Triangles whose squared area is at or below this are treated as degenerate and dropped.
const MIN_AREA_SQ: f32 = 1e-12;

/// Simplifies `scene` to at most `target_tris` triangles using vertex clustering.
///
/// Returns a new [`Scene`] (the input is never mutated). If the scene is empty, has degenerate
/// (zero-size) bounds, or already has `<= target_tris` triangles, the scene is returned unchanged
/// (a clone). The returned scene always has `triangle_count() <= target_tris`.
///
/// The result is deterministic: the same input and target always produce the same mesh.
pub fn simplify_scene(scene: &Scene, target_tris: usize) -> Scene {
    let original = scene.triangle_count();
    if original == 0 || target_tris >= original {
        return scene.clone();
    }
    let Some(bounds) = scene.bounding_box() else {
        return scene.clone();
    };
    if bounds.size().max_element() <= 0.0 {
        // A point/line-degenerate box has no volume to grid; leave it untouched.
        return scene.clone();
    }

    // Cells along the longest axis. Occupied cells on a surface grow ~ res^2, and triangles ~ 2x
    // vertices, so `sqrt(target)` is a good scale; the ceiling keeps the search bounded.
    let hi_est = ((target_tris as f64).sqrt() * 3.0).ceil() as u32;
    let mut lo = 1u32;
    let mut hi = hi_est.clamp(4, 4096);

    // Binary search the largest resolution whose clustered triangle count stays within budget.
    // Clustering triangle count is monotonic-ish in resolution (a coarser grid welds more and drops
    // more degenerate triangles), and every accepted candidate is `<= target_tris` by construction,
    // so the result never exceeds the budget even if monotonicity wobbles.
    let mut best: Option<Scene> = None;
    while lo <= hi {
        let mid = lo + (hi - lo) / 2;
        let candidate = cluster_scene(scene, &bounds, mid);
        if candidate.triangle_count() <= target_tris {
            best = Some(candidate);
            lo = mid + 1;
        } else if mid == 0 {
            break;
        } else {
            hi = mid - 1;
        }
    }

    best.unwrap_or_else(|| cluster_scene(scene, &bounds, 1))
}

/// Clusters every mesh in `scene` on a shared grid derived from `bounds` and `res`.
fn cluster_scene(scene: &Scene, bounds: &BoundingBox, res: u32) -> Scene {
    let meshes = scene
        .meshes
        .iter()
        .map(|m| cluster_mesh(m, bounds, res))
        .collect();
    Scene::new(scene.name.clone(), meshes)
}

/// Clusters a single mesh: weld vertices sharing a grid cell, then rebuild the triangle list.
fn cluster_mesh(mesh: &Mesh, bounds: &BoundingBox, res: u32) -> Mesh {
    let res = res.max(1) as f32;
    let min = bounds.min;
    let extent = bounds.size();
    let longest = extent.max_element().max(f32::MIN_POSITIVE);

    // Per-axis cell counts proportional to the box extent, at least one, with the longest axis at
    // `res`. A zero-extent axis collapses to a single cell.
    let dims = [
        ((extent.x / longest) * res).round().max(1.0) as i64,
        ((extent.y / longest) * res).round().max(1.0) as i64,
        ((extent.z / longest) * res).round().max(1.0) as i64,
    ];
    let cell = Vec3::new(
        extent.x / dims[0] as f32,
        extent.y / dims[1] as f32,
        extent.z / dims[2] as f32,
    );

    let axis_index = |v: f32, minv: f32, c: f32, d: i64| -> i64 {
        if c <= 0.0 {
            0
        } else {
            (((v - minv) / c).floor() as i64).clamp(0, d - 1)
        }
    };
    let cell_key = |p: Vec3| -> (i64, i64, i64) {
        (
            axis_index(p.x, min.x, cell.x, dims[0]),
            axis_index(p.y, min.y, cell.y, dims[1]),
            axis_index(p.z, min.z, cell.z, dims[2]),
        )
    };

    // Weld vertices per cell, assigning new indices in first-seen order (deterministic).
    let mut map: HashMap<(i64, i64, i64), u32> = HashMap::new();
    let mut pos_sum: Vec<Vec3> = Vec::new();
    let mut nrm_sum: Vec<Vec3> = Vec::new();
    let mut counts: Vec<f32> = Vec::new();
    let mut remap: Vec<u32> = Vec::with_capacity(mesh.vertices.len());
    for v in &mesh.vertices {
        let key = cell_key(v.position);
        let idx = *map.entry(key).or_insert_with(|| {
            let i = pos_sum.len() as u32;
            pos_sum.push(Vec3::ZERO);
            nrm_sum.push(Vec3::ZERO);
            counts.push(0.0);
            i
        });
        let i = idx as usize;
        pos_sum[i] += v.position;
        nrm_sum[i] += v.normal;
        counts[i] += 1.0;
        remap.push(idx);
    }

    let vertices: Vec<Vertex> = pos_sum
        .iter()
        .zip(&nrm_sum)
        .zip(&counts)
        .map(|((p, n), &c)| Vertex::new(*p / c, n.normalize_or_zero()))
        .collect();

    // Rebuild triangles, dropping any that collapse (two welded vertices coincide) or become a
    // zero-area sliver. Out-of-range indices (malformed input) are skipped rather than panicking.
    let mut indices = Vec::new();
    for tri in mesh.indices.chunks_exact(3) {
        let (Some(&a), Some(&b), Some(&c)) = (
            remap.get(tri[0] as usize),
            remap.get(tri[1] as usize),
            remap.get(tri[2] as usize),
        ) else {
            continue;
        };
        if a == b || b == c || a == c {
            continue;
        }
        let pa = vertices[a as usize].position;
        let pb = vertices[b as usize].position;
        let pc = vertices[c as usize].position;
        if (pb - pa).cross(pc - pa).length_squared() <= MIN_AREA_SQ {
            continue;
        }
        indices.extend_from_slice(&[a, b, c]);
    }

    Mesh::new(vertices, indices)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dense `n`×`n`-quad grid on the `z = 0` plane spanning the unit square, triangulated into
    /// `2*n*n` triangles with `(n+1)^2` shared vertices. A deterministic, high-poly stress mesh.
    fn grid_mesh(n: usize) -> Mesh {
        let side = n + 1;
        let mut vertices = Vec::with_capacity(side * side);
        for j in 0..side {
            for i in 0..side {
                let x = i as f32 / n as f32;
                let y = j as f32 / n as f32;
                vertices.push(Vertex::new(Vec3::new(x, y, 0.0), Vec3::Z));
            }
        }
        let mut indices = Vec::with_capacity(n * n * 6);
        for j in 0..n {
            for i in 0..n {
                let v00 = (j * side + i) as u32;
                let v10 = v00 + 1;
                let v01 = v00 + side as u32;
                let v11 = v01 + 1;
                indices.extend_from_slice(&[v00, v10, v11, v00, v11, v01]);
            }
        }
        Mesh::new(vertices, indices)
    }

    fn grid_scene(n: usize) -> Scene {
        Scene::new("grid", vec![grid_mesh(n)])
    }

    fn no_zero_area_triangles(scene: &Scene) -> bool {
        for mesh in &scene.meshes {
            for tri in mesh.indices.chunks_exact(3) {
                if tri[0] == tri[1] || tri[1] == tri[2] || tri[0] == tri[2] {
                    return false;
                }
                let a = mesh.vertices[tri[0] as usize].position;
                let b = mesh.vertices[tri[1] as usize].position;
                let c = mesh.vertices[tri[2] as usize].position;
                if (b - a).cross(c - a).length_squared() <= MIN_AREA_SQ {
                    return false;
                }
            }
        }
        true
    }

    #[test]
    fn decimates_dense_grid_to_target_budget() {
        let scene = grid_scene(100); // 20_000 triangles
        assert_eq!(scene.triangle_count(), 20_000);
        let target = 2_000;
        let simplified = simplify_scene(&scene, target);

        assert!(
            simplified.triangle_count() <= target,
            "must not exceed the target budget, got {}",
            simplified.triangle_count()
        );
        assert!(
            simplified.triangle_count() > 0,
            "must keep some geometry, not collapse to nothing"
        );
        assert!(
            no_zero_area_triangles(&simplified),
            "no degenerate or zero-area triangles may survive"
        );
    }

    #[test]
    fn preserves_bounding_box_within_tolerance() {
        let scene = grid_scene(80);
        let original = scene.bounding_box().unwrap();
        let simplified = simplify_scene(&scene, 1_500);
        let after = simplified.bounding_box().unwrap();

        let extent = original.size();
        // Averaging welded vertices keeps the hull inside the original box; the extreme cells still
        // pull their averages near the corners, so the bounds stay close on the non-degenerate axes.
        let tol = 0.12;
        assert!((after.min.x - original.min.x).abs() <= extent.x * tol);
        assert!((after.min.y - original.min.y).abs() <= extent.y * tol);
        assert!((after.max.x - original.max.x).abs() <= extent.x * tol);
        assert!((after.max.y - original.max.y).abs() <= extent.y * tol);
    }

    #[test]
    fn is_deterministic() {
        let scene = grid_scene(64);
        let a = simplify_scene(&scene, 1_000);
        let b = simplify_scene(&scene, 1_000);
        assert_eq!(a.meshes.len(), b.meshes.len());
        for (ma, mb) in a.meshes.iter().zip(&b.meshes) {
            assert_eq!(ma.indices, mb.indices, "index lists must match exactly");
            assert_eq!(
                ma.vertices.len(),
                mb.vertices.len(),
                "vertex counts must match"
            );
            for (va, vb) in ma.vertices.iter().zip(&mb.vertices) {
                assert_eq!(va.position, vb.position, "welded positions must match");
            }
        }
    }

    #[test]
    fn target_at_or_above_original_returns_unchanged() {
        let scene = grid_scene(10); // 200 triangles
        let same = simplify_scene(&scene, 10_000);
        assert_eq!(same.triangle_count(), scene.triangle_count());
        let equal = simplify_scene(&scene, scene.triangle_count());
        assert_eq!(equal.triangle_count(), scene.triangle_count());
    }

    #[test]
    fn empty_and_degenerate_scenes_are_safe() {
        // Empty scene: nothing to do.
        let empty = Scene::new("empty", vec![Mesh::default()]);
        let out = simplify_scene(&empty, 10);
        assert_eq!(out.triangle_count(), 0);

        // All vertices coincident (zero-size bounds): returned unchanged, no panic, no divide by 0.
        let degenerate = Scene::new(
            "point",
            vec![Mesh::new(
                vec![Vertex::from_position(Vec3::ZERO); 3],
                vec![0, 1, 2],
            )],
        );
        let out = simplify_scene(&degenerate, 1);
        assert!(out.triangle_count() <= degenerate.triangle_count());
    }

    #[test]
    #[ignore = "timing benchmark; run with --ignored --nocapture"]
    fn timing_full_vs_simplified() {
        use std::time::Instant;

        use crate::{Rasterizer, ShadingMode};
        use rgfx_core::{Camera, Framebuffer, SceneRenderer};

        // ~500k-triangle scene, a stand-in for the reported heavy OBJ.
        let t0 = Instant::now();
        let scene = grid_scene(500);
        let load = t0.elapsed();

        let mut cam = Camera::perspective(1.0, 60_f32.to_radians());
        cam.position = glam::Vec3::new(0.5, 0.5, 2.0);
        cam.target = glam::Vec3::new(0.5, 0.5, 0.0);
        let mut fb = Framebuffer::new(320, 320);

        let mut render_once = |scene: &Scene| {
            let mut ras = Rasterizer::new(ShadingMode::Flat);
            let t = Instant::now();
            ras.render(scene, &cam, &mut fb).unwrap();
            t.elapsed()
        };

        let full_frame = render_once(&scene);

        let ts = Instant::now();
        let simplified = simplify_scene(&scene, 150_000);
        let simplify_time = ts.elapsed();
        let simp_frame = render_once(&simplified);

        println!(
            "load {:?} for {} tris | full frame {:?} | simplify {:?} -> {} tris | simplified frame {:?} | speedup {:.2}x",
            load,
            scene.triangle_count(),
            full_frame,
            simplify_time,
            simplified.triangle_count(),
            simp_frame,
            full_frame.as_secs_f64() / simp_frame.as_secs_f64().max(1e-9),
        );
    }

    #[test]
    fn handles_half_million_triangle_mesh_without_panicking() {
        // (500+1)^2 vertices, 2*500*500 = 500_000 triangles.
        let scene = grid_scene(500);
        assert_eq!(scene.triangle_count(), 500_000);
        let target = 150_000;
        let simplified = simplify_scene(&scene, target);
        assert!(simplified.triangle_count() <= target);
        assert!(simplified.triangle_count() > 0);
        assert!(no_zero_area_triangles(&simplified));
    }
}
