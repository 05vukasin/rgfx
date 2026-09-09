//! Linear RGBA color used throughout the framebuffer.

/// An RGBA color with each channel in the range `0.0..=1.0`.
///
/// Colors are stored per framebuffer pixel. Encoders that emit grayscale derive a luminance
/// from the color via [`Color::luma`]; color-capable encoders quantize the channels directly.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    /// Red channel, `0.0..=1.0`.
    pub r: f32,
    /// Green channel, `0.0..=1.0`.
    pub g: f32,
    /// Blue channel, `0.0..=1.0`.
    pub b: f32,
    /// Alpha channel, `0.0..=1.0` (1.0 = opaque).
    pub a: f32,
}

impl Color {
    /// Fully transparent black.
    pub const TRANSPARENT: Color = Color::new(0.0, 0.0, 0.0, 0.0);
    /// Opaque black.
    pub const BLACK: Color = Color::new(0.0, 0.0, 0.0, 1.0);
    /// Opaque white.
    pub const WHITE: Color = Color::new(1.0, 1.0, 1.0, 1.0);

    /// Creates a color from raw channel values (not clamped).
    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// Creates an opaque color (`a = 1.0`).
    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self::new(r, g, b, 1.0)
    }

    /// Creates a color from 8-bit sRGB-ish channels, mapping `0..=255` to `0.0..=1.0` linearly.
    pub fn from_u8(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self::new(
            r as f32 / 255.0,
            g as f32 / 255.0,
            b as f32 / 255.0,
            a as f32 / 255.0,
        )
    }

    /// Converts to 8-bit channels, clamping each to `0.0..=1.0` first.
    pub fn to_u8(self) -> (u8, u8, u8, u8) {
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        (q(self.r), q(self.g), q(self.b), q(self.a))
    }

    /// Perceptual luminance in `0.0..=1.0` using Rec. 601 weights.
    ///
    /// Alpha is not premultiplied here; callers that need it should composite first.
    pub fn luma(self) -> f32 {
        (0.299 * self.r + 0.587 * self.g + 0.114 * self.b).clamp(0.0, 1.0)
    }

    /// Returns a copy with every channel clamped to `0.0..=1.0`.
    pub fn clamped(self) -> Self {
        Self::new(
            self.r.clamp(0.0, 1.0),
            self.g.clamp(0.0, 1.0),
            self.b.clamp(0.0, 1.0),
            self.a.clamp(0.0, 1.0),
        )
    }
}

impl Default for Color {
    fn default() -> Self {
        Color::TRANSPARENT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn luma_of_primaries_matches_rec601() {
        assert!((Color::BLACK.luma() - 0.0).abs() < 1e-6);
        assert!((Color::WHITE.luma() - 1.0).abs() < 1e-6);
        assert!((Color::rgb(1.0, 0.0, 0.0).luma() - 0.299).abs() < 1e-6);
        assert!((Color::rgb(0.0, 1.0, 0.0).luma() - 0.587).abs() < 1e-6);
        assert!((Color::rgb(0.0, 0.0, 1.0).luma() - 0.114).abs() < 1e-6);
    }

    #[test]
    fn u8_round_trip() {
        let c = Color::from_u8(255, 128, 0, 255);
        assert_eq!(c.to_u8(), (255, 128, 0, 255));
    }

    #[test]
    fn to_u8_clamps_out_of_range() {
        assert_eq!(Color::new(2.0, -1.0, 0.5, 1.0).to_u8(), (255, 0, 128, 255));
    }
}
