//! Rigid node-transform animation for glTF scenes.
//!
//! glTF animations drive node **TRS** (translation / rotation / scale) over time. This module
//! holds the pure, renderer-free data model and evaluator for that:
//!
//! - [`NodeTransform`] is a node's local transform as a decomposed T/R/S triple.
//! - [`SceneNode`] / [`MeshInstance`] describe a node hierarchy with *un-baked* local meshes, so a
//!   scene can be re-posed every frame (unlike the baked [`Scene`] produced by
//!   [`load_gltf`](crate::load_gltf)).
//! - [`SceneAnimation`] is one named animation — a set of [`AnimChannel`]s that sample keyframes
//!   (`LINEAR` and `STEP`) to override node transforms at a time `t`.
//! - [`AnimatedScene`] ties the hierarchy, instances, and animations together and can
//!   [`evaluate`](AnimatedScene::evaluate) per-node local transforms or
//!   [`bake_static`](AnimatedScene::bake_static) the default pose.
//! - [`SceneAnimator`] is the stateful, buffer-reusing helper the viewer drives: it poses the
//!   hierarchy at a time `t` and rewrites a reused [`Scene`]'s world-space vertices in place.
//!
//! **Skeletal skinning is out of scope.** Skins are detected ([`SceneAnimation::skinned`]) so the
//! caller can surface a note, but per-vertex joint blending is not applied — a skinned animation
//! still plays its *node* motion without crashing.

use glam::{Mat3, Mat4, Quat, Vec3};
use rgfx_core::{Mesh, Scene, Vertex};

/// A node's local transform, decomposed into translation / rotation / scale.
///
/// This is the form glTF stores node transforms and animates, so keeping it decomposed lets each
/// animation channel override exactly one component without disturbing the others.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NodeTransform {
    /// Translation.
    pub translation: Vec3,
    /// Rotation.
    pub rotation: Quat,
    /// Non-uniform scale.
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

    /// Builds a transform from a glTF-style decomposed triple (`translation`, `rotation` as
    /// `[x, y, z, w]`, `scale`).
    pub fn from_trs(translation: [f32; 3], rotation: [f32; 4], scale: [f32; 3]) -> Self {
        Self {
            translation: Vec3::from_array(translation),
            rotation: Quat::from_array(rotation),
            scale: Vec3::from_array(scale),
        }
    }

    /// The 4×4 affine matrix for this transform (`T * R * S`).
    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }
}

/// How a channel's keyframe values are blended between samples.
///
/// Only the two step/linear interpolation modes are supported; `CUBICSPLINE` is out of scope and
/// is mapped to [`Interpolation::Linear`] on the value points by the loader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interpolation {
    /// Linearly interpolate (lerp for vectors, slerp for rotations) between the bracketing samples.
    Linear,
    /// Hold the earlier sample's value until the next keyframe (no blending).
    Step,
}

/// The keyframe output values of a channel, tagged by which TRS component they drive.
#[derive(Clone, Debug, PartialEq)]
pub enum ChannelSamples {
    /// Translation keyframes.
    Translation(Vec<Vec3>),
    /// Rotation keyframes.
    Rotation(Vec<Quat>),
    /// Scale keyframes.
    Scale(Vec<Vec3>),
}

/// One animation channel: keyframe times plus the values they drive on a single node's transform.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimChannel {
    /// Index into [`AnimatedScene::nodes`] of the node this channel animates.
    pub node: usize,
    /// How values are blended between keyframes.
    pub interpolation: Interpolation,
    /// Keyframe times, in seconds, ascending. Parallel to the values in [`samples`](Self::samples).
    pub times: Vec<f32>,
    /// The per-keyframe output values and the TRS component they target.
    pub samples: ChannelSamples,
}

/// Locates the keyframe segment containing `t`, returning `(i0, i1, frac)` where `frac` in
/// `0.0..=1.0` is the position of `t` between `times[i0]` and `times[i1]`.
///
/// Clamps to the endpoints: before the first / after the last keyframe it returns that endpoint
/// doubled with `frac = 0.0`. Assumes `times` is non-empty and ascending.
fn locate(times: &[f32], t: f32) -> (usize, usize, f32) {
    let n = times.len();
    // invariant: callers guarantee a non-empty time list.
    if n == 1 || t <= times[0] {
        return (0, 0, 0.0);
    }
    if t >= times[n - 1] {
        return (n - 1, n - 1, 0.0);
    }
    // First index whose time is strictly greater than `t`; `t` sits in segment [i1-1, i1].
    let i1 = times.partition_point(|&x| x <= t).clamp(1, n - 1);
    let i0 = i1 - 1;
    let span = times[i1] - times[i0];
    let frac = if span > 0.0 {
        (t - times[i0]) / span
    } else {
        0.0
    };
    (i0, i1, frac)
}

impl AnimChannel {
    /// Samples this channel at time `t` and writes the result into the matching component of
    /// `out`. A no-op for an empty channel.
    pub fn apply(&self, t: f32, out: &mut NodeTransform) {
        if self.times.is_empty() {
            return;
        }
        let (i0, i1, frac) = locate(&self.times, t);
        match &self.samples {
            ChannelSamples::Translation(v) => {
                out.translation = lerp_vec3(v, i0, i1, frac, self.interpolation);
            }
            ChannelSamples::Scale(v) => {
                out.scale = lerp_vec3(v, i0, i1, frac, self.interpolation);
            }
            ChannelSamples::Rotation(v) => {
                out.rotation = slerp_quat(v, i0, i1, frac, self.interpolation);
            }
        }
    }
}

/// Samples a vec3 track: `STEP` holds `v[i0]`, `LINEAR` lerps toward `v[i1]` by `frac`.
fn lerp_vec3(v: &[Vec3], i0: usize, i1: usize, frac: f32, interp: Interpolation) -> Vec3 {
    match interp {
        Interpolation::Step => v[i0],
        Interpolation::Linear => v[i0].lerp(v[i1], frac),
    }
}

/// Samples a rotation track: `STEP` holds `v[i0]`, `LINEAR` slerps toward `v[i1]` by `frac`.
fn slerp_quat(v: &[Quat], i0: usize, i1: usize, frac: f32, interp: Interpolation) -> Quat {
    match interp {
        Interpolation::Step => v[i0],
        Interpolation::Linear => v[i0].slerp(v[i1], frac),
    }
}

/// One named glTF animation: a collection of channels plus its total duration.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneAnimation {
    /// The animation's name (synthesized as `animation {i}` when the asset leaves it unnamed).
    pub name: String,
    /// The animation length in seconds (the largest keyframe time across its channels).
    pub duration: f32,
    /// Whether this animation targets skinned nodes (joints). Skinning itself is not applied; this
    /// flags that only rigid node motion is played.
    pub skinned: bool,
    /// The per-node TRS channels.
    pub channels: Vec<AnimChannel>,
}

impl SceneAnimation {
    /// Wraps a playback time into `0.0..duration` for looping. Returns `0.0` for a zero-length
    /// animation so evaluation stays at the first pose.
    pub fn wrap_time(&self, t: f32) -> f32 {
        if self.duration <= 0.0 {
            0.0
        } else {
            t.rem_euclid(self.duration)
        }
    }
}

/// A node in the animated scene hierarchy, holding its default local transform and its links.
///
/// Nodes are stored in depth-first pre-order, so a node's index is always greater than its
/// parent's — world transforms compose correctly in a simple ascending pass.
#[derive(Clone, Debug)]
pub struct SceneNode {
    /// The node's name, if the asset provided one.
    pub name: Option<String>,
    /// The node's default (un-animated) local transform.
    pub local: NodeTransform,
    /// Index of this node's parent, or `None` for a root.
    pub parent: Option<usize>,
    /// Indices of this node's children.
    pub children: Vec<usize>,
}

/// A mesh attached to a node, with its vertices in node-*local* space (un-baked) so it can be
/// re-posed every frame.
#[derive(Clone, Debug)]
pub struct MeshInstance {
    /// Index into [`AnimatedScene::nodes`] of the node this mesh rides on.
    pub node: usize,
    /// The local-space triangle mesh.
    pub mesh: Mesh,
    /// The primitive's base color RGBA factor (parallel to the baked [`Scene`] meshes).
    pub base_color: [f32; 4],
}

/// An animated glTF scene: a node hierarchy, its un-baked mesh instances, and its animations.
///
/// Produced by [`load_gltf_animated`](crate::load_gltf_animated). Call
/// [`bake_static`](Self::bake_static) for the default pose, or drive a [`SceneAnimator`] for
/// time-based playback.
#[derive(Clone, Debug)]
pub struct AnimatedScene {
    /// A human-readable name (e.g. the file stem).
    pub name: String,
    /// The node hierarchy, in depth-first pre-order (parents precede their children).
    pub nodes: Vec<SceneNode>,
    /// The root node indices.
    pub roots: Vec<usize>,
    /// The mesh instances, each attached to a node by index.
    pub instances: Vec<MeshInstance>,
    /// The animations declared by the asset.
    pub animations: Vec<SceneAnimation>,
}

impl AnimatedScene {
    /// Whether the scene carries at least one animation.
    pub fn has_animations(&self) -> bool {
        !self.animations.is_empty()
    }

    /// Evaluates per-node *local* transforms for animation `anim` at time `t`.
    ///
    /// Starts from every node's default local transform, then overrides the components driven by
    /// the animation's channels. `t` is wrapped into the animation's duration (looping). An
    /// out-of-range `anim` index yields the default pose.
    pub fn evaluate(&self, anim: usize, t: f32) -> Vec<NodeTransform> {
        let mut locals: Vec<NodeTransform> = self.nodes.iter().map(|n| n.local).collect();
        if let Some(animation) = self.animations.get(anim) {
            let t = animation.wrap_time(t);
            for channel in &animation.channels {
                if let Some(node) = locals.get_mut(channel.node) {
                    channel.apply(t, node);
                }
            }
        }
        locals
    }

    /// Bakes the default (un-animated) pose into a flat [`Scene`] whose vertices carry world-space
    /// positions — matching [`load_gltf`](crate::load_gltf)'s output.
    pub fn bake_static(&self) -> Scene {
        let mut animator = SceneAnimator::new(self.clone());
        let mut scene = Scene::new(self.name.clone(), Vec::new());
        animator.pose_into(None, 0.0, &mut scene);
        scene
    }

    /// The names, durations, and skinning flags of the animations, for stats/reporting.
    pub fn animation_infos(&self) -> Vec<AnimationInfo> {
        self.animations
            .iter()
            .map(|a| AnimationInfo {
                name: a.name.clone(),
                duration: a.duration,
                skinned: a.skinned,
            })
            .collect()
    }
}

/// A lightweight summary of one animation, for stats/reporting.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationInfo {
    /// The animation name.
    pub name: String,
    /// The animation duration in seconds.
    pub duration: f32,
    /// Whether the animation targets skinned nodes (node motion only is played).
    pub skinned: bool,
}

/// A stateful, buffer-reusing poser that rewrites a [`Scene`]'s world-space vertices for an
/// animation at a given time, without reallocating per frame.
///
/// Build one from an [`AnimatedScene`] and call [`pose_into`](Self::pose_into) each tick with the
/// selected animation and time; the internal local-transform and world-matrix buffers are reused
/// across calls, and the output scene's mesh/vertex/index `Vec`s are rewritten in place.
#[derive(Clone, Debug)]
pub struct SceneAnimator {
    scene: AnimatedScene,
    /// Scratch: per-node posed local transforms.
    locals: Vec<NodeTransform>,
    /// Scratch: per-node world matrices (composed in ascending index order).
    world: Vec<Mat4>,
}

impl SceneAnimator {
    /// Builds an animator for `scene`, pre-allocating its scratch buffers.
    pub fn new(scene: AnimatedScene) -> Self {
        let n = scene.nodes.len();
        Self {
            locals: vec![NodeTransform::IDENTITY; n],
            world: vec![Mat4::IDENTITY; n],
            scene,
        }
    }

    /// The underlying animated scene.
    pub fn scene(&self) -> &AnimatedScene {
        &self.scene
    }

    /// The scene's animations.
    pub fn animations(&self) -> &[SceneAnimation] {
        &self.scene.animations
    }

    /// Poses the hierarchy for animation `anim` (or the default pose when `None`) at time `t` and
    /// rewrites `out` so each mesh carries world-space vertex positions and normals.
    ///
    /// Reuses `out`'s mesh `Vec` and each mesh's vertex/index storage where the shapes already
    /// match, so steady-state playback does not allocate. `t` is wrapped into the chosen
    /// animation's duration (looping).
    pub fn pose_into(&mut self, anim: Option<usize>, t: f32, out: &mut Scene) {
        // 1. Start from defaults, then overlay the selected animation's channels.
        for (dst, node) in self.locals.iter_mut().zip(&self.scene.nodes) {
            *dst = node.local;
        }
        if let Some(idx) = anim {
            if let Some(animation) = self.scene.animations.get(idx) {
                let tt = animation.wrap_time(t);
                for channel in &animation.channels {
                    if let Some(local) = self.locals.get_mut(channel.node) {
                        channel.apply(tt, local);
                    }
                }
            }
        }

        // 2. Compose world matrices. Nodes are in pre-order, so each parent precedes its children.
        for i in 0..self.scene.nodes.len() {
            let local_m = self.locals[i].matrix();
            self.world[i] = match self.scene.nodes[i].parent {
                Some(p) => self.world[p] * local_m,
                None => local_m,
            };
        }

        // 3. Rewrite the output scene's meshes in place (reusing buffers where possible).
        out.name.clone_from(&self.scene.name);
        out.meshes
            .resize_with(self.scene.instances.len(), Mesh::default);
        for (inst, mesh) in self.scene.instances.iter().zip(out.meshes.iter_mut()) {
            let world = self.world[inst.node];
            let normal_matrix = Mat3::from_mat4(world).inverse().transpose();

            // Indices are invariant; copy into the reused buffer.
            mesh.indices.clone_from(&inst.mesh.indices);

            let src = &inst.mesh.vertices;
            mesh.vertices
                .resize(src.len(), Vertex::from_position(Vec3::ZERO));
            for (s, d) in src.iter().zip(mesh.vertices.iter_mut()) {
                d.position = world.transform_point3(s.position);
                d.normal = (normal_matrix * s.normal).normalize_or_zero();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1e-5;

    fn translation_channel(node: usize, interp: Interpolation) -> AnimChannel {
        AnimChannel {
            node,
            interpolation: interp,
            times: vec![0.0, 1.0, 2.0],
            samples: ChannelSamples::Translation(vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(10.0, 0.0, 0.0),
                Vec3::new(10.0, 20.0, 0.0),
            ]),
        }
    }

    #[test]
    fn linear_interpolation_at_boundaries_and_midpoints() {
        let ch = translation_channel(0, Interpolation::Linear);
        let mut tr = NodeTransform::IDENTITY;

        ch.apply(0.0, &mut tr);
        assert!(
            (tr.translation - Vec3::ZERO).length() < EPS,
            "t=0 first key"
        );

        ch.apply(0.5, &mut tr);
        assert!(
            (tr.translation - Vec3::new(5.0, 0.0, 0.0)).length() < EPS,
            "midpoint lerps halfway"
        );

        ch.apply(1.0, &mut tr);
        assert!(
            (tr.translation - Vec3::new(10.0, 0.0, 0.0)).length() < EPS,
            "exact keyframe"
        );

        ch.apply(1.5, &mut tr);
        assert!(
            (tr.translation - Vec3::new(10.0, 10.0, 0.0)).length() < EPS,
            "second segment lerps"
        );

        ch.apply(5.0, &mut tr);
        assert!(
            (tr.translation - Vec3::new(10.0, 20.0, 0.0)).length() < EPS,
            "past the end clamps to the last key"
        );
    }

    #[test]
    fn step_interpolation_holds_the_earlier_keyframe() {
        let ch = translation_channel(0, Interpolation::Step);
        let mut tr = NodeTransform::IDENTITY;

        ch.apply(0.4, &mut tr);
        assert!(
            (tr.translation - Vec3::ZERO).length() < EPS,
            "holds first key"
        );
        ch.apply(0.99, &mut tr);
        assert!(
            (tr.translation - Vec3::ZERO).length() < EPS,
            "still holds just before the next key"
        );
        ch.apply(1.0, &mut tr);
        assert!(
            (tr.translation - Vec3::new(10.0, 0.0, 0.0)).length() < EPS,
            "snaps at the next key"
        );
    }

    #[test]
    fn rotation_slerp_midpoint_is_half_the_angle() {
        let ch = AnimChannel {
            node: 0,
            interpolation: Interpolation::Linear,
            times: vec![0.0, 1.0],
            samples: ChannelSamples::Rotation(vec![
                Quat::IDENTITY,
                Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
            ]),
        };
        let mut tr = NodeTransform::IDENTITY;
        ch.apply(0.5, &mut tr);
        let half = Quat::from_rotation_z(std::f32::consts::FRAC_PI_4);
        assert!(
            tr.rotation.angle_between(half) < 1e-4,
            "slerp halves the turn"
        );
    }

    #[test]
    fn wrap_time_loops_within_duration() {
        let anim = SceneAnimation {
            name: "a".into(),
            duration: 2.0,
            skinned: false,
            channels: vec![],
        };
        assert!((anim.wrap_time(0.5) - 0.5).abs() < EPS);
        assert!(
            (anim.wrap_time(2.5) - 0.5).abs() < EPS,
            "wraps past the end"
        );
        assert!(
            (anim.wrap_time(4.0) - 0.0).abs() < EPS,
            "exact multiple wraps to 0"
        );
        // A zero-length animation stays pinned at 0.
        let still = SceneAnimation {
            name: "z".into(),
            duration: 0.0,
            skinned: false,
            channels: vec![],
        };
        assert_eq!(still.wrap_time(3.0), 0.0);
    }

    /// A two-node hierarchy: a parent that translates and a child offset from it. The child's
    /// world position must track the parent's motion (parent → child composition).
    fn parent_child_scene() -> AnimatedScene {
        let parent = SceneNode {
            name: Some("parent".into()),
            local: NodeTransform::IDENTITY,
            parent: None,
            children: vec![1],
        };
        // Child sits at (0,1,0) relative to the parent and carries a point mesh at its origin.
        let child = SceneNode {
            name: Some("child".into()),
            local: NodeTransform {
                translation: Vec3::new(0.0, 1.0, 0.0),
                ..NodeTransform::IDENTITY
            },
            parent: Some(0),
            children: vec![],
        };
        let instance = MeshInstance {
            node: 1,
            mesh: Mesh::new(vec![Vertex::from_position(Vec3::ZERO)], vec![]),
            base_color: [1.0, 1.0, 1.0, 1.0],
        };
        let anim = SceneAnimation {
            name: "move-parent".into(),
            duration: 1.0,
            skinned: false,
            channels: vec![AnimChannel {
                node: 0,
                interpolation: Interpolation::Linear,
                times: vec![0.0, 1.0],
                samples: ChannelSamples::Translation(vec![Vec3::ZERO, Vec3::new(4.0, 0.0, 0.0)]),
            }],
        };
        AnimatedScene {
            name: "pc".into(),
            nodes: vec![parent, child],
            roots: vec![0],
            instances: vec![instance],
            animations: vec![anim],
        }
    }

    #[test]
    fn parent_motion_drags_the_child_in_world_space() {
        let scene = parent_child_scene();
        let mut animator = SceneAnimator::new(scene);
        let mut out = Scene::default();

        // t=0: child at its static offset (0,1,0).
        animator.pose_into(Some(0), 0.0, &mut out);
        let p0 = out.meshes[0].vertices[0].position;
        assert!(
            (p0 - Vec3::new(0.0, 1.0, 0.0)).length() < EPS,
            "static offset"
        );

        // t=0.5: parent has moved +2 in x, so the child rides along to (2,1,0).
        animator.pose_into(Some(0), 0.5, &mut out);
        let p1 = out.meshes[0].vertices[0].position;
        assert!(
            (p1 - Vec3::new(2.0, 1.0, 0.0)).length() < EPS,
            "child tracks the parent's translation"
        );
    }

    #[test]
    fn pose_into_reuses_buffers_without_reallocating() {
        let scene = parent_child_scene();
        let mut animator = SceneAnimator::new(scene);
        let mut out = Scene::default();
        animator.pose_into(Some(0), 0.0, &mut out);
        let cap = out.meshes[0].vertices.capacity();
        let ptr = out.meshes[0].vertices.as_ptr();
        // Re-posing must not grow or move the vertex storage (same count each frame).
        for i in 1..20 {
            animator.pose_into(Some(0), i as f32 * 0.05, &mut out);
        }
        assert_eq!(out.meshes[0].vertices.capacity(), cap);
        assert_eq!(
            out.meshes[0].vertices.as_ptr(),
            ptr,
            "buffer reused in place"
        );
    }

    #[test]
    fn evaluate_out_of_range_animation_is_the_default_pose() {
        let scene = parent_child_scene();
        let locals = scene.evaluate(99, 0.5);
        assert_eq!(locals[0], NodeTransform::IDENTITY, "parent stays default");
        assert_eq!(
            locals[1].translation,
            Vec3::new(0.0, 1.0, 0.0),
            "child keeps its static offset"
        );
    }
}
