//! Directional-light direction math shared by the viewer's light controls.
//!
//! A directional light is just a unit direction. Placing it on a sphere with two angles —
//! *azimuth* (around the vertical axis) and *elevation* (above the horizontal plane) — gives a
//! deterministic, intuitive way to "move the light around" that the interactive light menu drives.
//! This is pure math with no rendering state, so it lives here next to the rasterizer and is
//! unit-tested in isolation.

use glam::Vec3;

/// The unit direction *toward* a directional light placed at the given `azimuth` and `elevation`
/// (both in radians).
///
/// The convention matches a right-handed, `+Y`-up world:
/// - **elevation** lifts the light off the horizontal `XZ` plane; `y = sin(elevation)`.
/// - **azimuth** swings it around the vertical axis within that plane, measured from `+Z` toward
///   `+X`, so `x = cos(elevation) * sin(azimuth)` and `z = cos(elevation) * cos(azimuth)`.
///
/// At `(0, 0)` the direction is `+Z` (straight toward a default camera); increasing the azimuth
/// rotates it toward `+X` and increasing the elevation lifts it toward `+Y`. The result is always a
/// unit vector (its length is `1` for every finite input).
pub fn direction_from_azimuth_elevation(azimuth: f32, elevation: f32) -> Vec3 {
    let (sin_el, cos_el) = elevation.sin_cos();
    let (sin_az, cos_az) = azimuth.sin_cos();
    Vec3::new(cos_el * sin_az, sin_el, cos_el * cos_az)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_angles_point_toward_plus_z() {
        let d = direction_from_azimuth_elevation(0.0, 0.0);
        assert!((d - Vec3::Z).length() < 1e-6);
    }

    #[test]
    fn result_is_always_a_unit_vector() {
        for &az in &[-2.0_f32, -0.5, 0.0, 0.7, 3.0] {
            for &el in &[-1.2_f32, 0.0, 0.4, 1.2] {
                let d = direction_from_azimuth_elevation(az, el);
                assert!((d.length() - 1.0).abs() < 1e-6, "az={az} el={el} not unit");
            }
        }
    }

    #[test]
    fn positive_azimuth_and_elevation_land_in_the_expected_quadrant() {
        // Up-front-right: +X (positive azimuth), +Y (positive elevation), +Z (front).
        let d = direction_from_azimuth_elevation(0.5, 0.4);
        assert!(d.x > 0.0, "positive azimuth tilts toward +X");
        assert!(d.y > 0.0, "positive elevation lifts toward +Y");
        assert!(d.z > 0.0, "still on the +Z (front) half");
    }

    #[test]
    fn quarter_turn_azimuth_points_along_plus_x() {
        let d = direction_from_azimuth_elevation(std::f32::consts::FRAC_PI_2, 0.0);
        assert!((d - Vec3::X).length() < 1e-6);
    }

    #[test]
    fn full_elevation_points_straight_up() {
        let d = direction_from_azimuth_elevation(1.3, std::f32::consts::FRAC_PI_2);
        assert!((d - Vec3::Y).length() < 1e-6);
    }
}
