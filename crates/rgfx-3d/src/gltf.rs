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
    /// The name and duration (seconds) of each declared animation, in document order.
    ///
    /// Parallel in length to [`animation_count`](Self::animation_count); the CLI `info` view
    /// surfaces these so a user can see what an animated asset contains before playing it.
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

    let animations = animation_infos(&document, &buffers);
    let bounding_box = scene.bounding_box();
    let stats = GltfStats {
        mesh_count: scene.meshes.len(),
        vertex_count: scene.vertex_count(),
        triangle_count: scene.triangle_count(),
        material_count: document.materials().count(),
        animation_count: document.animations().count(),
        animations,
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

// ---------------------------------------------------------------------------------------------
// Rigid node-transform animation (task 038)
// ---------------------------------------------------------------------------------------------

/// The name and duration (seconds) of one declared animation.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationInfo {
    /// The animation's name, or a synthesized `animation N` when the asset left it unnamed.
    pub name: String,
    /// The animation's duration in seconds: the largest keyframe time across its channels.
    pub duration: f32,
}

/// A node's local transform expressed as translation / rotation / scale (TRS).
///
/// This is the un-baked, animatable form: animation channels overwrite one component at a time,
/// and [`to_matrix`](Self::to_matrix) composes it back into a local affine matrix for hierarchy
/// composition.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NodeTransform {
    /// Local translation.
    pub translation: Vec3,
    /// Local rotation.
    pub rotation: Quat,
    /// Local scale.
    pub scale: Vec3,
}

impl NodeTransform {
    /// The identity transform (no translation, unit rotation, unit scale).
    pub const IDENTITY: Self = Self {
        translation: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    };

    /// Composes the TRS components into a local affine matrix (`T * R * S`).
    pub fn to_matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }
}

impl Default for NodeTransform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// How a channel's keyframe values are interpolated between samples.
///
/// Only the two piecewise modes are represented; a `CUBICSPLINE` sampler is downgraded to
/// [`Linear`](Self::Linear) on load (its per-keyframe value is kept, its tangents dropped) so
/// playback never crashes on an unsupported mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interpolation {
    /// Piecewise-linear (positions/scales lerp, rotations slerp).
    Linear,
    /// Step (hold the previous keyframe's value until the next).
    Step,
}

/// The per-keyframe values of one channel, one variant per animatable TRS component.
#[derive(Clone, Debug)]
enum Keyframes {
    Translation(Vec<Vec3>),
    Rotation(Vec<Quat>),
    Scale(Vec<Vec3>),
}

/// One animation channel: a keyframed track driving a single TRS component of one node.
#[derive(Clone, Debug)]
struct AnimationChannel {
    /// The node whose transform this channel drives (index into the node arrays).
    target_node: usize,
    /// Keyframe times in seconds, ascending; parallel to the value list.
    times: Vec<f32>,
    /// The keyframe values (which component is driven is encoded by the variant).
    values: Keyframes,
    /// How values are interpolated between keyframes.
    interpolation: Interpolation,
}

/// A single glTF animation: a named set of channels with a total duration.
#[derive(Clone, Debug)]
pub struct SceneAnimation {
    /// The animation name (synthesized when the asset left it unnamed).
    name: String,
    /// The channels driving node transforms.
    channels: Vec<AnimationChannel>,
    /// Duration in seconds (the largest keyframe time across all channels).
    duration: f32,
}

impl SceneAnimation {
    /// The animation's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The animation's duration in seconds (largest keyframe time across its channels).
    pub fn duration(&self) -> f32 {
        self.duration
    }
}

/// An un-baked glTF scene: local-space meshes plus the node hierarchy and animation tracks needed
/// to pose it at an arbitrary time.
///
/// Unlike [`load_gltf`], which bakes each node's world transform into its vertices for a static
/// render, this keeps meshes in their node-local space and records the transform hierarchy so
/// [`animate_into`](Self::animate_into) can compose world transforms at time `t` and rebuild the
/// scene's world-space vertices each frame.
///
/// Skeletal skinning is **not** applied — a skinned asset's node-transform animation still plays,
/// but per-vertex joint deformation is ignored (see [`has_skinning`](Self::has_skinning)).
#[derive(Clone, Debug)]
pub struct AnimatedScene {
    /// A source name (the file stem).
    name: String,
    /// Meshes in node-local space, parallel to [`mesh_nodes`](Self::mesh_nodes).
    meshes: Vec<Mesh>,
    /// The owning node index of each mesh, parallel to [`meshes`](Self::meshes).
    mesh_nodes: Vec<usize>,
    /// The rest-pose local transform of every node (indexed by glTF node index).
    base_transforms: Vec<NodeTransform>,
    /// The child node indices of every node (indexed by glTF node index).
    children: Vec<Vec<usize>>,
    /// The scene's root node indices.
    roots: Vec<usize>,
    /// The declared animations.
    animations: Vec<SceneAnimation>,
    /// Whether the asset declares skins (skeletal skinning, which is not applied).
    has_skinning: bool,
}

impl AnimatedScene {
    /// The declared animations, in document order.
    pub fn animations(&self) -> &[SceneAnimation] {
        &self.animations
    }

    /// Whether the asset declares skins. Skinning deformation is not applied; node-transform
    /// motion still plays. The CLI uses this to surface a "skinning not supported" note.
    pub fn has_skinning(&self) -> bool {
        self.has_skinning
    }

    /// Whether the asset carries at least one animation.
    pub fn has_animations(&self) -> bool {
        !self.animations.is_empty()
    }

    /// Evaluates animation `anim_index` at `time` seconds, returning the local transform of every
    /// node (indexed by glTF node index). Nodes not driven by the animation keep their rest-pose
    /// transform. An out-of-range `anim_index` yields the rest pose unchanged.
    pub fn evaluate(&self, anim_index: usize, time: f32) -> Vec<NodeTransform> {
        let mut locals = self.base_transforms.clone();
        if let Some(anim) = self.animations.get(anim_index) {
            for ch in &anim.channels {
                let Some(node) = locals.get_mut(ch.target_node) else {
                    continue;
                };
                match &ch.values {
                    Keyframes::Translation(v) => {
                        node.translation = sample_vec3(&ch.times, v, ch.interpolation, time);
                    }
                    Keyframes::Scale(v) => {
                        node.scale = sample_vec3(&ch.times, v, ch.interpolation, time);
                    }
                    Keyframes::Rotation(q) => {
                        node.rotation = sample_quat(&ch.times, q, ch.interpolation, time);
                    }
                }
            }
        }
        locals
    }

    /// Composes the world matrix of every node from `locals`, walking the hierarchy from the roots
    /// so each child multiplies its parent's world matrix (parent motion drags its children along).
    fn world_matrices(&self, locals: &[NodeTransform]) -> Vec<Mat4> {
        let mut world = vec![Mat4::IDENTITY; locals.len()];
        // Iterative DFS: (node, parent world). Roots start from identity.
        let mut stack: Vec<(usize, Mat4)> =
            self.roots.iter().map(|&r| (r, Mat4::IDENTITY)).collect();
        while let Some((n, parent)) = stack.pop() {
            let Some(local) = locals.get(n) else {
                continue;
            };
            let w = parent * local.to_matrix();
            world[n] = w;
            for &c in &self.children[n] {
                stack.push((c, w));
            }
        }
        world
    }

    /// Bakes node world matrices into `out`, transforming each local mesh into world space.
    ///
    /// `out`'s mesh/vertex/index buffers are reused in place when their shapes already match, so
    /// repeated per-frame calls do not reallocate.
    fn bake_world_into(&self, world: &[Mat4], out: &mut Scene) {
        out.name.clone_from(&self.name);
        if out.meshes.len() != self.meshes.len() {
            out.meshes.resize_with(self.meshes.len(), Mesh::default);
        }
        for (i, src) in self.meshes.iter().enumerate() {
            let w = world[self.mesh_nodes[i]];
            // Normals transform by the inverse-transpose of the linear part; positions by the
            // full affine matrix.
            let normal_matrix = Mat3::from_mat4(w).inverse().transpose();
            let dst = &mut out.meshes[i];
            if dst.vertices.len() != src.vertices.len() {
                dst.vertices
                    .resize(src.vertices.len(), Vertex::from_position(Vec3::ZERO));
            }
            for (d, s) in dst.vertices.iter_mut().zip(src.vertices.iter()) {
                d.position = w.transform_point3(s.position);
                d.normal = (normal_matrix * s.normal).normalize_or_zero();
            }
            if dst.indices != src.indices {
                dst.indices.clone_from(&src.indices);
            }
        }
    }

    /// Poses the scene for animation `anim_index` at `time` seconds, writing world-space geometry
    /// into `out` (reusing its buffers). Use a single `out` [`Scene`] across frames to avoid
    /// per-frame allocation.
    pub fn animate_into(&self, anim_index: usize, time: f32, out: &mut Scene) {
        let locals = self.evaluate(anim_index, time);
        let world = self.world_matrices(&locals);
        self.bake_world_into(&world, out);
    }

    /// Builds the rest-pose (un-animated) [`Scene`], with base node transforms baked into world
    /// space. Used for initial camera framing and as the static fallback.
    pub fn rest_scene(&self) -> Scene {
        let world = self.world_matrices(&self.base_transforms);
        let mut scene = Scene::default();
        self.bake_world_into(&world, &mut scene);
        scene
    }
}

/// Samples a `Vec3` track (translation/scale) at `time`, holding the endpoints outside the range.
fn sample_vec3(times: &[f32], values: &[Vec3], interp: Interpolation, time: f32) -> Vec3 {
    match find_segment(times, time) {
        Segment::Before => values.first().copied().unwrap_or(Vec3::ZERO),
        Segment::After => values.last().copied().unwrap_or(Vec3::ZERO),
        Segment::Between(i, factor) => match interp {
            Interpolation::Step => values[i],
            Interpolation::Linear => values[i].lerp(values[i + 1], factor),
        },
    }
}

/// Samples a rotation track at `time`, holding the endpoints outside the range. Linear rotation
/// uses spherical interpolation (slerp).
fn sample_quat(times: &[f32], values: &[Quat], interp: Interpolation, time: f32) -> Quat {
    match find_segment(times, time) {
        Segment::Before => values.first().copied().unwrap_or(Quat::IDENTITY),
        Segment::After => values.last().copied().unwrap_or(Quat::IDENTITY),
        Segment::Between(i, factor) => match interp {
            Interpolation::Step => values[i],
            Interpolation::Linear => values[i].slerp(values[i + 1], factor),
        },
    }
}

/// Where `time` falls relative to a keyframe time list.
enum Segment {
    /// At or before the first keyframe.
    Before,
    /// At or after the last keyframe.
    After,
    /// Between keyframes `i` and `i + 1`, with a `0.0..=1.0` interpolation factor.
    Between(usize, f32),
}

/// Locates `time` within an ascending keyframe list.
fn find_segment(times: &[f32], time: f32) -> Segment {
    if times.len() < 2 {
        return Segment::Before;
    }
    if time <= times[0] {
        return Segment::Before;
    }
    if time >= times[times.len() - 1] {
        return Segment::After;
    }
    // Linear scan (keyframe counts are small); find i with times[i] <= time < times[i+1].
    for i in 0..times.len() - 1 {
        if time < times[i + 1] {
            let span = times[i + 1] - times[i];
            let factor = if span > f32::EPSILON {
                (time - times[i]) / span
            } else {
                0.0
            };
            return Segment::Between(i, factor.clamp(0.0, 1.0));
        }
    }
    Segment::After
}

/// Gathers the name + duration of every declared animation (cheap: only reads keyframe input
/// times, not the value buffers).
fn animation_infos(
    document: &gltf::Document,
    buffers: &[gltf::buffer::Data],
) -> Vec<AnimationInfo> {
    document
        .animations()
        .enumerate()
        .map(|(i, anim)| {
            let mut duration = 0.0f32;
            for channel in anim.channels() {
                let reader = channel.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
                if let Some(inputs) = reader.read_inputs() {
                    if let Some(last) = inputs.last() {
                        duration = duration.max(last);
                    }
                }
            }
            AnimationInfo {
                name: anim
                    .name()
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("animation {i}")),
                duration,
            }
        })
        .collect()
}

/// Loads a glTF / GLB asset in un-baked form for animation playback: local-space meshes, the node
/// hierarchy, and the animation tracks.
///
/// The rest pose (via [`AnimatedScene::rest_scene`]) matches what [`load_gltf`] produces for the
/// same file. Only triangle-mode primitives become geometry, as with the static loader.
///
/// # Errors
///
/// Same as [`load_gltf`]: [`Error::Decode`] for a bad container/buffer, [`Error::Geometry`] for a
/// primitive missing positions or with out-of-range indices.
pub fn load_gltf_animated(path: impl AsRef<Path>) -> Result<AnimatedScene> {
    let path = path.as_ref();
    let (document, buffers, _images) =
        gltf::import(path).map_err(|e| Error::Decode(format!("glTF import failed: {e}")))?;

    let node_count = document.nodes().count();
    let mut base_transforms = vec![NodeTransform::IDENTITY; node_count];
    let mut children = vec![Vec::new(); node_count];
    for node in document.nodes() {
        let (t, r, s) = node.transform().decomposed();
        base_transforms[node.index()] = NodeTransform {
            translation: Vec3::from_array(t),
            rotation: Quat::from_array(r),
            scale: Vec3::from_array(s),
        };
        children[node.index()] = node.children().map(|c| c.index()).collect();
    }

    let scene = document
        .default_scene()
        .or_else(|| document.scenes().next());
    let mut roots = Vec::new();
    let mut meshes = Vec::new();
    let mut mesh_nodes = Vec::new();
    if let Some(scene) = scene {
        for node in scene.nodes() {
            roots.push(node.index());
            collect_local_meshes(&node, &buffers, &mut meshes, &mut mesh_nodes)?;
        }
    }

    let animations = load_animations(&document, &buffers)?;
    let has_skinning = document.skins().next().is_some();
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();

    Ok(AnimatedScene {
        name,
        meshes,
        mesh_nodes,
        base_transforms,
        children,
        roots,
        animations,
        has_skinning,
    })
}

/// Recursively collects a node's triangle geometry in node-*local* space (no world baking),
/// recording the owning node index for each produced mesh.
fn collect_local_meshes(
    node: &gltf::Node,
    buffers: &[gltf::buffer::Data],
    meshes: &mut Vec<Mesh>,
    mesh_nodes: &mut Vec<usize>,
) -> Result<()> {
    if let Some(mesh) = node.mesh() {
        for primitive in mesh.primitives() {
            if primitive.mode() != gltf::mesh::Mode::Triangles {
                continue;
            }
            if let Some((mesh, _base_color)) =
                load_primitive(&primitive, Mat4::IDENTITY, Mat3::IDENTITY, buffers)?
            {
                meshes.push(mesh);
                mesh_nodes.push(node.index());
            }
        }
    }
    for child in node.children() {
        collect_local_meshes(&child, buffers, meshes, mesh_nodes)?;
    }
    Ok(())
}

/// Reads every animation's channels into [`SceneAnimation`]s. Morph-target-weight channels are
/// skipped (out of scope); `CUBICSPLINE` samplers are downgraded to linear.
fn load_animations(
    document: &gltf::Document,
    buffers: &[gltf::buffer::Data],
) -> Result<Vec<SceneAnimation>> {
    let mut animations = Vec::new();
    for (i, anim) in document.animations().enumerate() {
        let mut channels = Vec::new();
        let mut duration = 0.0f32;
        for channel in anim.channels() {
            let target_node = channel.target().node().index();
            let interpolation = match channel.sampler().interpolation() {
                gltf::animation::Interpolation::Step => Interpolation::Step,
                // Linear and (downgraded) CubicSpline both interpolate piecewise-linearly.
                _ => Interpolation::Linear,
            };
            let cubic = matches!(
                channel.sampler().interpolation(),
                gltf::animation::Interpolation::CubicSpline
            );
            let reader = channel.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
            let times: Vec<f32> = match reader.read_inputs() {
                Some(inputs) => inputs.collect(),
                None => continue,
            };
            if let Some(&last) = times.last() {
                duration = duration.max(last);
            }
            let Some(outputs) = reader.read_outputs() else {
                continue;
            };
            let values = match outputs {
                gltf::animation::util::ReadOutputs::Translations(iter) => {
                    Keyframes::Translation(despline(iter.map(Vec3::from_array).collect(), cubic))
                }
                gltf::animation::util::ReadOutputs::Scales(iter) => {
                    Keyframes::Scale(despline(iter.map(Vec3::from_array).collect(), cubic))
                }
                gltf::animation::util::ReadOutputs::Rotations(rot) => Keyframes::Rotation(
                    despline(rot.into_f32().map(Quat::from_array).collect(), cubic),
                ),
                // Morph-target weights are out of scope for rigid node-transform animation.
                gltf::animation::util::ReadOutputs::MorphTargetWeights(_) => continue,
            };
            channels.push(AnimationChannel {
                target_node,
                times,
                values,
                interpolation,
            });
        }
        animations.push(SceneAnimation {
            name: anim
                .name()
                .map(str::to_owned)
                .unwrap_or_else(|| format!("animation {i}")),
            channels,
            duration,
        });
    }
    Ok(animations)
}

/// For a `CUBICSPLINE` track the output has three values per keyframe (in-tangent, value,
/// out-tangent); keep only the middle value and drop the tangents. Non-cubic tracks pass through.
fn despline<T: Copy>(values: Vec<T>, cubic: bool) -> Vec<T> {
    if cubic {
        values.iter().skip(1).step_by(3).copied().collect()
    } else {
        values
    }
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

    // --- Animation: keyframe sampling -------------------------------------------------------------

    #[test]
    fn linear_vec3_sampling_at_boundaries_and_midpoint() {
        let times = [0.0, 2.0];
        let vals = [Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0)];
        // Before / at start, after / at end clamp to the endpoints.
        assert!((sample_vec3(&times, &vals, Interpolation::Linear, -1.0) - vals[0]).length() < EPS);
        assert!((sample_vec3(&times, &vals, Interpolation::Linear, 0.0) - vals[0]).length() < EPS);
        assert!((sample_vec3(&times, &vals, Interpolation::Linear, 5.0) - vals[1]).length() < EPS);
        // Midpoint lerps halfway.
        let mid = sample_vec3(&times, &vals, Interpolation::Linear, 1.0);
        assert!((mid - Vec3::new(5.0, 0.0, 0.0)).length() < EPS);
    }

    #[test]
    fn step_vec3_holds_previous_keyframe() {
        let times = [0.0, 1.0, 2.0];
        let vals = [Vec3::ZERO, Vec3::X, Vec3::Y];
        // Step holds the left keyframe across the whole interval.
        assert!((sample_vec3(&times, &vals, Interpolation::Step, 0.5) - vals[0]).length() < EPS);
        assert!((sample_vec3(&times, &vals, Interpolation::Step, 1.0) - vals[1]).length() < EPS);
        assert!((sample_vec3(&times, &vals, Interpolation::Step, 1.9) - vals[1]).length() < EPS);
    }

    #[test]
    fn linear_quat_slerps_to_midpoint() {
        let times = [0.0, 1.0];
        let q0 = Quat::IDENTITY;
        let q1 = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let mid = sample_quat(&times, &[q0, q1], Interpolation::Linear, 0.5);
        let expect = Quat::from_rotation_z(std::f32::consts::FRAC_PI_4);
        assert!(mid.angle_between(expect) < 1e-3, "slerp midpoint is 45°");
    }

    /// Builds a two-node hierarchy: root (node 0) with a child (node 1) that owns a unit triangle.
    /// An animation translates the *parent*, so the child geometry must move with it.
    fn parent_child_scene() -> AnimatedScene {
        let tri = Mesh::new(
            vec![
                Vertex::from_position(Vec3::ZERO),
                Vertex::from_position(Vec3::X),
                Vertex::from_position(Vec3::Y),
            ],
            vec![0, 1, 2],
        );
        let anim = SceneAnimation {
            name: "move_parent".to_string(),
            duration: 2.0,
            channels: vec![AnimationChannel {
                target_node: 0,
                times: vec![0.0, 2.0],
                values: Keyframes::Translation(vec![Vec3::ZERO, Vec3::new(0.0, 4.0, 0.0)]),
                interpolation: Interpolation::Linear,
            }],
        };
        AnimatedScene {
            name: "hier".to_string(),
            meshes: vec![tri],
            mesh_nodes: vec![1],
            base_transforms: vec![NodeTransform::IDENTITY, NodeTransform::IDENTITY],
            children: vec![vec![1], vec![]],
            roots: vec![0],
            animations: vec![anim],
            has_skinning: false,
        }
    }

    #[test]
    fn parent_transform_drags_child_geometry() {
        let scene = parent_child_scene();
        let mut out = Scene::default();

        scene.animate_into(0, 0.0, &mut out);
        let v0 = out.meshes[0].vertices[0].position;
        assert!((v0 - Vec3::ZERO).length() < EPS, "rest pose at origin");

        // Halfway: parent has translated +2 in y, so the child's first vertex sits at y≈2.
        scene.animate_into(0, 1.0, &mut out);
        let vmid = out.meshes[0].vertices[0].position;
        assert!(
            (vmid - Vec3::new(0.0, 2.0, 0.0)).length() < EPS,
            "child moves with parent, got {vmid:?}"
        );

        // End: parent at +4.
        scene.animate_into(0, 2.0, &mut out);
        let vend = out.meshes[0].vertices[0].position;
        assert!((vend - Vec3::new(0.0, 4.0, 0.0)).length() < EPS);
    }

    #[test]
    fn animate_into_reuses_scene_buffers() {
        let scene = parent_child_scene();
        let mut out = Scene::default();
        scene.animate_into(0, 0.0, &mut out);
        let verts_ptr = out.meshes[0].vertices.as_ptr();
        let cap = out.meshes[0].vertices.capacity();
        scene.animate_into(0, 1.0, &mut out);
        // Same allocation reused across frames (no reallocation for a fixed-shape mesh).
        assert_eq!(out.meshes[0].vertices.as_ptr(), verts_ptr);
        assert_eq!(out.meshes[0].vertices.capacity(), cap);
    }

    // --- Animation: loading a real animated asset -------------------------------------------------

    #[test]
    fn loads_animation_names_and_durations_into_stats() {
        let (_scene, stats) = load_gltf_with_stats(asset("animated_triangle.gltf")).unwrap();
        assert_eq!(stats.animation_count, 1);
        assert_eq!(stats.animations.len(), 1);
        assert_eq!(stats.animations[0].name, "slide");
        assert!((stats.animations[0].duration - 1.0).abs() < EPS);
    }

    #[test]
    fn animated_fixture_moves_between_t0_and_tmid() {
        let anim = load_gltf_animated(asset("animated_triangle.gltf")).unwrap();
        assert!(anim.has_animations());
        assert!(!anim.has_skinning());
        assert_eq!(anim.animations()[0].name(), "slide");
        assert!((anim.animations()[0].duration() - 1.0).abs() < EPS);

        // The rest pose matches the static loader (node is un-translated at t=0).
        let rest = anim.rest_scene();
        assert_eq!(rest.triangle_count(), 1);

        let mut out = Scene::default();
        anim.animate_into(0, 0.0, &mut out);
        let a = out.bounding_box().unwrap();
        anim.animate_into(0, 0.5, &mut out);
        let b = out.bounding_box().unwrap();
        // The channel slides x from 0 to 10 over 1s, so at t=0.5 the geometry has shifted +5 in x.
        assert!(
            (b.min.x - a.min.x - 5.0).abs() < 1e-3,
            "geometry must move between t=0 and t=mid ({} -> {})",
            a.min.x,
            b.min.x
        );
    }

    #[test]
    fn evaluate_out_of_range_animation_is_rest_pose() {
        let scene = parent_child_scene();
        let locals = scene.evaluate(99, 0.5);
        assert_eq!(locals[0], NodeTransform::IDENTITY);
    }

    #[test]
    fn static_file_has_no_animations() {
        let anim = load_gltf_animated(asset("triangle.gltf")).unwrap();
        assert!(!anim.has_animations());
        // A static asset still poses fine (rest scene renders the triangle).
        let mut out = Scene::default();
        anim.animate_into(0, 1.0, &mut out);
        assert_eq!(out.triangle_count(), 1);
    }
}
