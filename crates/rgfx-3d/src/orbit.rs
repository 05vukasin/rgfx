//! An orbit camera controller layered on top of [`rgfx_core::Camera`].
//!
//! The controller stores the orbit *state* — a target point plus spherical coordinates (yaw,
//! pitch, distance) around it — and offers the interactions an interactive 3D viewer needs:
//! orbit, zoom (dolly), pan, reset, and automatic framing. It never renders; call
//! [`OrbitController::sync`] to write the resulting position/target/up into a camera.
//!
//! # Coordinate convention
//!
//! The controller matches [`rgfx_core::Camera`]'s right-handed system. With yaw and pitch both
//! zero the camera sits on the target's `+Z` axis looking toward `-Z`, exactly like a freshly
//! constructed [`rgfx_core::Camera`]. Yaw rotates around world up (`+Y`); pitch tilts up and
//! down and is clamped just short of the poles to avoid gimbal flip.

use std::f32::consts::FRAC_PI_2;

use glam::Vec3;
use rgfx_core::{BoundingSphere, Camera, Projection};

use crate::framing::{
    DEFAULT_FRAMING_MARGIN, orthographic_fit_half_height, perspective_fit_distance,
};

/// The smallest orbit distance, keeping the camera from collapsing onto its target.
const MIN_DISTANCE: f32 = 1e-3;

/// How close pitch may approach the poles (±90°) before being clamped, avoiding a degenerate
/// view basis where the view direction aligns with world up.
const MAX_PITCH: f32 = FRAC_PI_2 - 1e-3;

/// A snapshot of the orbit parameters, used to implement [`OrbitController::reset`].
#[derive(Clone, Copy, Debug, PartialEq)]
struct OrbitState {
    target: Vec3,
    yaw: f32,
    pitch: f32,
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
    yaw: f32,
    pitch: f32,
    distance: f32,
    up: Vec3,
    home: OrbitState,
}

impl OrbitController {
    /// Creates a controller orbiting `target` at `distance`, starting on the target's `+Z`
    /// axis (yaw = pitch = 0) with world up `+Y`.
    pub fn new(target: Vec3, distance: f32) -> Self {
        let distance = distance.max(MIN_DISTANCE);
        let home = OrbitState {
            target,
            yaw: 0.0,
            pitch: 0.0,
            distance,
        };
        Self {
            target,
            yaw: 0.0,
            pitch: 0.0,
            distance,
            up: Vec3::Y,
            home,
        }
    }

    /// Derives orbit parameters (target, yaw, pitch, distance) from an existing camera so the
    /// controller can take over an already-positioned view without a visible jump.
    pub fn from_camera(camera: &Camera) -> Self {
        let offset = camera.position - camera.target;
        let distance = offset.length().max(MIN_DISTANCE);
        let pitch = (offset.y / distance).clamp(-1.0, 1.0).asin();
        let yaw = offset.x.atan2(offset.z);
        let home = OrbitState {
            target: camera.target,
            yaw,
            pitch,
            distance,
        };
        Self {
            target: camera.target,
            yaw,
            pitch,
            distance,
            up: if camera.up.length_squared() > f32::EPSILON {
                camera.up.normalize()
            } else {
                Vec3::Y
            },
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

    /// The yaw angle in radians (rotation about world up).
    pub fn yaw(&self) -> f32 {
        self.yaw
    }

    /// The pitch angle in radians (tilt above/below the target's horizon plane).
    pub fn pitch(&self) -> f32 {
        self.pitch
    }

    /// The unit direction from the target toward the camera.
    pub fn direction(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        Vec3::new(cp * sy, sp, cp * cy)
    }

    /// The world-space camera position implied by the current orbit state.
    pub fn position(&self) -> Vec3 {
        self.target + self.direction() * self.distance
    }

    /// Orbits the camera by `yaw_delta` and `pitch_delta` radians. Pitch is clamped just short
    /// of the poles; yaw is free (adding a full turn returns to the same orientation).
    pub fn orbit(&mut self, yaw_delta: f32, pitch_delta: f32) {
        self.yaw += yaw_delta;
        self.pitch = (self.pitch + pitch_delta).clamp(-MAX_PITCH, MAX_PITCH);
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
        let right = forward.cross(self.up);
        let right = if right.length_squared() > f32::EPSILON {
            right.normalize()
        } else {
            Vec3::X
        };
        let up = right.cross(forward).normalize();
        self.target += right * dx + up * dy;
    }

    /// Resets the orbit state to the values captured when the controller was created or last
    /// framed via [`OrbitController::auto_frame`].
    pub fn reset(&mut self) {
        self.target = self.home.target;
        self.yaw = self.home.yaw;
        self.pitch = self.home.pitch;
        self.distance = self.home.distance;
    }

    /// Writes the current orbit state into `camera` (position, target, and up).
    pub fn sync(&self, camera: &mut Camera) {
        camera.position = self.position();
        camera.target = self.target;
        camera.up = self.up;
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
            yaw: self.yaw,
            pitch: self.pitch,
            distance: self.distance,
        };

        self.sync(camera);
    }
}
