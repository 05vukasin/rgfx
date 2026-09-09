//! Automatic model framing: choose a camera distance and clip planes that fit a bounding
//! sphere inside the viewport.
//!
//! All logic here is pure math on [`rgfx_core::Camera`] and [`rgfx_core::BoundingSphere`]; it
//! never renders or allocates. The orbit controller ([`crate::OrbitController`]) builds on top
//! of it so an interactive viewer and a one-shot renderer share the same framing rules.

use glam::Vec3;
use rgfx_core::{BoundingSphere, Camera, Projection};

/// The default padding factor applied when framing, so the model does not touch the viewport
/// edges. A value of `1.1` leaves roughly 10% margin around the projected sphere.
pub const DEFAULT_FRAMING_MARGIN: f32 = 1.1;

/// The smallest radius considered when framing, guarding against degenerate (point) bounds.
const MIN_RADIUS: f32 = 1e-4;

/// The smallest camera distance produced by framing, keeping the near plane positive.
const MIN_DISTANCE: f32 = 1e-3;

/// The perspective camera distance from a sphere of `radius` that makes the sphere fill the
/// viewport (minus `margin`) for a vertical field of view `fov_y` (radians) and viewport
/// `aspect` (width / height).
///
/// The binding constraint is the smaller of the vertical and horizontal half field-of-view
/// angles, so the sphere fits in both dimensions. The returned distance is measured from the
/// sphere center to the camera.
pub fn perspective_fit_distance(radius: f32, fov_y: f32, aspect: f32, margin: f32) -> f32 {
    let radius = radius.max(MIN_RADIUS);
    let half_v = (fov_y * 0.5).max(f32::EPSILON);
    // Horizontal half-angle derived from the vertical one and the aspect ratio.
    let half_h = (aspect.max(f32::EPSILON) * half_v.tan()).atan();
    let half = half_v.min(half_h).max(f32::EPSILON);
    ((radius * margin) / half.sin()).max(MIN_DISTANCE)
}

/// The orthographic vertical half-height that fits a sphere of `radius` for a given viewport
/// `aspect`, including `margin`. When the viewport is taller than it is wide the half-height is
/// enlarged so the sphere still fits horizontally.
pub fn orthographic_fit_half_height(radius: f32, aspect: f32, margin: f32) -> f32 {
    let radius = radius.max(MIN_RADIUS);
    (radius * margin) / aspect.clamp(f32::EPSILON, 1.0)
}

/// Reframes `camera` so `sphere` fits inside a viewport of the given `aspect`, keeping the
/// camera's current view direction. Updates the camera target, position, aspect, and near/far
/// clip planes; for an orthographic camera it also updates the visible half-height.
///
/// Uses [`DEFAULT_FRAMING_MARGIN`]. This is the free-function form of
/// [`crate::OrbitController::auto_frame`] for callers that do not keep an orbit controller.
pub fn frame_camera(camera: &mut Camera, sphere: &BoundingSphere, aspect: f32) {
    frame_camera_with_margin(camera, sphere, aspect, DEFAULT_FRAMING_MARGIN);
}

/// [`frame_camera`] with an explicit `margin` factor (`1.0` = sphere touches the edges).
pub fn frame_camera_with_margin(
    camera: &mut Camera,
    sphere: &BoundingSphere,
    aspect: f32,
    margin: f32,
) {
    let radius = sphere.radius.max(MIN_RADIUS);
    camera.aspect = aspect;

    // Preserve the current viewing direction (camera -> target); fall back to +Z if degenerate.
    let dir = {
        let d = camera.position - camera.target;
        if d.length_squared() > f32::EPSILON {
            d.normalize()
        } else {
            Vec3::Z
        }
    };

    let distance = match camera.projection {
        Projection::Perspective { fov_y } => {
            perspective_fit_distance(radius, fov_y, aspect, margin)
        }
        Projection::Orthographic { .. } => {
            let half_height = orthographic_fit_half_height(radius, aspect, margin);
            camera.projection = Projection::Orthographic { half_height };
            // Distance is free for an orthographic camera; pick one that brackets the sphere.
            (radius * 2.0).max(MIN_DISTANCE)
        }
    };

    camera.target = sphere.center;
    camera.position = sphere.center + dir * distance;
    camera.near = (distance - radius).max(radius * 1e-3).max(MIN_DISTANCE);
    camera.far = (distance + radius).max(camera.near + MIN_DISTANCE);
}
