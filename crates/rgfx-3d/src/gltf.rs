//! glTF 2.0 / GLB loader.
//!
//! [`load_gltf`] reads a glTF 2.0 asset — either a JSON `.gltf` file (with its external or
//! data-URI buffers) or a binary `.glb` container — into an [`rgfx_core::Scene`] whose meshes
//! carry world-space vertex positions. This task covers **static meshes with the node transform
//! hierarchy baked in**: each node's local transform is composed with its parents' transforms and
//! applied to the vertices it references, so a multi-node scene assembles in the right place
//! without the renderer needing per-mesh model matrices.
//!
//! Materials are read only far enough to pull each primitive's base color *factor* (no texture
//! sampling — that is a later task); the factors and the asset's material/animation counts are
//! reported through [`GltfStats`] via [`load_gltf_with_stats`] so the CLI `info` view can surface
//! them. Only triangle-mode primitives are turned into geometry; other primitive modes (points,
//! lines, strips/fans) are outside this task's scope and are skipped.
//!
//! Malformed input — a bad container, a missing buffer, a primitive without positions, or an
//! out-of-range index — produces an [`rgfx_core::Error`] rather than a panic.
//!
//! # Example
//!
//! ```no_run
//! let scene = rgfx_3d::load_gltf("model.glb")?;
//! println!("{} triangles", scene.triangle_count());
//! # Ok::<(), rgfx_core::Error>(())
//! ```

use std::path::Path;

use glam::{Mat3, Mat4, Vec3};
use rgfx_core::{BoundingBox, BoundingSphere, Error, Mesh, Result, Scene, Vertex};

/// Summary statistics for a loaded glTF asset.
///
/// These are gathered during [`load_gltf_with_stats`] for reporting (e.g. the CLI `info` view).
/// Counts such as [`material_count`](Self::material_count) and
/// [`animation_count`](Self::animation_count) reflect what the asset *declares*, even though
/// materials are not yet shaded and animations are not yet played back.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GltfStats {
    /// Number of meshes produced (one per triangle-mode primitive that carried geometry).
    pub mesh_count: usize,
    /// Total vertex count across all produced meshes.
    pub vertex_count: usize,
    /// Total triangle count across all produced meshes.
    pub triangle_count: usize,
    /// Number of materials declared by the asset.
    pub material_count: usize,
    /// Number of animations declared by the asset.
    pub animation_count: usize,
    /// The base color RGBA factor of each produced mesh, parallel to [`Scene::meshes`].
    ///
    /// Base color textures are not sampled yet; this is the material's constant factor (or the
    /// glTF default of opaque white when a primitive has no material).
    pub base_colors: Vec<[f32; 4]>,
    /// The axis-aligned bounding box enclosing every mesh, or `None` for an empty scene.
    pub bounding_box: Option<BoundingBox>,
    /// The bounding sphere of [`bounding_box`](Self::bounding_box), or `None` for an empty scene.
    pub bounding_sphere: Option<BoundingSphere>,
}

/// Loads a glTF 2.0 / GLB asset into a [`Scene`] with node transforms baked into vertex positions.
///
/// Accepts both text `.gltf` (resolving external and data-URI buffers relative to the file) and
/// binary `.glb`. See the [module docs](self) for what is and isn't handled.
///
/// # Errors
///
/// Returns [`Error::Decode`] if the container or its buffers cannot be read/parsed, and
/// [`Error::Geometry`] if a primitive is missing positions or references out-of-range indices.
pub fn load_gltf(path: impl AsRef<Path>) -> Result<Scene> {
    load_gltf_with_stats(path).map(|(scene, _)| scene)
}

/// Like [`load_gltf`], but also returns [`GltfStats`] describing the loaded asset.
///
/// # Errors
///
/// Same as [`load_gltf`].
pub fn load_gltf_with_stats(path: impl AsRef<Path>) -> Result<(Scene, GltfStats)> {
    let path = path.as_ref();
    let (document, buffers, _images) =
        gltf::import(path).map_err(|e| Error::Decode(format!("glTF import failed: {e}")))?;

    let mut meshes = Vec::new();
    let mut base_colors = Vec::new();

    // Traverse the default scene (falling back to the first) so node transforms compose from the
    // roots down. An asset with no scenes yields an empty result, which is valid, not an error.
    let scene = document
        .default_scene()
        .or_else(|| document.scenes().next());
    if let Some(scene) = scene {
        for node in scene.nodes() {
            process_node(
                &node,
                Mat4::IDENTITY,
                &buffers,
                &mut meshes,
                &mut base_colors,
            )?;
        }
    }

    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let scene = Scene::new(name, meshes);

    let bounding_box = scene.bounding_box();
    let stats = GltfStats {
        mesh_count: scene.meshes.len(),
        vertex_count: scene.vertex_count(),
        triangle_count: scene.triangle_count(),
        material_count: document.materials().count(),
        animation_count: document.animations().count(),
        base_colors,
        bounding_box,
        bounding_sphere: bounding_box.map(|b| b.bounding_sphere()),
    };

    Ok((scene, stats))
}

/// Recursively walks a node, composing `parent` with the node's local transform and baking the
/// resulting world transform into any triangle geometry the node references.
fn process_node(
    node: &gltf::Node,
    parent: Mat4,
    buffers: &[gltf::buffer::Data],
    meshes: &mut Vec<Mesh>,
    base_colors: &mut Vec<[f32; 4]>,
) -> Result<()> {
    let world = parent * Mat4::from_cols_array_2d(&node.transform().matrix());

    if let Some(mesh) = node.mesh() {
        // Normals transform by the inverse-transpose of the linear part to survive non-uniform
        // scale/shear; positions transform by the full affine matrix.
        let normal_matrix = Mat3::from_mat4(world).inverse().transpose();
        for primitive in mesh.primitives() {
            if primitive.mode() != gltf::mesh::Mode::Triangles {
                continue;
            }
            if let Some((mesh, base_color)) =
                load_primitive(&primitive, world, normal_matrix, buffers)?
            {
                meshes.push(mesh);
                base_colors.push(base_color);
            }
        }
    }

    for child in node.children() {
        process_node(&child, world, buffers, meshes, base_colors)?;
    }
    Ok(())
}

/// Bakes a single triangle-mode primitive into a [`Mesh`], returning it together with its material
/// base color factor. Returns `Ok(None)` when the primitive carries no vertices.
fn load_primitive(
    primitive: &gltf::Primitive,
    world: Mat4,
    normal_matrix: Mat3,
    buffers: &[gltf::buffer::Data],
) -> Result<Option<(Mesh, [f32; 4])>> {
    let reader = primitive.reader(|buffer| buffers.get(buffer.index()).map(|d| &d.0[..]));

    let positions: Vec<[f32; 3]> = reader
        .read_positions()
        .ok_or_else(|| Error::Geometry("glTF primitive is missing POSITION attribute".into()))?
        .collect();
    if positions.is_empty() {
        return Ok(None);
    }
    let normals: Vec<[f32; 3]> = reader
        .read_normals()
        .map(|n| n.collect())
        .unwrap_or_default();

    let vertices: Vec<Vertex> = positions
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let position = world.transform_point3(Vec3::from_array(*p));
            let normal = normals
                .get(i)
                .map(|n| (normal_matrix * Vec3::from_array(*n)).normalize_or_zero())
                .unwrap_or(Vec3::ZERO);
            Vertex::new(position, normal)
        })
        .collect();

    let indices: Vec<u32> = match reader.read_indices() {
        Some(indices) => indices.into_u32().collect(),
        None => (0..vertices.len() as u32).collect(),
    };
    if indices.len() % 3 != 0 {
        return Err(Error::Geometry(format!(
            "glTF triangle primitive has {} indices, not a multiple of three",
            indices.len()
        )));
    }
    if let Some(&bad) = indices.iter().find(|&&i| i as usize >= vertices.len()) {
        return Err(Error::Geometry(format!(
            "glTF index {bad} is out of range for {} vertices",
            vertices.len()
        )));
    }

    let base_color = primitive
        .material()
        .pbr_metallic_roughness()
        .base_color_factor();

    Ok(Some((Mesh::new(vertices, indices), base_color)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn asset(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("assets")
            .join(name)
    }

    const EPS: f32 = 1e-5;

    #[test]
    fn loads_external_buffer_gltf_with_counts_and_bounds() {
        let (scene, stats) = load_gltf_with_stats(asset("triangle.gltf")).unwrap();
        assert_eq!(scene.name, "triangle");
        assert_eq!(scene.meshes.len(), 1);
        assert_eq!(scene.vertex_count(), 3);
        assert_eq!(scene.triangle_count(), 1);

        let bb = scene.bounding_box().unwrap();
        assert!((bb.min - Vec3::new(0.0, 0.0, 0.0)).length() < EPS);
        assert!((bb.max - Vec3::new(1.0, 1.0, 0.0)).length() < EPS);

        assert_eq!(stats.mesh_count, 1);
        assert_eq!(stats.triangle_count, 1);
        assert_eq!(stats.material_count, 1);
        assert_eq!(stats.animation_count, 0);
        assert_eq!(stats.base_colors, vec![[1.0, 0.0, 0.0, 1.0]]);
        assert!(stats.bounding_sphere.is_some());
    }

    #[test]
    fn glb_bakes_node_translation_into_bounds() {
        let (scene, stats) = load_gltf_with_stats(asset("translated_triangle.glb")).unwrap();
        assert_eq!(scene.triangle_count(), 1);

        // The node translates the triangle by (10, 0, 0); baked bounds must shift with it.
        let bb = scene.bounding_box().unwrap();
        assert!((bb.min - Vec3::new(10.0, 0.0, 0.0)).length() < EPS);
        assert!((bb.max - Vec3::new(11.0, 1.0, 0.0)).length() < EPS);

        assert_eq!(stats.base_colors, vec![[0.0, 1.0, 0.0, 1.0]]);
    }

    #[test]
    fn plain_load_gltf_matches_with_stats() {
        let scene = load_gltf(asset("triangle.gltf")).unwrap();
        assert_eq!(scene.triangle_count(), 1);
    }

    #[test]
    fn malformed_container_errors_without_panic() {
        let err = load_gltf(asset("malformed.glb")).unwrap_err();
        assert!(matches!(err, Error::Decode(_)));
    }

    #[test]
    fn missing_file_errors() {
        assert!(load_gltf(asset("does-not-exist.gltf")).is_err());
    }
}
