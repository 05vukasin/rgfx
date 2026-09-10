//! Rigid node-transform animation: the un-baked scene graph plus keyframed TRS tracks.
//!
//! The static glTF loader ([`load_gltf`](crate::load_gltf)) bakes each node's world transform
//! straight into vertex positions, which is ideal for a still model but throws away the hierarchy
//! an animation needs. This module holds the *un-baked* counterpart: an [`AnimatedScene`] keeps
//! every node's local [`NodeTransform`] and parent→child links, keeps each mesh in its own local
//! space, and carries the asset's [`SceneAnimation`]s.
//!
//! Playback is pure and terminal-free: [`SceneAnimation::evaluate`] samples the keyframe tracks at
//! a time `t` to produce per-node local transforms, [`AnimatedScene::world_matrices`] composes
//! those down the hierarchy, and [`AnimatedScene::pose_into`] bakes the resulting world transforms
//! into a reused [`Scene`] for the renderer. Only the **rigid** node-transform case is handled
//! here — skeletal skinning (per-vertex joint weights) is detected via
//! [`AnimatedScene::has_skinning`] but not applied.
//!
//! Two keyframe interpolations are supported, matching the glTF defaults this task targets:
//! [`Interpolation::Linear`] and [`Interpolation::Step`].

use glam::{Mat3, Mat4, Quat, Vec3};
use rgfx_core::{Mesh, Scene, Vertex};

/// A node's local transform, stored as separable translation / rotation / scale so that animation
/// tracks can drive each component independently (the glTF animation model).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NodeTransform {
    /// Local translation.
    pub translation: Vec3,
    /// Local rotation.
    pub rotation: Quat,
    /// Local scale.
    pub scale: Vec3,
}

impl Default for NodeTransform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl NodeTransform {
    /// The identity transform (no translation, no rotation, unit scale).
    pub const IDENTITY: Self = Self {
        translation: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    };

    /// Builds a transform from its components.
    pub const fn new(translation: Vec3, rotation: Quat, scale: Vec3) -> Self {
        Self {
            translation,
            rotation,
            scale,
        }
    }

    /// The local affine matrix `T · R · S` for this transform.
    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }
}

/// A node in the scene hierarchy: its local transform, its child node indices, and the mesh
/// instances it drives (indices into [`AnimatedScene::instances`]).
#[derive(Clone, Debug, Default)]
pub struct SceneNode {
    /// The node's name, when the asset provided one.
    pub name: Option<String>,
    /// The node's local transform (the animation base pose for this node).
    pub transform: NodeTransform,
    /// Indices of this node's children in [`AnimatedScene::nodes`].
    pub children: Vec<usize>,
}

/// A single drawable primitive in its own local space, tagged with the node whose world transform
/// positions it.
#[derive(Clone, Debug)]
pub struct MeshInstance {
    /// Index into [`AnimatedScene::nodes`] of the node that transforms this mesh.
    pub node: usize,
    /// The local-space geometry (positions/normals as authored, no transform baked in).
    pub mesh: Mesh,
    /// The material base color RGBA factor (parallel information to the static loader's stats).
    pub base_color: [f32; 4],
}

/// Keyframe interpolation mode. Only the two modes this task targets are represented; a glTF
/// `CUBICSPLINE` sampler is loaded as [`Interpolation::Linear`] (its tangents are ignored).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interpolation {
    /// Linearly interpolate between the two surrounding keyframes (slerp for rotations).
    Linear,
    /// Hold the earlier keyframe until the next one is reached (no interpolation).
    Step,
}

/// The keyframe values of a channel, one variant per animatable node property.
#[derive(Clone, Debug, PartialEq)]
pub enum Track {
    /// Translation keyframes (parallel to the channel's times).
    Translation(Vec<Vec3>),
    /// Rotation keyframes (parallel to the channel's times).
    Rotation(Vec<Quat>),
    /// Scale keyframes (parallel to the channel's times).
    Scale(Vec<Vec3>),
}

/// One animation channel: a keyframed track targeting a single node property.
#[derive(Clone, Debug)]
pub struct Channel {
    /// Index into [`AnimatedScene::nodes`] of the driven node.
    pub node: usize,
    /// How values are interpolated between keyframes.
    pub interpolation: Interpolation,
    /// Keyframe times in seconds, strictly non-decreasing, parallel to the track values.
    pub times: Vec<f32>,
    /// The keyframe values (and which property they drive).
    pub track: Track,
}

/// A named animation: a set of channels plus the precomputed total duration.
#[derive(Clone, Debug)]
pub struct SceneAnimation {
    /// The animation's name (synthesised as `animation{i}` when the asset left it unnamed).
    pub name: String,
    /// The channels composing this animation.
    pub channels: Vec<Channel>,
    /// The animation length in seconds (the latest keyframe time across all channels).
    pub duration: f32,
}

impl SceneAnimation {
    /// Recomputes [`duration`](Self::duration) as the maximum final keyframe time over all channels.
    pub fn compute_duration(&mut self) {
        self.duration = self
            .channels
            .iter()
            .filter_map(|c| c.times.last().copied())
            .fold(0.0_f32, f32::max);
    }

    /// Maps an arbitrary elapsed time onto the animation's `0..=duration` range: wrapping when
    /// `looping` (so playback repeats seamlessly) or clamping when not. A zero-length animation
    /// always samples at `0.0`.
    pub fn sample_time(&self, elapsed: f32, looping: bool) -> f32 {
        if self.duration <= 0.0 {
            return 0.0;
        }
        if looping {
            elapsed.rem_euclid(self.duration)
        } else {
            elapsed.clamp(0.0, self.duration)
        }
    }

    /// Samples every channel at time `t`, returning per-node local transforms: a copy of `base`
    /// with each animated node's driven component overwritten. `t` is used as given (callers wrap
    /// or clamp it with [`sample_time`](Self::sample_time) first).
    pub fn evaluate(&self, t: f32, base: &[NodeTransform]) -> Vec<NodeTransform> {
        let mut out = base.to_vec();
        self.apply_into(t, &mut out);
        out
    }

    /// Like [`evaluate`](Self::evaluate) but writes into an existing buffer (which must already
    /// hold the base pose), avoiding an allocation on the playback hot path.
    pub fn apply_into(&self, t: f32, out: &mut [NodeTransform]) {
        for ch in &self.channels {
            if ch.node >= out.len() || ch.times.is_empty() {
                continue;
            }
            let (i0, i1, frac) = sample_segment(&ch.times, t);
            let nt = &mut out[ch.node];
            match &ch.track {
                Track::Translation(v) => {
                    if let Some(value) = interp_vec3(v, i0, i1, frac, ch.interpolation) {
                        nt.translation = value;
                    }
                }
                Track::Scale(v) => {
                    if let Some(value) = interp_vec3(v, i0, i1, frac, ch.interpolation) {
                        nt.scale = value;
                    }
                }
                Track::Rotation(q) => {
                    if let Some(value) = interp_quat(q, i0, i1, frac, ch.interpolation) {
                        nt.rotation = value;
                    }
                }
            }
        }
    }
}

/// The un-baked scene graph, local-space meshes, and animations loaded from a glTF asset.
///
/// Produced by [`load_gltf_animated`](crate::load_gltf_animated). Call
/// [`pose_into`](Self::pose_into) to bake a given animation time into a renderable [`Scene`].
#[derive(Clone, Debug, Default)]
pub struct AnimatedScene {
    /// A source name (e.g. the file stem).
    pub name: String,
    /// All nodes, indexed as in the source so animation channels address them directly.
    pub nodes: Vec<SceneNode>,
    /// Indices of the root nodes (the default scene's top-level nodes).
    pub roots: Vec<usize>,
    /// The drawable mesh instances, each tagged with the node that transforms it.
    pub instances: Vec<MeshInstance>,
    /// The asset's animations.
    pub animations: Vec<SceneAnimation>,
    /// Whether any node is skinned. Skinning is out of scope: the rigid node motion still plays,
    /// but per-vertex joint deformation is not applied.
    pub has_skinning: bool,
}

impl AnimatedScene {
    /// The base (rest-pose) local transform of every node, in node order.
    pub fn base_transforms(&self) -> Vec<NodeTransform> {
        self.nodes.iter().map(|n| n.transform).collect()
    }

    /// Composes per-node world matrices from per-node local transforms by walking the hierarchy
    /// from the roots. `locals` must be indexed by node (length `nodes.len()`).
    pub fn world_matrices(&self, locals: &[NodeTransform]) -> Vec<Mat4> {
        let mut world = vec![Mat4::IDENTITY; self.nodes.len()];
        for &root in &self.roots {
            self.compose(root, Mat4::IDENTITY, locals, &mut world);
        }
        world
    }

    /// Recursively composes `parent · local` into `world[node]` and descends into children.
    fn compose(&self, node: usize, parent: Mat4, locals: &[NodeTransform], world: &mut [Mat4]) {
        if node >= self.nodes.len() || node >= locals.len() {
            return;
        }
        let m = parent * locals[node].matrix();
        world[node] = m;
        // Clone the child list index-by-index to avoid holding a borrow of `self.nodes` across the
        // recursive call that writes `world`.
        let child_count = self.nodes[node].children.len();
        for i in 0..child_count {
            let child = self.nodes[node].children[i];
            self.compose(child, m, locals, world);
        }
    }

    /// Bakes the pose at animation `anim` (or the rest pose when `None`) at time `t` into `out`,
    /// reusing `out`'s mesh/vertex buffers so a playback loop does not reallocate each frame.
    ///
    /// `t` is used as given; wrap or clamp it with [`SceneAnimation::sample_time`] first for
    /// looping/clamped playback.
    pub fn pose_into(&self, anim: Option<usize>, t: f32, out: &mut Scene) {
        let mut locals = self.base_transforms();
        if let Some(i) = anim {
            if let Some(animation) = self.animations.get(i) {
                animation.apply_into(t, &mut locals);
            }
        }
        let world = self.world_matrices(&locals);

        out.name.clone_from(&self.name);
        // Resize the mesh list in place, reusing the existing `Mesh` allocations.
        if out.meshes.len() != self.instances.len() {
            out.meshes.resize_with(self.instances.len(), Mesh::default);
        }
        for (dst, inst) in out.meshes.iter_mut().zip(self.instances.iter()) {
            let w = world.get(inst.node).copied().unwrap_or(Mat4::IDENTITY);
            let normal_matrix = Mat3::from_mat4(w).inverse().transpose();
            dst.vertices.clear();
            dst.vertices.extend(inst.mesh.vertices.iter().map(|v| {
                let position = w.transform_point3(v.position);
                let normal = (normal_matrix * v.normal).normalize_or_zero();
                Vertex::new(position, normal)
            }));
            if dst.indices != inst.mesh.indices {
                dst.indices.clone_from(&inst.mesh.indices);
            }
        }
    }

    /// Convenience wrapper around [`pose_into`](Self::pose_into) that allocates a fresh [`Scene`].
    pub fn pose(&self, anim: Option<usize>, t: f32) -> Scene {
        let mut scene = Scene::default();
        self.pose_into(anim, t, &mut scene);
        scene
    }
}

/// Finds the keyframe segment bracketing time `t` in the non-decreasing `times`, returning
/// `(i0, i1, frac)` where `frac ∈ [0, 1]` is the position of `t` within `[times[i0], times[i1]]`.
/// Times before the first / after the last keyframe clamp to the endpoints (`i0 == i1`, `frac 0`).
fn sample_segment(times: &[f32], t: f32) -> (usize, usize, f32) {
    let n = times.len();
    if n == 0 {
        return (0, 0, 0.0);
    }
    if t <= times[0] {
        return (0, 0, 0.0);
    }
    if t >= times[n - 1] {
        return (n - 1, n - 1, 0.0);
    }
    // Advance to the last keyframe whose time is <= t.
    let mut i = 0;
    while i + 1 < n && times[i + 1] <= t {
        i += 1;
    }
    let span = times[i + 1] - times[i];
    let frac = if span > 0.0 {
        (t - times[i]) / span
    } else {
        0.0
    };
    (i, i + 1, frac)
}

/// Interpolates a Vec3 track between keyframes `i0` and `i1`, honouring the interpolation mode.
fn interp_vec3(
    values: &[Vec3],
    i0: usize,
    i1: usize,
    frac: f32,
    interp: Interpolation,
) -> Option<Vec3> {
    let a = *values.get(i0)?;
    match interp {
        Interpolation::Step => Some(a),
        Interpolation::Linear => {
            let b = values.get(i1).copied().unwrap_or(a);
            Some(a.lerp(b, frac))
        }
    }
}

/// Interpolates a rotation track between keyframes `i0` and `i1`, honouring the interpolation mode
/// (spherical-linear for [`Interpolation::Linear`]).
fn interp_quat(
    values: &[Quat],
    i0: usize,
    i1: usize,
    frac: f32,
    interp: Interpolation,
) -> Option<Quat> {
    let a = *values.get(i0)?;
    match interp {
        Interpolation::Step => Some(a.normalize()),
        Interpolation::Linear => {
            let b = values.get(i1).copied().unwrap_or(a);
            Some(a.normalize().slerp(b.normalize(), frac))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1e-5;

    fn tri_mesh() -> Mesh {
        Mesh::new(
            vec![
                Vertex::from_position(Vec3::new(0.0, 0.0, 0.0)),
                Vertex::from_position(Vec3::new(1.0, 0.0, 0.0)),
                Vertex::from_position(Vec3::new(0.0, 1.0, 0.0)),
            ],
            vec![0, 1, 2],
        )
    }

    fn translation_anim(node: usize, interp: Interpolation) -> SceneAnimation {
        let mut a = SceneAnimation {
            name: "move".into(),
            channels: vec![Channel {
                node,
                interpolation: interp,
                times: vec![0.0, 1.0, 2.0],
                track: Track::Translation(vec![
                    Vec3::ZERO,
                    Vec3::new(0.0, 10.0, 0.0),
                    Vec3::new(0.0, 0.0, 0.0),
                ]),
            }],
            duration: 0.0,
        };
        a.compute_duration();
        a
    }

    #[test]
    fn duration_is_the_latest_keyframe_time() {
        let a = translation_anim(0, Interpolation::Linear);
        assert!((a.duration - 2.0).abs() < EPS);
    }

    #[test]
    fn linear_interpolates_at_boundaries_and_midpoints() {
        let a = translation_anim(0, Interpolation::Linear);
        let base = vec![NodeTransform::IDENTITY];

        // Exact keyframes.
        assert!((a.evaluate(0.0, &base)[0].translation - Vec3::ZERO).length() < EPS);
        assert!((a.evaluate(1.0, &base)[0].translation - Vec3::new(0.0, 10.0, 0.0)).length() < EPS);
        // Midpoint of the first segment: halfway to (0,10,0).
        assert!((a.evaluate(0.5, &base)[0].translation - Vec3::new(0.0, 5.0, 0.0)).length() < EPS);
        // Midpoint of the second segment: halfway back from (0,10,0) to origin.
        assert!((a.evaluate(1.5, &base)[0].translation - Vec3::new(0.0, 5.0, 0.0)).length() < EPS);
    }

    #[test]
    fn step_holds_the_earlier_keyframe() {
        let a = translation_anim(0, Interpolation::Step);
        let base = vec![NodeTransform::IDENTITY];
        // Anywhere inside [0,1) holds keyframe 0 (origin); at/after 1.0 it jumps to (0,10,0).
        assert!((a.evaluate(0.99, &base)[0].translation - Vec3::ZERO).length() < EPS);
        assert!((a.evaluate(1.0, &base)[0].translation - Vec3::new(0.0, 10.0, 0.0)).length() < EPS);
        assert!(
            (a.evaluate(1.99, &base)[0].translation - Vec3::new(0.0, 10.0, 0.0)).length() < EPS
        );
    }

    #[test]
    fn time_before_and_after_clamps_to_endpoints() {
        let a = translation_anim(0, Interpolation::Linear);
        let base = vec![NodeTransform::IDENTITY];
        assert!((a.evaluate(-5.0, &base)[0].translation - Vec3::ZERO).length() < EPS);
        // Past the end clamps to the final keyframe (origin again, here).
        assert!((a.evaluate(99.0, &base)[0].translation - Vec3::ZERO).length() < EPS);
    }

    #[test]
    fn sample_time_loops_and_clamps() {
        let a = translation_anim(0, Interpolation::Linear);
        // Looping wraps past the duration back to the start.
        assert!((a.sample_time(2.25, true) - 0.25).abs() < EPS);
        assert!((a.sample_time(4.0, true) - 0.0).abs() < EPS);
        // Non-looping clamps to the end.
        assert!((a.sample_time(99.0, false) - 2.0).abs() < EPS);
        assert!((a.sample_time(-1.0, false) - 0.0).abs() < EPS);
    }

    #[test]
    fn rotation_slerp_reaches_the_target() {
        let quarter = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let mut a = SceneAnimation {
            name: "spin".into(),
            channels: vec![Channel {
                node: 0,
                interpolation: Interpolation::Linear,
                times: vec![0.0, 1.0],
                track: Track::Rotation(vec![Quat::IDENTITY, quarter]),
            }],
            duration: 0.0,
        };
        a.compute_duration();
        let base = vec![NodeTransform::IDENTITY];
        let r = a.evaluate(1.0, &base)[0].rotation;
        // A +90° Z-rotation sends +X to +Y.
        let x = r * Vec3::X;
        assert!((x - Vec3::Y).length() < 1e-4, "quarter turn maps X->Y");
    }

    #[test]
    fn parent_motion_composes_onto_the_child() {
        // Two-node chain: node 0 (parent) moves +10 in x; node 1 (child) sits at +2 in x locally.
        // The child's world position must follow the parent.
        let mut scene = AnimatedScene {
            name: "chain".into(),
            nodes: vec![
                SceneNode {
                    name: Some("parent".into()),
                    transform: NodeTransform::IDENTITY,
                    children: vec![1],
                },
                SceneNode {
                    name: Some("child".into()),
                    transform: NodeTransform::new(
                        Vec3::new(2.0, 0.0, 0.0),
                        Quat::IDENTITY,
                        Vec3::ONE,
                    ),
                    children: vec![],
                },
            ],
            roots: vec![0],
            instances: vec![MeshInstance {
                node: 1,
                mesh: tri_mesh(),
                base_color: [1.0, 1.0, 1.0, 1.0],
            }],
            animations: vec![],
            has_skinning: false,
        };
        // Animate the parent (node 0) to x = +10 at t = 1.
        let mut anim = SceneAnimation {
            name: "slide".into(),
            channels: vec![Channel {
                node: 0,
                interpolation: Interpolation::Linear,
                times: vec![0.0, 1.0],
                track: Track::Translation(vec![Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0)]),
            }],
            duration: 0.0,
        };
        anim.compute_duration();
        scene.animations.push(anim);

        // Rest pose: child's first vertex (local origin) sits at world x = 2.
        let rest = scene.pose(None, 0.0);
        assert!((rest.meshes[0].vertices[0].position - Vec3::new(2.0, 0.0, 0.0)).length() < EPS);

        // At t = 1 the parent has moved +10, so the child's vertex is at x = 12.
        let moved = scene.pose(Some(0), 1.0);
        assert!(
            (moved.meshes[0].vertices[0].position - Vec3::new(12.0, 0.0, 0.0)).length() < EPS,
            "child follows the parent's world motion"
        );

        // Midpoint: parent at +5 -> child vertex at x = 7.
        let mid = scene.pose(Some(0), 0.5);
        assert!((mid.meshes[0].vertices[0].position - Vec3::new(7.0, 0.0, 0.0)).length() < EPS);
    }

    #[test]
    fn pose_into_reuses_mesh_buffers() {
        let scene = AnimatedScene {
            name: "one".into(),
            nodes: vec![SceneNode {
                name: None,
                transform: NodeTransform::IDENTITY,
                children: vec![],
            }],
            roots: vec![0],
            instances: vec![MeshInstance {
                node: 0,
                mesh: tri_mesh(),
                base_color: [1.0, 1.0, 1.0, 1.0],
            }],
            animations: vec![],
            has_skinning: false,
        };
        let mut out = Scene::default();
        scene.pose_into(None, 0.0, &mut out);
        let ptr_before = out.meshes[0].vertices.as_ptr();
        // A second pose into the same buffer must not grow/reallocate the vertex storage.
        scene.pose_into(None, 0.0, &mut out);
        let ptr_after = out.meshes[0].vertices.as_ptr();
        assert_eq!(
            ptr_before, ptr_after,
            "vertex buffer is reused across poses"
        );
        assert_eq!(out.meshes.len(), 1);
        assert_eq!(out.meshes[0].vertices.len(), 3);
    }
}
