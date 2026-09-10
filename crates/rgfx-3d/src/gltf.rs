//! glTF 2.0 / GLB loader.
//!
//! [`load_gltf`] reads a glTF 2.0 asset — either a JSON `.gltf` file (with its external or
//! data-URI buffers) or a binary `.glb` container — into an [`rgfx_core::Scene`] whose meshes
//! carry world-space vertex positions. Each node's local transform is composed with its parents'
//! transforms and applied to the vertices it references, so a multi-node scene assembles in the
//! right place without the renderer needing per-mesh model matrices.
//!
//! For **animation**, [`load_gltf_animated`] returns an [`AnimatedScene`] instead: the node
//! hierarchy plus each mesh's *un-baked* local-space vertices and the asset's TRS animations, so
//! the scene can be re-posed every frame. The static [`load_gltf`] path is the default pose of the
//! very same data ([`AnimatedScene::bake_static`]).
//!
//! Materials are read only far enough to pull each primitive's base color *factor* (no texture
//! sampling — that is a later task); the factors and the asset's material/animation counts are
//! reported through [`GltfStats`] via [`load_gltf_with_stats`] so the CLI `info` view can surface
//! them. Only triangle-mode primitives are turned into geometry; other primitive modes (points,
//! lines, strips/fans) are outside this task's scope and are skipped. **Skeletal skinning is not
//! applied** — a skinned animation is detected (see [`AnimationInfo::skinned`]) and plays only its
//! rigid node motion.
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

use std::collections::HashMap;
use std::path::Path;

use glam::Vec3;
use rgfx_core::{BoundingBox, BoundingSphere, Error, Mesh, Result, Scene, Vertex};

use crate::anim::{
    AnimChannel, AnimatedScene, AnimationInfo, ChannelSamples, Interpolation, MeshInstance,
    NodeTransform, SceneAnimation, SceneNode,
};

/// Summary statistics for a loaded glTF asset.
///
/// These are gathered during [`load_gltf_with_stats`] for reporting (e.g. the CLI `info` view).
/// Counts such as [`material_count`](Self::material_count) reflect what the asset *declares*, even
/// though materials are not yet shaded.
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
    /// The name, duration, and skinning flag of each declared animation.
    pub animations: Vec<AnimationInfo>,
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
    let (animated, material_count) = import_animated(path.as_ref())?;
    let scene = animated.bake_static();

    let base_colors: Vec<[f32; 4]> = animated.instances.iter().map(|i| i.base_color).collect();
    let bounding_box = scene.bounding_box();
    let stats = GltfStats {
        mesh_count: scene.meshes.len(),
        vertex_count: scene.vertex_count(),
        triangle_count: scene.triangle_count(),
        material_count,
        animation_count: animated.animations.len(),
        animations: animated.animation_infos(),
        base_colors,
        bounding_box,
        bounding_sphere: bounding_box.map(|b| b.bounding_sphere()),
    };

    Ok((scene, stats))
}

/// Loads a glTF 2.0 / GLB asset into an [`AnimatedScene`]: the node hierarchy, each mesh's
/// *local-space* (un-baked) vertices, and the asset's TRS animations — the form required for
/// time-based playback.
///
/// For a model with no animations this still returns the full hierarchy (with an empty
/// [`AnimatedScene::animations`]); [`AnimatedScene::bake_static`] reproduces [`load_gltf`]'s
/// output.
///
/// # Errors
///
/// Same as [`load_gltf`].
pub fn load_gltf_animated(path: impl AsRef<Path>) -> Result<AnimatedScene> {
    import_animated(path.as_ref()).map(|(scene, _)| scene)
}

/// Imports a glTF asset into an [`AnimatedScene`], also returning the declared material count.
fn import_animated(path: &Path) -> Result<(AnimatedScene, usize)> {
    let (document, buffers, _images) =
        gltf::import(path).map_err(|e| Error::Decode(format!("glTF import failed: {e}")))?;

    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();

    let mut builder = SceneBuilder::default();
    // Traverse the default scene (falling back to the first) so the hierarchy roots are consistent
    // with the baked loader. An asset with no scenes yields an empty result, which is valid.
    let scene = document
        .default_scene()
        .or_else(|| document.scenes().next());
    if let Some(scene) = scene {
        for node in scene.nodes() {
            builder.visit(&node, None, &buffers)?;
        }
    }

    let animations = build_animations(&document, &buffers, &builder.gltf_to_local);

    let animated = AnimatedScene {
        name,
        nodes: builder.nodes,
        roots: builder.roots,
        instances: builder.instances,
        animations,
    };
    Ok((animated, document.materials().count()))
}

/// Accumulates the node hierarchy and local mesh instances during a depth-first scene walk.
///
/// Nodes are pushed in pre-order, so a node's dense index is always greater than its parent's —
/// which lets [`crate::anim::SceneAnimator`] compose world matrices in a single ascending pass.
#[derive(Default)]
struct SceneBuilder {
    nodes: Vec<SceneNode>,
    roots: Vec<usize>,
    instances: Vec<MeshInstance>,
    /// Maps a glTF node index to its dense index in [`nodes`](Self::nodes), for channel targeting.
    gltf_to_local: HashMap<usize, usize>,
}

impl SceneBuilder {
    /// Visits `node`, recording its local transform, its mesh instances (in local space), and
    /// recursing into its children.
    fn visit(
        &mut self,
        node: &gltf::Node,
        parent: Option<usize>,
        buffers: &[gltf::buffer::Data],
    ) -> Result<()> {
        let dense = self.nodes.len();
        self.gltf_to_local.insert(node.index(), dense);

        let (t, r, s) = node.transform().decomposed();
        self.nodes.push(SceneNode {
            name: node.name().map(str::to_owned),
            local: NodeTransform::from_trs(t, r, s),
            parent,
            children: Vec::new(),
        });
        if let Some(p) = parent {
            self.nodes[p].children.push(dense);
        } else {
            self.roots.push(dense);
        }

        if let Some(mesh) = node.mesh() {
            for primitive in mesh.primitives() {
                if primitive.mode() != gltf::mesh::Mode::Triangles {
                    continue;
                }
                if let Some((mesh, base_color)) = load_primitive(&primitive, buffers)? {
                    self.instances.push(MeshInstance {
                        node: dense,
                        mesh,
                        base_color,
                    });
                }
            }
        }

        for child in node.children() {
            self.visit(&child, Some(dense), buffers)?;
        }
        Ok(())
    }
}

/// Reads a single triangle-mode primitive into a *local-space* [`Mesh`] (no transform baked in),
/// returning it with its material base color factor. Returns `Ok(None)` for an empty primitive.
fn load_primitive(
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

/// Parses every animation in the document into a [`SceneAnimation`], mapping glTF node targets to
/// dense node indices. Channels targeting unreachable nodes, unsupported paths (morph weights), or
/// `CUBICSPLINE` tangents are handled conservatively (skipped or linearised on the value points).
fn build_animations(
    document: &gltf::Document,
    buffers: &[gltf::buffer::Data],
    gltf_to_local: &HashMap<usize, usize>,
) -> Vec<SceneAnimation> {
    let skinned_nodes = skinned_node_set(document, gltf_to_local);

    document
        .animations()
        .enumerate()
        .map(|(i, animation)| {
            let name = animation
                .name()
                .map(str::to_owned)
                .unwrap_or_else(|| format!("animation {i}"));

            let mut channels = Vec::new();
            let mut duration = 0.0f32;
            let mut skinned = false;

            for channel in animation.channels() {
                let target = channel.target();
                let Some(&node) = gltf_to_local.get(&target.node().index()) else {
                    continue;
                };
                if skinned_nodes.contains(&node) {
                    skinned = true;
                }

                let interpolation = match channel.sampler().interpolation() {
                    gltf::animation::Interpolation::Step => Interpolation::Step,
                    // LINEAR, and CUBICSPLINE linearised on its value points (tangents dropped).
                    _ => Interpolation::Linear,
                };

                let reader = channel.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
                let Some(times) = reader.read_inputs().map(|it| it.collect::<Vec<f32>>()) else {
                    continue;
                };
                if times.is_empty() {
                    continue;
                }
                let Some(outputs) = reader.read_outputs() else {
                    continue;
                };

                let samples = match outputs {
                    gltf::animation::util::ReadOutputs::Translations(it) => {
                        ChannelSamples::Translation(it.map(Vec3::from_array).collect())
                    }
                    gltf::animation::util::ReadOutputs::Scales(it) => {
                        ChannelSamples::Scale(it.map(Vec3::from_array).collect())
                    }
                    gltf::animation::util::ReadOutputs::Rotations(r) => {
                        ChannelSamples::Rotation(r.into_f32().map(glam::Quat::from_array).collect())
                    }
                    // Morph-target weights are out of scope.
                    gltf::animation::util::ReadOutputs::MorphTargetWeights(_) => continue,
                };

                if let Some(&last) = times.last() {
                    duration = duration.max(last);
                }
                channels.push(AnimChannel {
                    node,
                    interpolation,
                    times,
                    samples,
                });
            }

            SceneAnimation {
                name,
                duration,
                skinned,
                channels,
            }
        })
        .collect()
}

/// The set of dense node indices that are skin joints, so animations touching them can be flagged
/// as (unsupported) skinned even though their rigid node motion still plays.
fn skinned_node_set(
    document: &gltf::Document,
    gltf_to_local: &HashMap<usize, usize>,
) -> std::collections::HashSet<usize> {
    let mut set = std::collections::HashSet::new();
    for skin in document.skins() {
        for joint in skin.joints() {
            if let Some(&dense) = gltf_to_local.get(&joint.index()) {
                set.insert(dense);
            }
        }
    }
    set
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
        assert!(stats.animations.is_empty());
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

    // --- Animation loading ------------------------------------------------------------------------

    #[test]
    fn animated_glb_reports_animation_name_and_duration() {
        let (_scene, stats) = load_gltf_with_stats(asset("animated_triangle.glb")).unwrap();
        assert_eq!(stats.animation_count, 1);
        assert_eq!(stats.animations.len(), 1);
        assert_eq!(stats.animations[0].name, "slide");
        assert!((stats.animations[0].duration - 1.0).abs() < EPS);
        assert!(!stats.animations[0].skinned, "no skin in this fixture");
    }

    #[test]
    fn animated_scene_moves_vertices_between_t0_and_tmid() {
        let animated = load_gltf_animated(asset("animated_triangle.glb")).unwrap();
        assert!(animated.has_animations());
        assert_eq!(animated.instances.len(), 1, "one triangle primitive");

        let mut animator = crate::anim::SceneAnimator::new(animated);
        let mut scene = Scene::default();

        animator.pose_into(Some(0), 0.0, &mut scene);
        let at0 = scene.meshes[0].vertices[0].position;
        animator.pose_into(Some(0), 0.5, &mut scene);
        let at_mid = scene.meshes[0].vertices[0].position;

        // The channel slides the node +10 in x over 1s, so the midpoint is +5.
        assert!(
            (at_mid - at0 - Vec3::new(5.0, 0.0, 0.0)).length() < 1e-4,
            "vertices must move between t=0 and t=mid"
        );
    }

    #[test]
    fn baked_static_pose_matches_default_load() {
        // The static baked scene is the animated scene posed at its defaults.
        let animated = load_gltf_animated(asset("triangle.gltf")).unwrap();
        let baked = animated.bake_static();
        let direct = load_gltf(asset("triangle.gltf")).unwrap();
        assert_eq!(baked.triangle_count(), direct.triangle_count());
        assert_eq!(baked.vertex_count(), direct.vertex_count());
    }
}
