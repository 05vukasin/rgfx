//! An orbit camera controller layered on top of [`rgfx_core::Camera`].
//!
//! The controller stores the orbit *state* — a target point, a free view *orientation* (a
//! [`glam::Quat`]), and a distance — around it, and offers the interactions an interactive 3D
//! viewer needs: orbit, roll, zoom (dolly), pan, reset, and automatic framing. It never renders;
//! call [`OrbitController::sync`] to write the resulting position/target/up into a camera.
//!
//! # Coordinate convention
//!
//! The controller matches [`rgfx_core::Camera`]'s right-handed system. With the identity
//! orientation the camera sits on the target's `+Z` axis looking toward `-Z` with up `+Y`,
//! exactly like a freshly constructed [`rgfx_core::Camera`].
//!
//! # Free (arcball) rotation
//!
//! Orientation is a quaternion rather than clamped Euler yaw/pitch, so the model tumbles freely
//! in every direction with **no pole clamp and no gimbal lock**. [`OrbitController::orbit`]
//! composes incremental rotations about the camera's *current* right and up axes, so dragging or
//! arrowing always tumbles relative to what you see; [`OrbitController::roll`] composes a rotation
//! about the current view axis. A full turn about any axis returns to the start orientation.

use glam::{Quat, Vec3};
use rgfx_core::{BoundingSphere, Camera, Projection};

use crate::framing::{
    DEFAULT_FRAMING_MARGIN, orthographic_fit_half_height, perspective_fit_distance,
};

/// The smallest orbit distance, keeping the camera from collapsing onto its target.
const MIN_DISTANCE: f32 = 1e-3;

/// Builds the view orientation for a classic yaw/pitch pair, matching the legacy Euler
/// convention: `+yaw` swings the camera toward `+X`, `+pitch` lifts it toward `+Y`. Used by
/// [`OrbitController::from_camera`] and [`OrbitController::set_view`] to seed the quaternion.
fn orientation_from_yaw_pitch(yaw: f32, pitch: f32) -> Quat {
    (Quat::from_axis_angle(Vec3::Y, yaw) * Quat::from_axis_angle(Vec3::X, -pitch)).normalize()
}

/// A snapshot of the orbit parameters, used to implement [`OrbitController::reset`].
#[derive(Clone, Copy, Debug, PartialEq)]
struct OrbitState {
    target: Vec3,
    orientation: Quat,
    distance: f32,
}

/// An orbit camera controller operating on an [`rgfx_core::Camera`].
///
/// Construct one from an explicit target and distance, or derive it from an existing camera
/// with [`OrbitController::from_camera`]. Mutating methods change the internal state; call
/// [`OrbitController::sync`] to push the state into a camera before rendering.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrbitController {
    target: Vec3,
    /// The free view orientation. `orientation * +Z` is the unit direction from the target toward
    /// the camera; `orientation * +Y` is the camera up. The identity looks down `-Z` with up `+Y`.
    orientation: Quat,
    distance: f32,
    /// A running odometer of applied roll (radians) about the view axis, for reporting only. The
    /// authoritative roll lives inside `orientation`; this just tracks the net roll applied.
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
        // Recover the yaw/pitch that reproduce this offset direction, then build the orientation
        // from them so `position()` round-trips the camera position.
        let pitch = (offset.y / distance).clamp(-1.0, 1.0).asin();
        let yaw = offset.x.atan2(offset.z);
        let orientation = orientation_from_yaw_pitch(yaw, pitch);
        let home = OrbitState {
            target: camera.target,
            orientation,
            distance,
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

    /// The free view orientation as a quaternion (`orientation * +Z` points from the target toward
    /// the camera; `orientation * +Y` is the camera up).
    pub fn orientation(&self) -> Quat {
        self.orientation
    }

    /// The unit direction from the target toward the camera.
    pub fn direction(&self) -> Vec3 {
        self.orientation * Vec3::Z
    }

    /// The world-space camera position implied by the current orbit state.
    pub fn position(&self) -> Vec3 {
        self.target + self.direction() * self.distance
    }

    /// The net roll applied about the view axis, in radians (reporting odometer). `0` means the
    /// view has not been rolled since the last reset.
    pub fn roll_angle(&self) -> f32 {
        self.roll
    }

    /// Orbits the camera by `yaw_delta` and `pitch_delta` radians, composing incremental rotations
    /// about the camera's **current** up and right axes. There is no pole clamp: pitch tumbles
    /// freely over the top and bottom, and a full turn about either axis returns to the start.
    pub fn orbit(&mut self, yaw_delta: f32, pitch_delta: f32) {
        if !yaw_delta.is_finite() || !pitch_delta.is_finite() {
            return;
        }
        // Post-multiplying by a base-frame rotation is equivalent to rotating about that axis
        // *after* the current orientation — i.e. about the camera's current world-space axis.
        // `+pitch` should lift the camera toward `+Y`, hence the negated X rotation.
        let delta = Quat::from_axis_angle(Vec3::Y, yaw_delta)
            * Quat::from_axis_angle(Vec3::X, -pitch_delta);
        self.orientation = (self.orientation * delta).normalize();
    }

    /// Rolls the view about the forward (view) axis by `delta` radians — the third rotation axis,
    /// tilting the horizon. Yaw and pitch tumble the camera around the model; roll spins the camera
    /// about the line of sight, composing into the orientation.
    pub fn roll(&mut self, delta: f32) {
        if delta.is_finite() {
            self.orientation =
                (self.orientation * Quat::from_axis_angle(Vec3::Z, delta)).normalize();
            self.roll += delta;
        }
    }

    /// Sets the view orientation from a yaw/pitch pair (radians, legacy Euler convention) and
    /// records it as the reset home, so `reset` returns here. Used to establish a pleasant default
    /// 3/4 view after [`OrbitController::auto_frame`]. Clears any accumulated roll.
    pub fn set_view(&mut self, yaw: f32, pitch: f32) {
        self.orientation = orientation_from_yaw_pitch(yaw, pitch);
        self.roll = 0.0;
        self.home.orientation = self.orientation;
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
        let right = self.orientation * Vec3::X;
        let up = self.orientation * Vec3::Y;
        self.target += right * dx + up * dy;
    }

    /// Resets the orbit state to the values captured when the controller was created or last
    /// framed via [`OrbitController::auto_frame`] / [`OrbitController::set_view`].
    pub fn reset(&mut self) {
        self.target = self.home.target;
        self.orientation = self.home.orientation;
        self.distance = self.home.distance;
        self.roll = 0.0;
    }

    /// Writes the current orbit state into `camera` (position, target, and up).
    pub fn sync(&self, camera: &mut Camera) {
        camera.position = self.position();
        camera.target = self.target;
        camera.up = self.orientation * Vec3::Y;
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
        };

        self.sync(camera);
    }
}
