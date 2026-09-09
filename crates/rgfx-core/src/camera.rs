//! The camera shared by the 3D renderer and the interactive viewer.
//!
//! This crate owns the camera *data* and its view/projection matrices so the [`SceneRenderer`]
//! trait can name it. Higher-level *controls* (orbit, zoom, pan, automatic framing) are layered
//! on top in the `rgfx-3d` crate, operating on this type.
//!
//! [`SceneRenderer`]: crate::SceneRenderer

use glam::{Mat4, Vec3};

/// How a camera projects the scene onto the framebuffer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Projection {
    /// Perspective projection with a vertical field of view in radians.
    Perspective {
        /// Vertical field of view, in radians.
        fov_y: f32,
    },
    /// Orthographic projection with a vertical half-height in world units.
    Orthographic {
        /// Half of the visible vertical extent, in world units.
        half_height: f32,
    },
}

/// A camera positioned in world space, looking at a target.
///
/// Uses a right-handed coordinate system and depth mapped to `0.0..=1.0` (matching the
/// framebuffer's far value of `1.0`), consistent with `glam`'s `*_rh` matrix builders.
#[derive(Clone, Copy, Debug)]
pub struct Camera {
    /// Camera position in world space.
    pub position: Vec3,
    /// The point the camera looks at.
    pub target: Vec3,
    /// The camera's up direction.
    pub up: Vec3,
    /// Aspect ratio (width / height) of the target viewport.
    pub aspect: f32,
    /// Near clip plane distance (> 0).
    pub near: f32,
    /// Far clip plane distance (> near).
    pub far: f32,
    /// The projection kind.
    pub projection: Projection,
}

impl Camera {
    /// Creates a perspective camera with sensible defaults looking down `-Z` at the origin.
    pub fn perspective(aspect: f32, fov_y: f32) -> Self {
        Self {
            position: Vec3::new(0.0, 0.0, 3.0),
            target: Vec3::ZERO,
            up: Vec3::Y,
            aspect,
            near: 0.1,
            far: 100.0,
            projection: Projection::Perspective { fov_y },
        }
    }

    /// Creates an orthographic camera looking down `-Z` at the origin.
    pub fn orthographic(aspect: f32, half_height: f32) -> Self {
        Self {
            position: Vec3::new(0.0, 0.0, 3.0),
            target: Vec3::ZERO,
            up: Vec3::Y,
            aspect,
            near: 0.1,
            far: 100.0,
            projection: Projection::Orthographic { half_height },
        }
    }

    /// The right-handed view matrix (world → camera space).
    pub fn view_matrix(&self) -> Mat4 {
        Mat4::look_at_rh(self.position, self.target, self.up)
    }

    /// The projection matrix (camera → clip space), depth in `0.0..=1.0`.
    pub fn projection_matrix(&self) -> Mat4 {
        match self.projection {
            Projection::Perspective { fov_y } => {
                Mat4::perspective_rh(fov_y, self.aspect.max(f32::EPSILON), self.near, self.far)
            }
            Projection::Orthographic { half_height } => {
                let hh = half_height.max(f32::EPSILON);
                let hw = hh * self.aspect.max(f32::EPSILON);
                Mat4::orthographic_rh(-hw, hw, -hh, hh, self.near, self.far)
            }
        }
    }

    /// The combined view-projection matrix (world → clip space).
    pub fn view_projection(&self) -> Mat4 {
        self.projection_matrix() * self.view_matrix()
    }

    /// Updates the aspect ratio from a viewport size in pixels.
    ///
    /// A zero height is ignored to avoid a non-finite aspect ratio.
    pub fn set_aspect_from_size(&mut self, width: usize, height: usize) {
        if height != 0 {
            self.aspect = width as f32 / height as f32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_matrix_places_target_on_negative_z() {
        // Camera at +Z looking at origin: the origin should map to camera space (0,0,-3).
        let cam = Camera::perspective(1.0, 60_f32.to_radians());
        let p = cam.view_matrix().transform_point3(Vec3::ZERO);
        assert!((p.x).abs() < 1e-5 && (p.y).abs() < 1e-5);
        assert!((p.z + 3.0).abs() < 1e-5);
    }

    #[test]
    fn perspective_keeps_center_centered() {
        let cam = Camera::perspective(1.5, 60_f32.to_radians());
        let clip = cam.view_projection().project_point3(cam.target);
        // The target sits on the view axis, so it projects to the center of the screen.
        assert!(clip.x.abs() < 1e-5 && clip.y.abs() < 1e-5);
    }

    #[test]
    fn set_aspect_ignores_zero_height() {
        let mut cam = Camera::perspective(1.0, 1.0);
        cam.set_aspect_from_size(200, 0);
        assert_eq!(cam.aspect, 1.0);
        cam.set_aspect_from_size(200, 100);
        assert_eq!(cam.aspect, 2.0);
    }
}
