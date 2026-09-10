//! A quaternion **arcball** camera controller layered on top of [`rgfx_core::Camera`].
//!
//! The controller stores the view *orientation* as a [`glam::Quat`] (plus a `target` point and a
//! `distance` from it) and offers the interactions an interactive 3D viewer needs: orbit (free
//! tumble in every direction), roll, zoom (dolly), pan, reset, and automatic framing. It never
//! renders; call [`OrbitController::sync`] to write the resulting position/target/up into a camera.
//!
//! # Coordinate convention
//!
//! The controller matches [`rgfx_core::Camera`]'s right-handed system. The orientation quaternion
//! rotates a canonical basis: the camera's *view basis* is `orientation * (X, Y, Z)` where `+Z`
//! points from the target toward the camera, `+Y` is up, and `+X` is right. With the identity
//! orientation the camera therefore sits on the target's `+Z` axis looking toward `-Z` with up
//! `+Y`, exactly like a freshly constructed [`rgfx_core::Camera`].
//!
//! # Free rotation (no gimbal lock, no clamp)
//!
//! [`OrbitController::orbit`] applies incremental rotations about the camera's *current* right and
//! up axes and composes them into the orientation quaternion. Because the tumble is expressed as a
//! quaternion there is **no pitch clamp and no gimbal lock**: the model can be rolled clean over
//! the poles and spun a full 360° in any direction. Roll composes about the view (line-of-sight)
//! axis.

use glam::{Mat3, Quat, Vec3};
use rgfx_core::{BoundingSphere, Camera, Projection};

use crate::framing::{
    DEFAULT_FRAMING_MARGIN, orthographic_fit_half_height, perspective_fit_distance,
};

/// The smallest orbit distance, keeping the camera from collapsing onto its target.
const MIN_DISTANCE: f32 = 1e-3;

/// Builds a rotation whose view basis has `+Z` pointing along `dir` (target → camera) and `+Y`
/// aligned as closely as possible with `up_hint`. Degenerate inputs fall back to sane axes so the
/// result is always a finite, orthonormal rotation.
fn orientation_from_dir_up(dir: Vec3, up_hint: Vec3) -> Quat {
    let z_axis = dir.normalize_or_zero();
    let z_axis = if z_axis.length_squared() > f32::EPSILON {
        z_axis
    } else {
        Vec3::Z
    };
    let up_hint = up_hint.normalize_or_zero();
    let up_hint = if up_hint.length_squared() > f32::EPSILON {
        up_hint
    } else {
        Vec3::Y
    };
    // Right = up × forward. If up is parallel to the view axis, pick any perpendicular.
    let mut x_axis = up_hint.cross(z_axis);
    if x_axis.length_squared() < f32::EPSILON {
        x_axis = z_axis.cross(Vec3::X);
        if x_axis.length_squared() < f32::EPSILON {
            x_axis = z_axis.cross(Vec3::Y);
        }
    }
    let x_axis = x_axis.normalize();
    let y_axis = z_axis.cross(x_axis).normalize();
    // Columns are the images of the canonical basis vectors, so `orientation * Z == z_axis`.
    Quat::from_mat3(&Mat3::from_cols(x_axis, y_axis, z_axis)).normalize()
}

/// A snapshot of the orbit parameters, used to implement [`OrbitController::reset`].
#[derive(Clone, Copy, Debug, PartialEq)]
struct OrbitState {
    target: Vec3,
    orientation: Quat,
    distance: f32,
    roll: f32,
}

/// A quaternion arcball camera controller operating on an [`rgfx_core::Camera`].
///
/// Construct one from an explicit target and distance, or derive it from an existing camera with
/// [`OrbitController::from_camera`]. Mutating methods change the internal state; call
/// [`OrbitController::sync`] to push the state into a camera before rendering.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrbitController {
    target: Vec3,
    /// The free view orientation. Rotates the canonical view basis (`+Z` target → camera).
    orientation: Quat,
    distance: f32,
    /// Roll about the view axis, in radians. `0` keeps `up` aligned with the orientation's up.
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
    /// controller can take over an already-positioned view without a visible jump.
    pub fn from_camera(camera: &Camera) -> Self {
        let offset = camera.position - camera.target;
        let distance = offset.length().max(MIN_DISTANCE);
        let orientation = orientation_from_dir_up(offset, camera.up);
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

    /// A best-effort yaw angle in radians (rotation about world up), derived from the current view
    /// direction. With a free orientation this is no longer an independent state variable; it is
    /// recovered from [`OrbitController::direction`] for display and legacy callers.
    pub fn yaw(&self) -> f32 {
        let d = self.direction();
        d.x.atan2(d.z)
    }

    /// A best-effort pitch angle in radians (tilt above/below the target's horizon plane), derived
    /// from the current view direction. See [`OrbitController::yaw`] for the caveat.
    pub fn pitch(&self) -> f32 {
        self.direction().y.clamp(-1.0, 1.0).asin()
    }

    /// The unit direction from the target toward the camera (the orientation's `+Z` axis).
    pub fn direction(&self) -> Vec3 {
        (self.orientation * Vec3::Z).normalize_or_zero()
    }

    /// The world-space camera position implied by the current orbit state.
    pub fn position(&self) -> Vec3 {
        self.target + self.direction() * self.distance
    }

    /// Orbits (tumbles) the camera by `yaw_delta` and `pitch_delta` radians about the camera's
    /// *current* up and right axes, composing the rotation into the orientation quaternion.
    ///
    /// There is no pole clamp and no gimbal lock: repeatedly tumbling in one direction rolls the
    /// model clean over the top and back, and any full turn (2π) returns to the start orientation.
    pub fn orbit(&mut self, yaw_delta: f32, pitch_delta: f32) {
        let yaw_delta = if yaw_delta.is_finite() {
            yaw_delta
        } else {
            0.0
        };
        let pitch_delta = if pitch_delta.is_finite() {
            pitch_delta
        } else {
            0.0
        };
        // Post-multiplying by a rotation expressed in the canonical basis applies it about the
        // camera's *current* axes (the conjugation `q * R * q⁻¹` rotates about `q * axis`). Yaw is
        // about local `+Y`; pitch about local `+X` (negated so +pitch tilts the camera upward).
        let rot = Quat::from_rotation_y(yaw_delta) * Quat::from_rotation_x(-pitch_delta);
        self.orientation = (self.orientation * rot).normalize();
    }

    /// The roll angle in radians (rotation of the up vector about the view axis).
    pub fn roll_angle(&self) -> f32 {
        self.roll
    }

    /// Rolls the view about the forward (view) axis by `delta` radians — the third rotation axis,
    /// tilting the horizon. Orbit tumbles the camera around the model; roll spins it about the line
    /// of sight without moving it.
    pub fn roll(&mut self, delta: f32) {
        if delta.is_finite() {
            self.roll += delta;
        }
    }

    /// Sets the orientation from yaw/pitch orbit angles (radians, unclamped) and records the result
    /// as the reset home, so `reset` returns here. Used to establish a pleasant default 3/4 view
    /// after [`OrbitController::auto_frame`].
    pub fn set_view(&mut self, yaw: f32, pitch: f32) {
        let yaw = if yaw.is_finite() { yaw } else { 0.0 };
        let pitch = if pitch.is_finite() { pitch } else { 0.0 };
        self.orientation = (Quat::from_rotation_y(yaw) * Quat::from_rotation_x(-pitch)).normalize();
        self.home.orientation = self.orientation;
    }

    /// The orientation's up axis (`orientation * +Y`), used as the un-rolled up reference.
    fn up_basis(&self) -> Vec3 {
        let up = (self.orientation * Vec3::Y).normalize_or_zero();
        if up.length_squared() > f32::EPSILON {
            up
        } else {
            Vec3::Y
        }
    }

    /// The camera up vector after applying roll about the view axis.
    fn effective_up(&self) -> Vec3 {
        let base = self.up_basis();
        if self.roll.abs() < f32::EPSILON {
            return base;
        }
        let forward = (-self.direction()).normalize_or_zero();
        if forward.length_squared() < f32::EPSILON {
            return base;
        }
        (Quat::from_axis_angle(forward, self.roll) * base).normalize_or_zero()
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
        // Camera looks from `position` toward `target`; forward = -direction.
        let forward = -self.direction();
        let right = forward.cross(self.up_basis());
        let right = if right.length_squared() > f32::EPSILON {
            right.normalize()
        } else {
            Vec3::X
        };
        let up = right.cross(forward).normalize();
        self.target += right * dx + up * dy;
    }

    /// Resets the orbit state to the values captured when the controller was created or last
    /// framed via [`OrbitController::auto_frame`] / [`OrbitController::set_view`].
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
