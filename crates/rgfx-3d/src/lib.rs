//! `rgfx-3d`: the 3D math, camera-controls, and automatic-framing layer for rgfx.
//!
//! [`rgfx_core`] owns the camera *data* and its view/projection matrices, plus the mesh and
//! bounds primitives ([`rgfx_core::Camera`], [`rgfx_core::BoundingSphere`], …). This crate adds
//! the *controls* on top: an [`OrbitController`] for interactive orbit/zoom/pan/reset around a
//! target, and automatic model framing that positions the camera so a bounding sphere fits the
//! viewport with sensible clip planes.
//!
//! It also houses the CPU software rasterizer: [`Rasterizer`] implements
//! [`rgfx_core::SceneRenderer`], running the full transform → near-clip → project → cull →
//! barycentric-fill → depth-test → shade pipeline. Its [`ShadingMode`] covers unlit, flat and
//! smooth Lambert lighting (ambient + directional diffuse), the normals and depth debug views,
//! and a wireframe mode that draws deduplicated mesh edges via [`draw_line`]. Built-in mesh
//! [`primitives`] (a cube and a tetrahedron) let the CLI show interactive 3D before any loader is
//! involved.
//!
//! # Example
//!
//! ```
//! use glam::Vec3;
//! use rgfx_core::{BoundingSphere, Camera};
//! use rgfx_3d::OrbitController;
//!
//! let mut camera = Camera::perspective(16.0 / 9.0, 60_f32.to_radians());
//! let sphere = BoundingSphere { center: Vec3::ZERO, radius: 1.0 };
//!
//! let mut controls = OrbitController::from_camera(&camera);
//! controls.auto_frame(&mut camera, &sphere, 16.0 / 9.0);
//! controls.orbit(30_f32.to_radians(), 15_f32.to_radians());
//! controls.sync(&mut camera);
//! ```
#![warn(missing_docs)]
#![forbid(unsafe_code)]

pub mod blend;
mod framing;
pub mod gltf;
mod light;
mod line;
pub mod obj;
mod orbit;
pub mod primitives;
mod raster;
mod simplify;
pub mod stl;

pub use blend::{blender_available, load_blend, load_blend_with_stats};
pub use framing::{
    DEFAULT_FRAMING_MARGIN, frame_camera, frame_camera_with_margin, orthographic_fit_half_height,
    perspective_fit_distance,
};
pub use gltf::{GltfStats, load_gltf, load_gltf_with_stats};
pub use light::direction_from_azimuth_elevation;
pub use line::draw_line;
pub use obj::{ObjStats, load_obj};
pub use orbit::OrbitController;
pub use raster::{Cull, FrontFace, Rasterizer, ShadingMode};
pub use simplify::simplify_scene;
pub use stl::{StlOptions, StlStats, load_stl, load_stl_with_options};

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
    use rgfx_core::{BoundingSphere, Camera, Projection};

    const EPS: f32 = 1e-4;

    fn approx(a: Vec3, b: Vec3, eps: f32) -> bool {
        (a - b).length() < eps
    }

    // --- Orbit state / camera position math ---------------------------------------------------

    #[test]
    fn roll_rotates_camera_up_about_the_view_axis() {
        // Looking down -Z from +Z, up starts at +Y. A +90° roll about the view axis rotates up
        // into the horizontal plane (|up.y| ~ 0), and the up vector stays unit length.
        let mut c = OrbitController::new(Vec3::ZERO, 3.0);
        let mut cam = Camera::perspective(1.0, 60_f32.to_radians());
        c.sync(&mut cam);
        assert!(approx(cam.up, Vec3::Y, EPS), "no roll => up is +Y");
        c.roll(std::f32::consts::FRAC_PI_2);
        c.sync(&mut cam);
        assert!(cam.up.y.abs() < EPS, "90° roll tilts up out of vertical");
        assert!((cam.up.length() - 1.0).abs() < EPS, "up stays unit length");
        // A full turn returns to the original up.
        c.roll(std::f32::consts::FRAC_PI_2 * 3.0);
        c.sync(&mut cam);
        assert!(approx(cam.up, Vec3::Y, 1e-3), "2π roll returns to +Y");
    }

    #[test]
    fn reset_restores_orientation_and_set_view_rehomes() {
        let mut c = OrbitController::new(Vec3::ZERO, 3.0);
        c.set_view(0.6, 0.4); // establish a default 3/4 view as the new home
        let home_dir = c.direction();
        let home_pos = c.position();
        c.roll(0.5);
        c.orbit(1.0, 0.2);
        c.reset();
        assert!(
            (c.roll_angle()).abs() < EPS,
            "reset clears roll to home (0)"
        );
        assert!(
            approx(c.direction(), home_dir, EPS) && approx(c.position(), home_pos, EPS),
            "reset returns to the set_view home orientation"
        );
    }

    #[test]
    fn default_orientation_sits_on_plus_z() {
        let c = OrbitController::new(Vec3::ZERO, 3.0);
        assert!(approx(c.position(), Vec3::new(0.0, 0.0, 3.0), EPS));
        assert!(approx(c.direction(), Vec3::Z, EPS));
    }

    #[test]
    fn sync_places_target_at_expected_camera_space_depth() {
        // Hand reference: camera at (0,0,3) looking at origin -> origin maps to (0,0,-3).
        let c = OrbitController::new(Vec3::ZERO, 3.0);
        let mut cam = Camera::perspective(1.0, 60_f32.to_radians());
        c.sync(&mut cam);
        let p = cam.view_matrix().transform_point3(Vec3::ZERO);
        assert!(p.x.abs() < EPS && p.y.abs() < EPS);
        assert!((p.z + 3.0).abs() < EPS);
    }

    #[test]
    fn synced_target_projects_to_screen_center() {
        let mut c = OrbitController::new(Vec3::new(1.0, 2.0, -3.0), 5.0);
        c.orbit(0.7, 0.3);
        let mut cam = Camera::perspective(1.5, 50_f32.to_radians());
        c.sync(&mut cam);
        let ndc = cam.view_projection().project_point3(c.target());
        assert!(ndc.x.abs() < EPS && ndc.y.abs() < EPS);
    }

    #[test]
    fn yaw_ninety_degrees_moves_camera_onto_plus_x() {
        let mut c = OrbitController::new(Vec3::ZERO, 2.0);
        c.orbit(std::f32::consts::FRAC_PI_2, 0.0);
        assert!(approx(c.position(), Vec3::new(2.0, 0.0, 0.0), EPS));
    }

    #[test]
    fn from_camera_round_trips_position() {
        let mut cam = Camera::perspective(1.0, 60_f32.to_radians());
        cam.position = Vec3::new(3.0, 4.0, 5.0);
        cam.target = Vec3::new(1.0, 0.0, -1.0);
        let c = OrbitController::from_camera(&cam);
        assert!(approx(c.position(), cam.position, 1e-3));
        assert!(approx(c.target(), cam.target, EPS));
    }

    // --- Orbit / zoom / pan / reset invariants ------------------------------------------------

    #[test]
    fn orbit_full_turn_returns_to_start() {
        let mut c = OrbitController::new(Vec3::ZERO, 4.0);
        c.orbit(0.0, 0.4); // some pitch
        let before = c.position();
        c.orbit(std::f32::consts::TAU, 0.0); // full yaw turn
        assert!(approx(c.position(), before, EPS));
    }

    #[test]
    fn up_rotation_full_turn_returns_to_start_no_clamp() {
        // Arcball: many small up-rotations summing to a full 2π tumble the model over the top and
        // back to the start — no pole clamp, no gimbal lock.
        let mut c = OrbitController::new(Vec3::ZERO, 3.0);
        let before_pos = c.position();
        let before_up = {
            let mut cam = Camera::perspective(1.0, 60_f32.to_radians());
            c.sync(&mut cam);
            cam.up
        };
        let steps = 360;
        let step = std::f32::consts::TAU / steps as f32;
        for _ in 0..steps {
            c.orbit(0.0, step); // pure up-rotation, straight over the pole
        }
        assert!(c.position().is_finite(), "basis stays finite over the pole");
        assert!(
            approx(c.position(), before_pos, 1e-3),
            "2π of up-rotation returns to the start position"
        );
        let mut cam = Camera::perspective(1.0, 60_f32.to_radians());
        c.sync(&mut cam);
        assert!(
            approx(cam.up, before_up, 1e-3),
            "2π of up-rotation returns the up vector"
        );
    }

    #[test]
    fn up_then_right_gives_orthonormal_basis_past_ninety_degrees() {
        // Tumble well past the old ±90° pitch limit, then rotate right: the camera basis must stay
        // orthonormal and finite (no gimbal collapse), which the clamped Euler model could not do.
        let mut c = OrbitController::new(Vec3::ZERO, 2.0);
        c.orbit(0.0, 2.2); // ~126° up — past the old clamp
        c.orbit(1.3, 0.0); // then right
        let mut cam = Camera::perspective(1.0, 60_f32.to_radians());
        c.sync(&mut cam);

        let forward = (cam.target - cam.position).normalize();
        let up = cam.up;
        let right = forward.cross(up);
        for v in [forward, up, right] {
            assert!(
                v.is_finite() && (v.length() - 1.0).abs() < 1e-3,
                "unit + finite"
            );
        }
        assert!(forward.dot(up).abs() < 1e-3, "forward ⟂ up");
        assert!(forward.dot(right).abs() < 1e-3, "forward ⟂ right");
        assert!(up.dot(right).abs() < 1e-3, "up ⟂ right");
    }

    #[test]
    fn roll_composes_without_disturbing_the_tumble() {
        // Roll rotates the up vector about the view axis without moving the camera position
        // (the line of sight is unchanged), and a full turn of roll returns to the start.
        let mut c = OrbitController::new(Vec3::ZERO, 3.0);
        c.orbit(0.7, 1.9); // tumble past 90°
        let dir_before = c.direction();
        let pos_before = c.position();
        c.roll(0.6);
        assert!(
            approx(c.direction(), dir_before, EPS) && approx(c.position(), pos_before, EPS),
            "roll must not move the camera along the view axis"
        );
        c.roll(std::f32::consts::TAU - 0.6);
        let mut cam = Camera::perspective(1.0, 60_f32.to_radians());
        c.sync(&mut cam);
        assert!(cam.up.is_finite(), "up stays finite after composed roll");
    }

    #[test]
    fn zoom_scales_distance_and_clamps_positive() {
        let mut c = OrbitController::new(Vec3::ZERO, 10.0);
        c.zoom(0.5);
        assert!((c.distance() - 5.0).abs() < EPS);
        c.zoom(-1.0); // invalid -> ignored
        assert!((c.distance() - 5.0).abs() < EPS);
        c.zoom(1e-9); // clamps to minimum, stays positive
        assert!(c.distance() > 0.0);
    }

    #[test]
    fn dolly_adds_distance() {
        let mut c = OrbitController::new(Vec3::ZERO, 5.0);
        c.dolly(2.0);
        assert!((c.distance() - 7.0).abs() < EPS);
        c.dolly(-100.0); // clamps to minimum
        assert!(c.distance() > 0.0);
    }

    #[test]
    fn pan_moves_target_in_view_plane_only() {
        let mut c = OrbitController::new(Vec3::ZERO, 3.0);
        let dir_before = c.direction();
        let dist_before = c.distance();
        c.pan(2.0, 1.0);
        // Default orientation: right = +X, up = +Y, so target shifts by (2,1,0).
        assert!(approx(c.target(), Vec3::new(2.0, 1.0, 0.0), EPS));
        // Panning preserves the view direction and distance.
        assert!(approx(c.direction(), dir_before, EPS));
        assert!((c.distance() - dist_before).abs() < EPS);
    }

    #[test]
    fn reset_restores_initial_state() {
        let mut c = OrbitController::new(Vec3::new(1.0, 1.0, 1.0), 4.0);
        let home_pos = c.position();
        c.orbit(1.0, 0.5);
        c.zoom(2.0);
        c.pan(3.0, -2.0);
        c.reset();
        assert!(approx(c.position(), home_pos, EPS));
    }

    // --- Auto framing -------------------------------------------------------------------------

    /// Sample points across a sphere's surface, returning true if every one projects inside the
    /// clip cube (`|x| <= 1`, `|y| <= 1`) for the given camera.
    fn sphere_fits(cam: &Camera, sphere: &BoundingSphere) -> bool {
        let vp = cam.view_projection();
        let steps = 16;
        for i in 0..=steps {
            let phi = std::f32::consts::PI * (i as f32 / steps as f32); // 0..pi
            for j in 0..steps {
                let theta = std::f32::consts::TAU * (j as f32 / steps as f32); // 0..2pi
                let dir = Vec3::new(phi.sin() * theta.cos(), phi.cos(), phi.sin() * theta.sin());
                let p = sphere.center + dir * sphere.radius;
                let ndc = vp.project_point3(p);
                if ndc.x.abs() > 1.0 + 1e-3 || ndc.y.abs() > 1.0 + 1e-3 {
                    return false;
                }
            }
        }
        true
    }

    #[test]
    fn auto_frame_fits_unit_sphere_for_several_aspects() {
        let sphere = BoundingSphere {
            center: Vec3::new(2.0, -1.0, 0.5),
            radius: 1.0,
        };
        for &aspect in &[0.5_f32, 1.0, 4.0 / 3.0, 16.0 / 9.0, 2.5] {
            let mut cam = Camera::perspective(aspect, 60_f32.to_radians());
            let mut c = OrbitController::from_camera(&cam);
            c.auto_frame(&mut cam, &sphere, aspect);

            assert!(cam.near > 0.0, "near must stay positive");
            assert!(cam.far > cam.near, "far must exceed near");
            assert!(approx(cam.target, sphere.center, EPS), "target centered");
            assert!(
                sphere_fits(&cam, &sphere),
                "sphere must fit for aspect {aspect}"
            );
        }
    }

    #[test]
    fn auto_frame_fits_orthographic_camera() {
        let sphere = BoundingSphere {
            center: Vec3::ZERO,
            radius: 2.0,
        };
        for &aspect in &[0.5_f32, 1.0, 2.0] {
            let mut cam = Camera::orthographic(aspect, 1.0);
            let mut c = OrbitController::from_camera(&cam);
            c.auto_frame(&mut cam, &sphere, aspect);
            assert!(matches!(cam.projection, Projection::Orthographic { .. }));
            assert!(cam.near > 0.0 && cam.far > cam.near);
            assert!(
                sphere_fits(&cam, &sphere),
                "ortho sphere must fit for aspect {aspect}"
            );
        }
    }

    #[test]
    fn auto_frame_preserves_view_direction() {
        let sphere = BoundingSphere {
            center: Vec3::ZERO,
            radius: 1.0,
        };
        let mut cam = Camera::perspective(1.0, 60_f32.to_radians());
        let mut c = OrbitController::from_camera(&cam);
        c.orbit(0.9, 0.4);
        c.sync(&mut cam);
        let dir_before = c.direction();
        c.auto_frame(&mut cam, &sphere, 1.0);
        assert!(approx(c.direction(), dir_before, EPS));
    }

    #[test]
    fn auto_frame_sets_reset_home() {
        let sphere = BoundingSphere {
            center: Vec3::new(5.0, 5.0, 5.0),
            radius: 1.0,
        };
        let mut cam = Camera::perspective(1.0, 60_f32.to_radians());
        let mut c = OrbitController::new(Vec3::ZERO, 3.0);
        c.auto_frame(&mut cam, &sphere, 1.0);
        let framed = c.position();
        c.orbit(1.0, 0.5);
        c.reset();
        assert!(approx(c.position(), framed, EPS));
    }

    // --- Framing math free functions ----------------------------------------------------------

    #[test]
    fn free_function_frame_matches_controller() {
        let sphere = BoundingSphere {
            center: Vec3::ZERO,
            radius: 1.0,
        };
        let mut cam = Camera::perspective(1.5, 60_f32.to_radians());
        frame_camera(&mut cam, &sphere, 1.5);
        assert!(sphere_fits(&cam, &sphere));
    }

    #[test]
    fn perspective_fit_distance_grows_with_radius() {
        let d1 = perspective_fit_distance(1.0, 60_f32.to_radians(), 1.0, DEFAULT_FRAMING_MARGIN);
        let d2 = perspective_fit_distance(2.0, 60_f32.to_radians(), 1.0, DEFAULT_FRAMING_MARGIN);
        assert!((d2 - 2.0 * d1).abs() < 1e-3);
    }
}
