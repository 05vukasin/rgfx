//! An orbit camera controller layered on top of [`rgfx_core::Camera`].
//!
//! The controller stores the orbit *state* — a target point, a free view *orientation* (a unit
//! [`glam::Quat`]) around it, and a distance — and offers the interactions an interactive 3D
//! viewer needs: orbit (tumble), roll, zoom (dolly), pan, reset, and automatic framing. It never
//! renders; call [`OrbitController::sync`] to write the resulting position/target/up into a
//! camera.
//!
//! # Coordinate convention
//!
//! The controller matches [`rgfx_core::Camera`]'s right-handed system. With the identity
//! orientation the camera sits on the target's `+Z` axis looking toward `-Z` with up `+Y`,
//! exactly like a freshly constructed [`rgfx_core::Camera`].
//!
//! # Arcball (free 360° rotation)
//!
//! Orientation is a quaternion rather than clamped Euler yaw/pitch, so the model tumbles freely in
//! every direction with **no pole clamp and no gimbal lock**: [`OrbitController::orbit`] composes
//! incremental rotations about the camera's *current* right and up axes into the quaternion, so
//! the view can roll right over the poles and keep going. Because the camera up is derived from
//! the same orientation it is always orthogonal to the view axis, so the view basis never
//! degenerates.

use glam::{Mat3, Quat, Vec3};
use rgfx_core::{BoundingSphere, Camera, Projection};

use crate::framing::{
    DEFAULT_FRAMING_MARGIN, orthographic_fit_half_height, perspective_fit_distance,
};

/// The smallest orbit distance, keeping the camera from collapsing onto its target.
const MIN_DISTANCE: f32 = 1e-3;

/// The base view axis: the direction from the target toward the camera at the identity
/// orientation (the camera sits on the target's `+Z` axis).
const VIEW_AXIS: Vec3 = Vec3::Z;

/// The base up axis at the identity orientation.
const UP_AXIS: Vec3 = Vec3::Y;

/// Builds the view orientation that reproduces the classic spherical `yaw`/`pitch` angles: the
/// camera direction becomes `(cosθ·sinψ, sinθ, cosθ·cosψ)` for yaw `ψ` and pitch `θ`, with up
/// `+Y` when both are zero. Used by [`OrbitController::set_view`] and [`OrbitController::new`] so a
/// pleasant default 3/4 view can still be expressed in angles.
fn orientation_from_yaw_pitch(yaw: f32, pitch: f32) -> Quat {
    (Quat::from_rotation_y(yaw) * Quat::from_rotation_x(-pitch)).normalize()
}

/// A snapshot of the orbit parameters, used to implement [`OrbitController::reset`].
#[derive(Clone, Copy, Debug, PartialEq)]
struct OrbitState {
    target: Vec3,
    orientation: Quat,
    distance: f32,
    roll: f32,
}

/// An orbit camera controller operating on an [`rgfx_core::Camera`].
///
/// Construct one from an explicit target and distance, or derive it from an existing camera
/// with [`OrbitController::from_camera`]. Mutating methods change the internal state; call
/// [`OrbitController::sync`] to push the state into a camera before rendering.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrbitController {
    target: Vec3,
    /// The free view orientation: a unit quaternion mapping the base frame (right `+X`, up `+Y`,
    /// view `+Z`) into world space. Replaces clamped Euler yaw/pitch to allow free tumbling.
    orientation: Quat,
    distance: f32,
    /// Roll about the view axis, in radians. `0` keeps `up` upright. Kept separate from the
    /// orientation so it can be reported and reset independently; applied about the current view
    /// axis in [`OrbitController::sync`].
    roll: f32,
    home: OrbitState,
}

impl OrbitController {
    /// Creates a controller orbiting `target` at `distance`, starting on the target's `+Z`
    /// axis (identity orientation) with world up `+Y`.
    pub fn new(target: Vec3, distance: f32) -> Self {
        let distance = distance.max(MIN_DISTANCE);
        let orientation = Quat::IDENTITY;
        let home = OrbitState {
            target,
            orientation,
            distance,
            roll: 0.0,
        };
        Self {
            target,
            orientation,
            distance,
            roll: 0.0,
            home,
        }
    }

    /// Derives orbit parameters (target, orientation, distance) from an existing camera so the
    /// controller can take over an already-positioned view without a visible jump. The camera's
    /// up direction is preserved as closely as the orthonormal basis allows.
    pub fn from_camera(camera: &Camera) -> Self {
        let offset = camera.position - camera.target;
        let distance = offset.length().max(MIN_DISTANCE);
        let orientation = orientation_from_view(offset, camera.up);
        let home = OrbitState {
            target: camera.target,
            orientation,
            distance,
            roll: 0.0,
        };
        Self {
            target: camera.target,
            orientation,
            distance,
            roll: 0.0,
            home,
        }
    }

    /// The point the camera orbits and looks at.
    pub fn target(&self) -> Vec3 {
        self.target
    }

    /// The current orbit distance from the target.
    pub fn distance(&self) -> f32 {
        self.distance
    }

    /// The free view orientation (base frame → world). Rarely needed directly; prefer
    /// [`OrbitController::direction`] / [`OrbitController::position`].
    pub fn orientation(&self) -> Quat {
        self.orientation
    }

    /// A best-effort yaw angle in radians (rotation about world up), derived from the current view
    /// direction. Exact only while the view has not tumbled past the poles; kept for callers and
    /// tests that reason in spherical angles.
    pub fn yaw(&self) -> f32 {
        let d = self.direction();
        d.x.atan2(d.z)
    }

    /// A best-effort pitch angle in radians (tilt above/below the horizon), derived from the
    /// current view direction and therefore reported in `-π/2..=π/2`. The controller itself is not
    /// clamped — the view may tumble freely past the poles.
    pub fn pitch(&self) -> f32 {
        self.direction().y.clamp(-1.0, 1.0).asin()
    }

    /// The unit direction from the target toward the camera.
    pub fn direction(&self) -> Vec3 {
        self.orientation * VIEW_AXIS
    }

    /// The world-space camera position implied by the current orbit state.
    pub fn position(&self) -> Vec3 {
        self.target + self.direction() * self.distance
    }

    /// Orbits (tumbles) the camera by `yaw_delta` and `pitch_delta` radians, composing incremental
    /// rotations about the camera's *current* up and right axes into the view orientation. There
    /// is **no pole clamp and no gimbal lock**: repeated pitch rolls right over the top and keeps
    /// going, and a full turn about either axis returns to the same orientation.
    pub fn orbit(&mut self, yaw_delta: f32, pitch_delta: f32) {
        if !(yaw_delta.is_finite() && pitch_delta.is_finite()) {
            return;
        }
        // Compose in the local (camera) frame: yaw about local up (+Y), pitch about local right
        // (+X). Right-multiplying keeps the rotation relative to what the viewer currently sees.
        let delta = Quat::from_rotation_y(yaw_delta) * Quat::from_rotation_x(-pitch_delta);
        self.orientation = (self.orientation * delta).normalize();
    }

    /// The roll angle in radians (rotation of the up vector about the view axis).
    pub fn roll_angle(&self) -> f32 {
        self.roll
    }

    /// Rolls the view about the forward (view) axis by `delta` radians — the third rotation axis,
    /// tilting the horizon without disturbing the tumble. Orbit rotates the camera around the
    /// model; roll spins the camera about the line of sight.
    pub fn roll(&mut self, delta: f32) {
        if delta.is_finite() {
            self.roll += delta;
        }
    }

    /// Sets the view orientation from spherical `yaw`/`pitch` angles (radians) and records it as
    /// the reset home, so `reset` returns here. Used to establish a pleasant default 3/4 view
    /// after [`OrbitController::auto_frame`]. Unlike the old Euler controller the angles are not
    /// clamped.
    pub fn set_view(&mut self, yaw: f32, pitch: f32) {
        self.orientation = orientation_from_yaw_pitch(yaw, pitch);
        self.home.orientation = self.orientation;
    }

    /// The camera up vector after applying roll about the view axis.
    fn effective_up(&self) -> Vec3 {
        let up = self.orientation * UP_AXIS;
        if self.roll.abs() < f32::EPSILON {
            return up;
        }
        // The view axis points target → camera; the camera looks along its negation.
        let forward = -self.direction();
        if forward.length_squared() < f32::EPSILON {
            return up;
        }
        (Quat::from_axis_angle(forward, self.roll) * up).normalize_or_zero()
    }

    /// Zooms (dollies) by scaling the orbit distance. `scale < 1` moves closer, `scale > 1`
    /// moves farther; the distance is clamped to a small positive minimum.
    pub fn zoom(&mut self, scale: f32) {
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        self.distance = (self.distance * scale).max(MIN_DISTANCE);
    }

    /// Dollies by adding `delta` world units to the orbit distance (positive = farther).
    pub fn dolly(&mut self, delta: f32) {
        if delta.is_finite() {
            self.distance = (self.distance + delta).max(MIN_DISTANCE);
        }
    }

    /// Pans the target within the camera's view plane by `dx` (right) and `dy` (up) world
    /// units. The camera position follows the target, so the view direction is preserved.
    pub fn pan(&mut self, dx: f32, dy: f32) {
        if !(dx.is_finite() && dy.is_finite()) {
            return;
        }
        // The orientation is orthonormal, so its X/Y columns are the camera right/up axes.
        let right = self.orientation * Vec3::X;
        let up = self.orientation * Vec3::Y;
        self.target += right * dx + up * dy;
    }

    /// Resets the orbit state to the values captured when the controller was created or last
    /// framed via [`OrbitController::auto_frame`] (or re-homed via [`OrbitController::set_view`]).
    pub fn reset(&mut self) {
        self.target = self.home.target;
        self.orientation = self.home.orientation;
        self.distance = self.home.distance;
        self.roll = self.home.roll;
    }

    /// Writes the current orbit state into `camera` (position, target, and up).
    pub fn sync(&self, camera: &mut Camera) {
        camera.position = self.position();
        camera.target = self.target;
        camera.up = self.effective_up();
    }

    /// Frames `sphere` inside a viewport of the given `aspect`, keeping the current view
    /// direction. Recenters the target on the sphere, chooses a distance (and orthographic
    /// half-height) so the sphere fits with [`DEFAULT_FRAMING_MARGIN`], sets sensible near/far
    /// clip planes, updates the camera aspect, and writes the result into `camera`.
    ///
    /// The framed state becomes the new [`OrbitController::reset`] home.
    pub fn auto_frame(&mut self, camera: &mut Camera, sphere: &BoundingSphere, aspect: f32) {
        self.auto_frame_with_margin(camera, sphere, aspect, DEFAULT_FRAMING_MARGIN);
    }

    /// [`OrbitController::auto_frame`] with an explicit `margin` factor.
    pub fn auto_frame_with_margin(
        &mut self,
        camera: &mut Camera,
        sphere: &BoundingSphere,
        aspect: f32,
        margin: f32,
    ) {
        let radius = sphere.radius.max(1e-4);
        camera.aspect = aspect;
        self.target = sphere.center;

        self.distance = match camera.projection {
            Projection::Perspective { fov_y } => {
                perspective_fit_distance(radius, fov_y, aspect, margin)
            }
            Projection::Orthographic { .. } => {
                let half_height = orthographic_fit_half_height(radius, aspect, margin);
                camera.projection = Projection::Orthographic { half_height };
                (radius * 2.0).max(MIN_DISTANCE)
            }
        };

        camera.near = (self.distance - radius)
            .max(radius * 1e-3)
            .max(MIN_DISTANCE);
        camera.far = (self.distance + radius).max(camera.near + MIN_DISTANCE);

        self.home = OrbitState {
            target: self.target,
            orientation: self.orientation,
            distance: self.distance,
            roll: self.roll,
        };

        self.sync(camera);
    }
}

/// Builds a view orientation (base frame → world) that reproduces a given target → camera `offset`
/// direction while preserving `up` as closely as the orthonormal basis allows. Falls back to a
/// sane axis when either input is degenerate.
fn orientation_from_view(offset: Vec3, up: Vec3) -> Quat {
    let z_axis = offset.normalize_or_zero();
    let z_axis = if z_axis.length_squared() > f32::EPSILON {
        z_axis
    } else {
        VIEW_AXIS
    };
    let up = if up.length_squared() > f32::EPSILON {
        up.normalize()
    } else {
        UP_AXIS
    };
    // Right-handed basis columns: X = Y × Z, then re-derive Y = Z × X so it is orthonormal even
    // when the supplied up is not perpendicular to the view axis.
    let mut x_axis = up.cross(z_axis);
    if x_axis.length_squared() < f32::EPSILON {
        // up is (anti)parallel to the view axis; pick any perpendicular axis.
        x_axis = z_axis.cross(Vec3::X);
        if x_axis.length_squared() < f32::EPSILON {
            x_axis = z_axis.cross(Vec3::Y);
        }
    }
    let x_axis = x_axis.normalize();
    let y_axis = z_axis.cross(x_axis).normalize();
    Quat::from_mat3(&Mat3::from_cols(x_axis, y_axis, z_axis)).normalize()
}
