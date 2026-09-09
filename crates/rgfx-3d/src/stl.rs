//! STL mesh loading (binary and ASCII) built on the [`stl_io`] crate.
//!
//! STL files describe a soup of independent triangles, each carrying only a *per-face* normal
//! and no vertex sharing or attributes. This module turns such a file into an
//! [`rgfx_core::Scene`] containing a single [`rgfx_core::Mesh`], handling both the binary and
//! ASCII encodings transparently (auto-detected by `stl_io`).
//!
//! Two shading-oriented conversions are offered:
//!
//! * **Flat / per-face (default).** Every triangle contributes three fresh vertices, all carrying
//!   the triangle's face normal. This preserves the exact per-face normals STL is built around
//!   and is what flat shading wants. See [`load_stl`].
//! * **Welded / smooth.** Coincident vertices (within a distance tolerance) are merged into one,
//!   the index buffer is rebuilt to reference the shared vertices, and each vertex normal becomes
//!   the normalized sum of its incident face normals — producing smooth per-vertex normals. See
//!   [`StlOptions::weld_tolerance`] and [`load_stl_with_options`].
//!
//! The 3D-printing inspection use case (`rgfx part.stl`) drives this: load, auto-frame from the
//! bounding sphere, and report [`StlStats`] for an `info` view.
//!
//! ```no_run
//! use rgfx_3d::stl::{load_stl, StlStats};
//!
//! let scene = load_stl("part.stl")?;
//! let stats = StlStats::from_scene(&scene);
//! # Ok::<(), rgfx_core::Error>(())
//! ```

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek};
use std::path::Path;

use glam::Vec3;
use rgfx_core::{BoundingBox, BoundingSphere, Error, Mesh, Result, Scene, Vertex};

/// Options controlling how an STL file is converted into a [`Scene`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StlOptions {
    /// When `Some(tolerance)`, vertices closer together than `tolerance` (in model units) are
    /// welded into a single shared vertex and their incident face normals are averaged to give
    /// smooth per-vertex normals; the index buffer is rebuilt accordingly. `tolerance` must be a
    /// finite, strictly-positive number.
    ///
    /// When `None` (the default), no welding happens: each triangle keeps its own three vertices,
    /// each carrying the per-face normal (flat shading).
    ///
    /// Welding quantizes positions onto a grid of cell size `tolerance`, so exactly-coincident
    /// vertices (the common case for shared cube/mesh corners) always merge. Points that are
    /// within `tolerance` but straddle a grid boundary may not merge; pick a tolerance well below
    /// the smallest feature size.
    pub weld_tolerance: Option<f32>,
}

/// Summary statistics for a loaded STL scene, suitable for an `info` display.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StlStats {
    /// Total number of triangles across the scene.
    pub triangle_count: usize,
    /// Total number of (post-weld) vertices across the scene.
    pub vertex_count: usize,
    /// The axis-aligned bounding box, or `None` if the scene has no vertices.
    pub bounding_box: Option<BoundingBox>,
    /// The bounding sphere used for auto-framing, or `None` if the scene has no vertices.
    pub bounding_sphere: Option<BoundingSphere>,
}

impl StlStats {
    /// Computes statistics from a scene.
    pub fn from_scene(scene: &Scene) -> Self {
        let bounding_box = scene.bounding_box();
        Self {
            triangle_count: scene.triangle_count(),
            vertex_count: scene.vertex_count(),
            bounding_box,
            bounding_sphere: bounding_box.map(|b| b.bounding_sphere()),
        }
    }
}

/// Loads an STL file (binary or ASCII) into a [`Scene`] using the default [`StlOptions`].
///
/// Per-face normals are preserved (flat shading); no vertex welding is performed. The scene is
/// named after the file stem. Malformed input yields [`Error::Decode`]; I/O failures yield
/// [`Error::Io`]. This function never panics on file contents.
pub fn load_stl(path: impl AsRef<Path>) -> Result<Scene> {
    load_stl_with_options(path, &StlOptions::default())
}

/// Loads an STL file (binary or ASCII) into a [`Scene`] with explicit [`StlOptions`].
///
/// See [`StlOptions::weld_tolerance`] for the welding behavior. Malformed input yields
/// [`Error::Decode`]; an invalid weld tolerance yields [`Error::Geometry`]; I/O failures yield
/// [`Error::Io`]. This function never panics on file contents.
pub fn load_stl_with_options(path: impl AsRef<Path>, options: &StlOptions) -> Result<Scene> {
    let path = path.as_ref();
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut file = File::open(path)?;
    scene_from_reader(&mut file, name, options)
}

/// Reads an STL from any `Read + Seek` source into a named [`Scene`].
///
/// This is the shared core of [`load_stl`]/[`load_stl_with_options`] and lets tests feed embedded
/// byte/text fixtures via [`std::io::Cursor`].
fn scene_from_reader<R: Read + Seek>(
    reader: &mut R,
    name: impl Into<String>,
    options: &StlOptions,
) -> Result<Scene> {
    let mesh =
        stl_io::read_stl(reader).map_err(|e| Error::Decode(format!("failed to parse STL: {e}")))?;
    let mesh = match options.weld_tolerance {
        Some(tolerance) => weld_mesh(&mesh, tolerance)?,
        None => flat_mesh(&mesh),
    };
    Ok(Scene::new(name, vec![mesh]))
}

/// Builds a flat-shaded mesh: three fresh vertices per triangle, each carrying the face normal.
fn flat_mesh(mesh: &stl_io::IndexedMesh) -> Mesh {
    let count = mesh.faces.len() * 3;
    let mut vertices = Vec::with_capacity(count);
    let mut indices = Vec::with_capacity(count);
    for face in &mesh.faces {
        let positions = face_positions(mesh, face);
        let normal = face_normal(Vec3::from_array(face.normal.0), &positions);
        for position in positions {
            indices.push(vertices.len() as u32);
            vertices.push(Vertex::new(position, normal));
        }
    }
    Mesh::new(vertices, indices)
}

/// Builds a smooth-shaded mesh by welding coincident vertices within `tolerance` and averaging
/// incident face normals. Returns [`Error::Geometry`] if `tolerance` is not finite and positive.
fn weld_mesh(mesh: &stl_io::IndexedMesh, tolerance: f32) -> Result<Mesh> {
    if !(tolerance.is_finite() && tolerance > 0.0) {
        return Err(Error::Geometry(format!(
            "weld tolerance must be finite and positive, got {tolerance}"
        )));
    }
    let inv = 1.0 / tolerance;
    let mut lookup: HashMap<[i64; 3], u32> = HashMap::new();
    let mut positions: Vec<Vec3> = Vec::new();
    let mut normal_sums: Vec<Vec3> = Vec::new();
    let mut indices: Vec<u32> = Vec::with_capacity(mesh.faces.len() * 3);

    for face in &mesh.faces {
        let tri = face_positions(mesh, face);
        let normal = face_normal(Vec3::from_array(face.normal.0), &tri);
        for position in tri {
            let key = [
                (position.x * inv).round() as i64,
                (position.y * inv).round() as i64,
                (position.z * inv).round() as i64,
            ];
            let index = *lookup.entry(key).or_insert_with(|| {
                let index = positions.len() as u32;
                positions.push(position);
                normal_sums.push(Vec3::ZERO);
                index
            });
            normal_sums[index as usize] += normal;
            indices.push(index);
        }
    }

    let vertices = positions
        .iter()
        .zip(&normal_sums)
        .map(|(&position, &normal_sum)| Vertex::new(position, safe_normalize(normal_sum)))
        .collect();
    Ok(Mesh::new(vertices, indices))
}

/// The three triangle-corner positions of an indexed face as glam vectors.
fn face_positions(mesh: &stl_io::IndexedMesh, face: &stl_io::IndexedTriangle) -> [Vec3; 3] {
    [
        Vec3::from_array(mesh.vertices[face.vertices[0]].0),
        Vec3::from_array(mesh.vertices[face.vertices[1]].0),
        Vec3::from_array(mesh.vertices[face.vertices[2]].0),
    ]
}

/// Resolves a usable unit face normal.
///
/// STL files store a per-face normal, but it is frequently zero or garbage. We trust the stored
/// normal when it is finite and non-degenerate; otherwise we derive the geometric normal from the
/// triangle's winding, falling back to `+Z` for a fully degenerate (zero-area) triangle.
fn face_normal(stored: Vec3, positions: &[Vec3; 3]) -> Vec3 {
    if stored.is_finite() && stored.length_squared() > 1e-12 {
        return stored.normalize();
    }
    let geometric = (positions[1] - positions[0]).cross(positions[2] - positions[0]);
    safe_normalize(geometric)
}

/// Normalizes a vector, returning `+Z` for a (near-)zero vector so the result is always finite.
fn safe_normalize(v: Vec3) -> Vec3 {
    if v.is_finite() && v.length_squared() > 1e-20 {
        v.normalize()
    } else {
        Vec3::Z
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    // Unit cube [0,1]^3 as 12 triangles: (face normal, [v0, v1, v2]), wound CCW when viewed from
    // outside. Corners:
    //   0:(0,0,0) 1:(1,0,0) 2:(1,1,0) 3:(0,1,0)  (z=0)
    //   4:(0,0,1) 5:(1,0,1) 6:(1,1,1) 7:(0,1,1)  (z=1)
    const CUBE: [([f32; 3], [[f32; 3]; 3]); 12] = [
        // bottom (z=0), -Z
        (
            [0.0, 0.0, -1.0],
            [[0.0, 0.0, 0.0], [1.0, 1.0, 0.0], [1.0, 0.0, 0.0]],
        ),
        (
            [0.0, 0.0, -1.0],
            [[0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0]],
        ),
        // top (z=1), +Z
        (
            [0.0, 0.0, 1.0],
            [[0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [1.0, 1.0, 1.0]],
        ),
        (
            [0.0, 0.0, 1.0],
            [[0.0, 0.0, 1.0], [1.0, 1.0, 1.0], [0.0, 1.0, 1.0]],
        ),
        // front (y=0), -Y
        (
            [0.0, -1.0, 0.0],
            [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 1.0]],
        ),
        (
            [0.0, -1.0, 0.0],
            [[0.0, 0.0, 0.0], [1.0, 0.0, 1.0], [0.0, 0.0, 1.0]],
        ),
        // back (y=1), +Y
        (
            [0.0, 1.0, 0.0],
            [[0.0, 1.0, 0.0], [0.0, 1.0, 1.0], [1.0, 1.0, 1.0]],
        ),
        (
            [0.0, 1.0, 0.0],
            [[0.0, 1.0, 0.0], [1.0, 1.0, 1.0], [1.0, 1.0, 0.0]],
        ),
        // left (x=0), -X
        (
            [-1.0, 0.0, 0.0],
            [[0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 1.0]],
        ),
        (
            [-1.0, 0.0, 0.0],
            [[0.0, 0.0, 0.0], [0.0, 1.0, 1.0], [0.0, 1.0, 0.0]],
        ),
        // right (x=1), +X
        (
            [1.0, 0.0, 0.0],
            [[1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [1.0, 1.0, 1.0]],
        ),
        (
            [1.0, 0.0, 0.0],
            [[1.0, 0.0, 0.0], [1.0, 1.0, 1.0], [1.0, 0.0, 1.0]],
        ),
    ];

    fn ascii_cube() -> String {
        let mut s = String::from("solid cube\n");
        for (n, vs) in CUBE {
            s.push_str(&format!("facet normal {} {} {}\n", n[0], n[1], n[2]));
            s.push_str("outer loop\n");
            for v in vs {
                s.push_str(&format!("vertex {} {} {}\n", v[0], v[1], v[2]));
            }
            s.push_str("endloop\n");
            s.push_str("endfacet\n");
        }
        s.push_str("endsolid cube\n");
        s
    }

    fn binary_cube() -> Vec<u8> {
        let mut b = vec![0u8; 80]; // 80-byte header (non-"solid " so it parses as binary)
        b.extend_from_slice(&(CUBE.len() as u32).to_le_bytes());
        for (n, vs) in CUBE {
            for c in n {
                b.extend_from_slice(&c.to_le_bytes());
            }
            for v in vs {
                for c in v {
                    b.extend_from_slice(&c.to_le_bytes());
                }
            }
            b.extend_from_slice(&0u16.to_le_bytes()); // attribute byte count
        }
        b
    }

    fn assert_unit_cube_bounds(scene: &Scene) {
        let bb = scene.bounding_box().expect("cube has a bounding box");
        assert!((bb.min - Vec3::ZERO).length() < 1e-6, "min = {:?}", bb.min);
        assert!((bb.max - Vec3::ONE).length() < 1e-6, "max = {:?}", bb.max);
    }

    #[test]
    fn ascii_cube_loads_with_correct_counts_and_bounds() {
        let text = ascii_cube();
        let mut cursor = Cursor::new(text.into_bytes());
        let scene = scene_from_reader(&mut cursor, "cube", &StlOptions::default()).unwrap();
        assert_eq!(scene.triangle_count(), 12);
        assert_eq!(scene.vertex_count(), 36); // flat: 3 unshared verts per triangle
        assert_unit_cube_bounds(&scene);
    }

    #[test]
    fn binary_cube_loads_with_correct_counts_and_bounds() {
        let mut cursor = Cursor::new(binary_cube());
        let scene = scene_from_reader(&mut cursor, "cube", &StlOptions::default()).unwrap();
        assert_eq!(scene.triangle_count(), 12);
        assert_eq!(scene.vertex_count(), 36);
        assert_unit_cube_bounds(&scene);
    }

    #[test]
    fn ascii_and_binary_agree_on_geometry() {
        let mut a = Cursor::new(ascii_cube().into_bytes());
        let mut b = Cursor::new(binary_cube());
        let sa = scene_from_reader(&mut a, "cube", &StlOptions::default()).unwrap();
        let sb = scene_from_reader(&mut b, "cube", &StlOptions::default()).unwrap();
        assert_eq!(sa.triangle_count(), sb.triangle_count());
        assert_eq!(sa.vertex_count(), sb.vertex_count());
        assert_eq!(sa.bounding_box(), sb.bounding_box());
    }

    #[test]
    fn flat_vertices_carry_the_per_face_normal() {
        let mut cursor = Cursor::new(binary_cube());
        let scene = scene_from_reader(&mut cursor, "cube", &StlOptions::default()).unwrap();
        let mesh = &scene.meshes[0];
        // First two triangles are the -Z bottom face; all six vertices must have normal -Z.
        for v in &mesh.vertices[0..6] {
            assert!((v.normal - Vec3::new(0.0, 0.0, -1.0)).length() < 1e-6);
        }
    }

    #[test]
    fn welding_merges_coincident_vertices() {
        let options = StlOptions {
            weld_tolerance: Some(1e-4),
        };
        let mut cursor = Cursor::new(binary_cube());
        let scene = scene_from_reader(&mut cursor, "cube", &options).unwrap();
        let mesh = &scene.meshes[0];
        // A cube has 8 distinct corners; welding must collapse the 36 flat verts to 8.
        assert_eq!(mesh.vertices.len(), 8);
        assert_eq!(scene.triangle_count(), 12); // faces unchanged
        assert_eq!(mesh.indices.len(), 36); // index buffer rebuilt, still 12 tris
        assert!(mesh.indices.iter().all(|&i| (i as usize) < 8));
        // Every welded normal is unit length (smooth normals).
        for v in &mesh.vertices {
            assert!(
                (v.normal.length() - 1.0).abs() < 1e-5,
                "normal = {:?}",
                v.normal
            );
        }
        assert_unit_cube_bounds(&scene);
    }

    #[test]
    fn welded_corner_normal_points_diagonally_outward() {
        let options = StlOptions {
            weld_tolerance: Some(1e-4),
        };
        let mut cursor = Cursor::new(binary_cube());
        let scene = scene_from_reader(&mut cursor, "cube", &options).unwrap();
        let mesh = &scene.meshes[0];
        // The corner at (1,1,1) should average the +X, +Y, +Z faces -> diagonal outward.
        let corner = mesh
            .vertices
            .iter()
            .find(|v| (v.position - Vec3::ONE).length() < 1e-6)
            .expect("cube has a (1,1,1) corner");
        let expected = Vec3::splat(1.0).normalize();
        assert!(
            (corner.normal - expected).length() < 1e-5,
            "normal = {:?}",
            corner.normal
        );
    }

    #[test]
    fn stats_summarize_scene() {
        let mut cursor = Cursor::new(binary_cube());
        let scene = scene_from_reader(&mut cursor, "cube", &StlOptions::default()).unwrap();
        let stats = StlStats::from_scene(&scene);
        assert_eq!(stats.triangle_count, 12);
        assert_eq!(stats.vertex_count, 36);
        assert!(stats.bounding_box.is_some());
        let sphere = stats.bounding_sphere.expect("has sphere");
        assert!((sphere.center - Vec3::splat(0.5)).length() < 1e-6);
        assert!((sphere.radius - 3.0_f32.sqrt() * 0.5).abs() < 1e-5);
    }

    #[test]
    fn malformed_stl_is_an_error_not_a_panic() {
        // Starts with "solid " so it parses as ASCII, but the facet header is truncated.
        let bad = b"solid broken\nfacet normal 1 2\nendsolid broken\n".to_vec();
        let mut cursor = Cursor::new(bad);
        let result = scene_from_reader(&mut cursor, "broken", &StlOptions::default());
        assert!(matches!(result, Err(Error::Decode(_))), "got {result:?}");
    }

    #[test]
    fn truncated_binary_stl_is_an_error() {
        // Valid-looking header claiming triangles, but no triangle data follows.
        let mut bad = vec![0u8; 80];
        bad.extend_from_slice(&5u32.to_le_bytes());
        let mut cursor = Cursor::new(bad);
        let result = scene_from_reader(&mut cursor, "broken", &StlOptions::default());
        assert!(matches!(result, Err(Error::Decode(_))), "got {result:?}");
    }

    #[test]
    fn invalid_weld_tolerance_is_a_geometry_error() {
        let options = StlOptions {
            weld_tolerance: Some(0.0),
        };
        let mut cursor = Cursor::new(binary_cube());
        let result = scene_from_reader(&mut cursor, "cube", &options);
        assert!(matches!(result, Err(Error::Geometry(_))), "got {result:?}");
    }
}
