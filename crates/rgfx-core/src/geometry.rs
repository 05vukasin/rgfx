//! Mesh and scene geometry shared by the loaders and the 3D renderer.

use glam::Vec3;

/// A single mesh vertex: position plus an optional normal.
///
/// Loaders that lack normals leave [`Vertex::normal`] as `Vec3::ZERO`; the renderer or loader is
/// then expected to generate face/vertex normals before shading.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    /// Position in model space.
    pub position: Vec3,
    /// Normal in model space, or `Vec3::ZERO` if unknown.
    pub normal: Vec3,
}

impl Vertex {
    /// Creates a vertex with a position and a normal.
    pub const fn new(position: Vec3, normal: Vec3) -> Self {
        Self { position, normal }
    }

    /// Creates a vertex with a position and a zero (unknown) normal.
    pub const fn from_position(position: Vec3) -> Self {
        Self {
            position,
            normal: Vec3::ZERO,
        }
    }
}

/// An indexed triangle mesh.
///
/// `indices` is a flat list whose length is a multiple of three; each triple indexes into
/// `vertices` to form one triangle.
#[derive(Clone, Debug, Default)]
pub struct Mesh {
    /// The mesh vertices.
    pub vertices: Vec<Vertex>,
    /// Triangle indices into `vertices` (length divisible by 3).
    pub indices: Vec<u32>,
}

impl Mesh {
    /// Creates a mesh from vertices and triangle indices.
    pub fn new(vertices: Vec<Vertex>, indices: Vec<u32>) -> Self {
        Self { vertices, indices }
    }

    /// The number of triangles (`indices.len() / 3`).
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// The axis-aligned bounding box of the mesh's vertices, or `None` if it has no vertices.
    pub fn bounding_box(&self) -> Option<BoundingBox> {
        BoundingBox::from_points(self.vertices.iter().map(|v| v.position))
    }
}

/// A scene: a collection of meshes plus a human-readable name.
#[derive(Clone, Debug, Default)]
pub struct Scene {
    /// An optional source name (e.g. the file stem).
    pub name: String,
    /// The meshes in the scene, with transforms already baked into vertex positions.
    pub meshes: Vec<Mesh>,
}

impl Scene {
    /// Creates a scene from a name and its meshes.
    pub fn new(name: impl Into<String>, meshes: Vec<Mesh>) -> Self {
        Self {
            name: name.into(),
            meshes,
        }
    }

    /// Total vertex count across all meshes.
    pub fn vertex_count(&self) -> usize {
        self.meshes.iter().map(|m| m.vertices.len()).sum()
    }

    /// Total triangle count across all meshes.
    pub fn triangle_count(&self) -> usize {
        self.meshes.iter().map(|m| m.triangle_count()).sum()
    }

    /// The axis-aligned bounding box enclosing every mesh, or `None` if the scene is empty.
    pub fn bounding_box(&self) -> Option<BoundingBox> {
        BoundingBox::from_points(
            self.meshes
                .iter()
                .flat_map(|m| m.vertices.iter().map(|v| v.position)),
        )
    }
}

/// An axis-aligned bounding box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoundingBox {
    /// The minimum corner.
    pub min: Vec3,
    /// The maximum corner.
    pub max: Vec3,
}

impl BoundingBox {
    /// Builds the tightest box enclosing `points`, or `None` if the iterator is empty.
    pub fn from_points(points: impl IntoIterator<Item = Vec3>) -> Option<Self> {
        let mut iter = points.into_iter();
        let first = iter.next()?;
        let (mut min, mut max) = (first, first);
        for p in iter {
            min = min.min(p);
            max = max.max(p);
        }
        Some(Self { min, max })
    }

    /// The center point of the box.
    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    /// The full extent (size) of the box along each axis.
    pub fn size(&self) -> Vec3 {
        self.max - self.min
    }

    /// The bounding sphere that encloses this box (centered at the box center).
    pub fn bounding_sphere(&self) -> BoundingSphere {
        BoundingSphere {
            center: self.center(),
            radius: self.size().length() * 0.5,
        }
    }
}

/// A bounding sphere, used for automatic camera framing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoundingSphere {
    /// The sphere center.
    pub center: Vec3,
    /// The sphere radius (always non-negative).
    pub radius: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tri(a: Vec3, b: Vec3, c: Vec3) -> Mesh {
        Mesh::new(
            vec![
                Vertex::from_position(a),
                Vertex::from_position(b),
                Vertex::from_position(c),
            ],
            vec![0, 1, 2],
        )
    }

    #[test]
    fn triangle_and_vertex_counts() {
        let scene = Scene::new(
            "t",
            vec![
                tri(Vec3::ZERO, Vec3::X, Vec3::Y),
                tri(Vec3::ZERO, Vec3::Y, Vec3::Z),
            ],
        );
        assert_eq!(scene.triangle_count(), 2);
        assert_eq!(scene.vertex_count(), 6);
    }

    #[test]
    fn bounding_box_encloses_points() {
        let bb = BoundingBox::from_points([Vec3::new(-1.0, 0.0, 2.0), Vec3::new(3.0, -4.0, 0.0)])
            .unwrap();
        assert_eq!(bb.min, Vec3::new(-1.0, -4.0, 0.0));
        assert_eq!(bb.max, Vec3::new(3.0, 0.0, 2.0));
        assert_eq!(bb.center(), Vec3::new(1.0, -2.0, 1.0));
    }

    #[test]
    fn empty_points_have_no_box() {
        assert!(BoundingBox::from_points(std::iter::empty()).is_none());
        assert!(Scene::default().bounding_box().is_none());
    }

    #[test]
    fn unit_cube_sphere_radius() {
        let bb = BoundingBox {
            min: Vec3::splat(-1.0),
            max: Vec3::splat(1.0),
        };
        let s = bb.bounding_sphere();
        assert_eq!(s.center, Vec3::ZERO);
        // size = (2,2,2), length = 2*sqrt(3), radius = sqrt(3)
        assert!((s.radius - 3.0_f32.sqrt()).abs() < 1e-5);
    }
}
