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

use glam::{Mat3, Mat4, Quat, Vec3};
use rgfx_core::{BoundingBox, BoundingSphere, Error, Mesh, Result, Scene, Vertex};

use crate::anim::{
    AnimatedScene, Channel, Interpolation, MeshInstance, NodeTransform, SceneAnimation, SceneNode,
    Track,
};

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
    /// The name of each declared animation, parallel to [`animation_durations`](Self::animation_durations).
    ///
    /// Unnamed animations are reported as `animation{i}` so every entry has a stable label.
    pub animation_names: Vec<String>,
    /// The duration in seconds of each declared animation (its latest keyframe time), parallel to
    /// [`animation_names`](Self::animation_names).
    pub animation_durations: Vec<f32>,
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

    let animations = collect_animations(&document, &buffers);
    let bounding_box = scene.bounding_box();
    let stats = GltfStats {
        mesh_count: scene.meshes.len(),
        vertex_count: scene.vertex_count(),
        triangle_count: scene.triangle_count(),
        material_count: document.materials().count(),
        animation_count: animations.len(),
        animation_names: animations.iter().map(|a| a.name.clone()).collect(),
        animation_durations: animations.iter().map(|a| a.duration).collect(),
        base_colors,
        bounding_box,
        bounding_sphere: bounding_box.map(|b| b.bounding_sphere()),
    };

    Ok((scene, stats))
}

/// Loads a glTF 2.0 / GLB asset for **animation playback**: the un-baked node hierarchy, each mesh
/// kept in its local space, and the asset's keyframed TRS animations. Unlike [`load_gltf`] (which
/// bakes world transforms into vertices for a still model), the returned [`AnimatedScene`] can be
/// posed at any animation time via [`AnimatedScene::pose_into`].
///
/// The same [`GltfStats`] as [`load_gltf_with_stats`] is returned. Skeletal skinning is out of
/// scope — it is detected ([`AnimatedScene::has_skinning`]) so the caller can warn, but only the
/// rigid node motion is represented.
///
/// # Errors
///
/// Same as [`load_gltf`].
pub fn load_gltf_animated(path: impl AsRef<Path>) -> Result<(AnimatedScene, GltfStats)> {
    let path = path.as_ref();
    let (document, buffers, _images) =
        gltf::import(path).map_err(|e| Error::Decode(format!("glTF import failed: {e}")))?;

    // One SceneNode per document node, indexed so animation channels address them directly.
    let node_count = document.nodes().count();
    let mut nodes = vec![SceneNode::default(); node_count];
    let mut instances = Vec::new();
    let mut base_colors = Vec::new();
    let mut has_skinning = false;

    for node in document.nodes() {
        let idx = node.index();
        let (t, r, s) = node.transform().decomposed();
        let slot = &mut nodes[idx];
        slot.name = node.name().map(str::to_owned);
        slot.transform = NodeTransform::new(
            Vec3::from_array(t),
            Quat::from_array(r),
            Vec3::from_array(s),
        );
        slot.children = node.children().map(|c| c.index()).collect();
        if node.skin().is_some() {
            has_skinning = true;
        }

        if let Some(mesh) = node.mesh() {
            for primitive in mesh.primitives() {
                if primitive.mode() != gltf::mesh::Mode::Triangles {
                    continue;
                }
                if let Some((local_mesh, base_color)) = load_local_primitive(&primitive, &buffers)?
                {
                    instances.push(MeshInstance {
                        node: idx,
                        mesh: local_mesh,
                        base_color,
                    });
                    base_colors.push(base_color);
                }
            }
        }
    }

    let scene = document
        .default_scene()
        .or_else(|| document.scenes().next());
    let roots: Vec<usize> = scene
        .map(|s| s.nodes().map(|n| n.index()).collect())
        .unwrap_or_default();

    let animations = collect_animations(&document, &buffers);

    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();

    let animated = AnimatedScene {
        name,
        nodes,
        roots,
        instances,
        animations,
        has_skinning,
    };

    // Reuse the baked rest pose for reporting the same counts/bounds as the static loader.
    let rest = animated.pose(None, 0.0);
    let bounding_box = rest.bounding_box();
    let stats = GltfStats {
        mesh_count: rest.meshes.len(),
        vertex_count: rest.vertex_count(),
        triangle_count: rest.triangle_count(),
        material_count: document.materials().count(),
        animation_count: animated.animations.len(),
        animation_names: animated.animations.iter().map(|a| a.name.clone()).collect(),
        animation_durations: animated.animations.iter().map(|a| a.duration).collect(),
        base_colors,
        bounding_box,
        bounding_sphere: bounding_box.map(|b| b.bounding_sphere()),
    };

    Ok((animated, stats))
}

/// Reads every animation in `document` into engine [`SceneAnimation`]s (translation/rotation/scale
/// channels, `LINEAR`/`STEP` interpolation — `CUBICSPLINE` degrades to `LINEAR`). Channels whose
/// target property is unsupported here (e.g. morph-target weights) are skipped.
fn collect_animations(
    document: &gltf::Document,
    buffers: &[gltf::buffer::Data],
) -> Vec<SceneAnimation> {
    use gltf::animation::util::ReadOutputs;
    use gltf::animation::{Interpolation as GInterp, Property};

    let mut animations = Vec::new();
    for (i, anim) in document.animations().enumerate() {
        let name = anim
            .name()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("animation{i}"));
        let mut channels = Vec::new();
        for channel in anim.channels() {
            let target = channel.target();
            let node = target.node().index();
            let interpolation = match channel.sampler().interpolation() {
                GInterp::Step => Interpolation::Step,
                // CubicSpline tangents are not modelled; fall back to linear between values.
                GInterp::Linear | GInterp::CubicSpline => Interpolation::Linear,
            };
            let reader = channel.reader(|buffer| buffers.get(buffer.index()).map(|d| &d.0[..]));
            let Some(times) = reader.read_inputs() else {
                continue;
            };
            let times: Vec<f32> = times.collect();
            let Some(outputs) = reader.read_outputs() else {
                continue;
            };
            let track = match (target.property(), outputs) {
                (Property::Translation, ReadOutputs::Translations(it)) => {
                    Track::Translation(it.map(Vec3::from_array).collect())
                }
                (Property::Scale, ReadOutputs::Scales(it)) => {
                    Track::Scale(it.map(Vec3::from_array).collect())
                }
                (Property::Rotation, ReadOutputs::Rotations(rot)) => {
                    Track::Rotation(rot.into_f32().map(Quat::from_array).collect())
                }
                // Morph-target weights and any mismatched pairing are out of scope.
                _ => continue,
            };
            if times.is_empty() {
                continue;
            }
            channels.push(Channel {
                node,
                interpolation,
                times,
                track,
            });
        }
        let mut animation = SceneAnimation {
            name,
            channels,
            duration: 0.0,
        };
        animation.compute_duration();
        animations.push(animation);
    }
    animations
}

/// Reads a single triangle-mode primitive into a **local-space** [`Mesh`] (no transform baked in),
/// returning it with its material base color. Returns `Ok(None)` for an empty primitive.
fn load_local_primitive(
    primitive: &gltf::Primitive,
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
            let position = Vec3::from_array(*p);
            let normal = normals
                .get(i)
                .map(|n| Vec3::from_array(*n))
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

    #[test]
    fn stats_report_animation_names_and_durations() {
        let (_scene, stats) = load_gltf_with_stats(asset("animated_triangle.gltf")).unwrap();
        assert_eq!(stats.animation_count, 1);
        assert_eq!(stats.animation_names, vec!["bob".to_string()]);
        assert_eq!(stats.animation_durations.len(), 1);
        assert!((stats.animation_durations[0] - 1.0).abs() < EPS);
    }

    #[test]
    fn animated_loader_exposes_hierarchy_and_channels() {
        let (animated, stats) = load_gltf_animated(asset("animated_triangle.gltf")).unwrap();
        assert_eq!(animated.name, "animated_triangle");
        assert_eq!(animated.nodes.len(), 1);
        assert_eq!(animated.roots, vec![0]);
        assert_eq!(animated.instances.len(), 1);
        assert!(!animated.has_skinning);
        assert_eq!(animated.animations.len(), 1);
        assert_eq!(animated.animations[0].channels.len(), 1);
        // The local (un-baked) triangle sits at the origin regardless of animation.
        let local = &animated.instances[0].mesh;
        assert_eq!(local.vertices.len(), 3);
        assert!((local.vertices[0].position - Vec3::ZERO).length() < EPS);
        // Stats mirror the static loader's counts.
        assert_eq!(stats.triangle_count, 1);
    }

    #[test]
    fn posing_moves_vertices_between_t0_and_midpoint() {
        let (animated, _stats) = load_gltf_animated(asset("animated_triangle.gltf")).unwrap();
        // The node translates +10 in Y from t=0 to t=1. At t=0 the bounds start at y=0; at the
        // midpoint the whole triangle has shifted up by ~5, so its bounds differ.
        let rest = animated.pose(Some(0), 0.0);
        let mid = animated.pose(Some(0), 0.5);
        let rest_bb = rest.bounding_box().unwrap();
        let mid_bb = mid.bounding_box().unwrap();
        assert!(rest_bb.min.y.abs() < EPS, "rest pose starts at y=0");
        assert!(
            (mid_bb.min.y - 5.0).abs() < EPS,
            "midpoint pose has translated up by 5, got {}",
            mid_bb.min.y
        );
        assert!(
            (mid_bb.min - rest_bb.min).length() > 1.0,
            "vertex positions must differ between t=0 and t=mid"
        );
    }

    #[test]
    fn static_loader_matches_animated_rest_pose() {
        // The baked static scene must equal the animated scene posed at its rest (t=0).
        let baked = load_gltf(asset("animated_triangle.gltf")).unwrap();
        let (animated, _) = load_gltf_animated(asset("animated_triangle.gltf")).unwrap();
        let rest = animated.pose(None, 0.0);
        assert_eq!(baked.triangle_count(), rest.triangle_count());
        let (a, b) = (baked.bounding_box().unwrap(), rest.bounding_box().unwrap());
        assert!((a.min - b.min).length() < EPS && (a.max - b.max).length() < EPS);
    }
}
