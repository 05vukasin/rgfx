//! Built-in mesh primitives for demos and tests.
//!
//! These generators return plain [`rgfx_core::Mesh`] values (indexed triangle lists) so the CLI
//! can show interactive 3D — a spinning wireframe cube, say — before any file loader is wired up.
//! The meshes use shared corner vertices (no per-face duplication), so the triangle indices
//! describe the real edge topology: deduplicating triangle edges recovers the cube's 12 box edges
//! plus one diagonal per quad face.
//!
//! Corner vertices carry a zero normal ([`Vertex::from_position`]); the rasterizer synthesizes a
//! geometric face normal per triangle, so these meshes still shade correctly in the flat and unlit
//! modes as well as rendering as wireframes.

use glam::Vec3;
use rgfx_core::{Mesh, Vertex};

/// An axis-aligned cube centered at the origin with corners at `±half_extent` on each axis.
///
/// The mesh has 8 shared corner vertices and 12 triangles (two per face), wound
/// counter-clockwise when viewed from outside so back-face culling keeps the front faces. A
/// non-positive `half_extent` is clamped to a small positive value so the mesh is never
/// degenerate.
pub fn cube(half_extent: f32) -> Mesh {
    let h = half_extent.max(1e-4);
    // Corner layout (z = -h is the "back" face, z = +h the "front"):
    //   0:(-h,-h,-h) 1:(+h,-h,-h) 2:(+h,+h,-h) 3:(-h,+h,-h)
    //   4:(-h,-h,+h) 5:(+h,-h,+h) 6:(+h,+h,+h) 7:(-h,+h,+h)
    let corners = [
        Vec3::new(-h, -h, -h),
        Vec3::new(h, -h, -h),
        Vec3::new(h, h, -h),
        Vec3::new(-h, h, -h),
        Vec3::new(-h, -h, h),
        Vec3::new(h, -h, h),
        Vec3::new(h, h, h),
        Vec3::new(-h, h, h),
    ];
    let vertices = corners.map(Vertex::from_position).to_vec();

    // Two triangles per face, each quad wound CCW as seen from outside the cube.
    #[rustfmt::skip]
    let indices = vec![
        4, 5, 6, 4, 6, 7, // +Z front
        1, 0, 3, 1, 3, 2, // -Z back
        5, 1, 2, 5, 2, 6, // +X right
        0, 4, 7, 0, 7, 3, // -X left
        7, 6, 2, 7, 2, 3, // +Y top
        0, 1, 5, 0, 5, 4, // -Y bottom
    ];
    Mesh::new(vertices, indices)
}

/// A regular tetrahedron centered near the origin, sized so its vertices sit at radius
/// `circumradius` from the center.
///
/// The mesh has 4 shared vertices and 4 triangles, each wound counter-clockwise when viewed from
/// outside. A non-positive `circumradius` is clamped to a small positive value. Deduplicating the
/// triangle edges recovers all 6 edges of the tetrahedron.
pub fn tetrahedron(circumradius: f32) -> Mesh {
    let r = circumradius.max(1e-4);
    // The four vertices of a regular tetrahedron inscribed in a cube; scaled to unit circumradius
    // (each raw corner has length sqrt(3)) then out to `r`.
    let s = r / 3.0_f32.sqrt();
    let corners = [
        Vec3::new(1.0, 1.0, 1.0) * s,
        Vec3::new(1.0, -1.0, -1.0) * s,
        Vec3::new(-1.0, 1.0, -1.0) * s,
        Vec3::new(-1.0, -1.0, 1.0) * s,
    ];
    let vertices = corners.map(Vertex::from_position).to_vec();
    // Faces wound CCW as seen from outside (verified against the outward face normals).
    #[rustfmt::skip]
    let indices = vec![
        0, 1, 2,
        0, 3, 1,
        0, 2, 3,
        1, 3, 2,
    ];
    Mesh::new(vertices, indices)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_has_expected_topology() {
        let m = cube(1.0);
        assert_eq!(m.vertices.len(), 8, "cube uses 8 shared corner vertices");
        assert_eq!(m.triangle_count(), 12, "cube has two triangles per face");
    }

    #[test]
    fn cube_is_centered_and_sized() {
        let m = cube(2.0);
        let bb = m.bounding_box().unwrap();
        assert_eq!(bb.min, Vec3::splat(-2.0));
        assert_eq!(bb.max, Vec3::splat(2.0));
        assert_eq!(bb.center(), Vec3::ZERO);
    }

    #[test]
    fn cube_faces_wind_outward() {
        // Each triangle's geometric normal should point away from the cube center (positive dot
        // with the triangle centroid), confirming a consistent outward CCW winding.
        let m = cube(1.0);
        for tri in m.indices.chunks_exact(3) {
            let a = m.vertices[tri[0] as usize].position;
            let b = m.vertices[tri[1] as usize].position;
            let c = m.vertices[tri[2] as usize].position;
            let normal = (b - a).cross(c - a);
            let centroid = (a + b + c) / 3.0;
            assert!(
                normal.dot(centroid) > 0.0,
                "triangle {tri:?} should be wound counter-clockwise as seen from outside"
            );
        }
    }

    #[test]
    fn cube_clamps_nonpositive_extent() {
        let m = cube(0.0);
        let bb = m.bounding_box().unwrap();
        assert!(bb.size().x > 0.0 && bb.size().y > 0.0 && bb.size().z > 0.0);
    }

    #[test]
    fn tetrahedron_has_expected_topology() {
        let m = tetrahedron(1.0);
        assert_eq!(m.vertices.len(), 4);
        assert_eq!(m.triangle_count(), 4);
    }

    #[test]
    fn tetrahedron_vertices_sit_on_circumsphere() {
        let m = tetrahedron(3.0);
        for v in &m.vertices {
            assert!(
                (v.position.length() - 3.0).abs() < 1e-4,
                "vertex should sit at the requested circumradius"
            );
        }
    }

    #[test]
    fn tetrahedron_faces_wind_outward() {
        let m = tetrahedron(1.0);
        let center = m
            .vertices
            .iter()
            .fold(Vec3::ZERO, |acc, v| acc + v.position)
            / 4.0;
        for tri in m.indices.chunks_exact(3) {
            let a = m.vertices[tri[0] as usize].position;
            let b = m.vertices[tri[1] as usize].position;
            let c = m.vertices[tri[2] as usize].position;
            let normal = (b - a).cross(c - a);
            let centroid = (a + b + c) / 3.0;
            assert!(
                normal.dot(centroid - center) > 0.0,
                "triangle {tri:?} should face outward"
            );
        }
    }
}
