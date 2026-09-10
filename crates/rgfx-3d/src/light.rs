//! Directional-light math shared by the interactive viewers.
//!
//! A directional light is described by a unit *direction toward the light*. The 3D viewer lets the
//! user move that light around a sphere with two angles — an **azimuth** (rotation about world up,
//! `+Y`) and an **elevation** (tilt above or below the horizon plane). This module turns those two
//! angles into the world-space (or view-space) unit vector the [`crate::Rasterizer`] consumes.
//!
//! The convention matches [`crate::OrbitController`]'s: with both angles zero the direction is
//! `+Z`; increasing azimuth swings toward `+X`; increasing elevation lifts toward `+Y`.

use glam::Vec3;

/// Maps an `azimuth` and `elevation` (both in radians) to a unit direction vector.
///
/// - `azimuth` rotates about world up (`+Y`): `0` points along `+Z`, `+π/2` along `+X`.
/// - `elevation` tilts above (`+`) or below (`-`) the horizon plane toward `±Y`.
///
/// The result is always a unit vector for finite inputs (it is built directly from sines and
/// cosines of the angles), so it is safe to hand straight to
/// [`crate::Rasterizer::set_light_direction`].
///
/// # Example
///
/// ```
/// use glam::Vec3;
/// use rgfx_3d::direction_from_azimuth_elevation;
///
/// // Zero angles look along +Z.
/// let d = direction_from_azimuth_elevation(0.0, 0.0);
/// assert!((d - Vec3::Z).length() < 1e-6);
/// ```
pub fn direction_from_azimuth_elevation(azimuth: f32, elevation: f32) -> Vec3 {
    let (sa, ca) = azimuth.sin_cos();
    let (se, ce) = elevation.sin_cos();
    Vec3::new(ce * sa, se, ce * ca)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_PI_2;

    fn approx(a: Vec3, b: Vec3, eps: f32) -> bool {
        (a - b).length() < eps
    }

    #[test]
    fn zero_angles_point_along_plus_z() {
        assert!(approx(
            direction_from_azimuth_elevation(0.0, 0.0),
            Vec3::Z,
            1e-6
        ));
    }

    #[test]
    fn ninety_degree_azimuth_points_along_plus_x() {
        assert!(approx(
            direction_from_azimuth_elevation(FRAC_PI_2, 0.0),
            Vec3::X,
            1e-6
        ));
    }

    #[test]
    fn full_elevation_points_up() {
        assert!(approx(
            direction_from_azimuth_elevation(0.0, FRAC_PI_2),
            Vec3::Y,
            1e-6
        ));
    }

    #[test]
    fn result_is_unit_length_and_in_expected_quadrant() {
        // Upper-front-right: +X (east), +Y (up), +Z (toward the +Z axis) all positive.
        let d = direction_from_azimuth_elevation(0.46, 0.56);
        assert!((d.length() - 1.0).abs() < 1e-6, "must be a unit vector");
        assert!(
            d.x > 0.0 && d.y > 0.0 && d.z > 0.0,
            "upper-front-right quadrant"
        );
    }

    #[test]
    fn negative_elevation_points_below_the_horizon() {
        let d = direction_from_azimuth_elevation(0.0, -0.4);
        assert!(d.y < 0.0, "negative elevation dips below the horizon plane");
        assert!((d.length() - 1.0).abs() < 1e-6);
    }
}
