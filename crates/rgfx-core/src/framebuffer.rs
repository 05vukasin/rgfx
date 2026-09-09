//! The CPU render target that every media source converges onto.

use crate::Color;

/// A CPU render target holding a contiguous color buffer and a matching depth buffer.
///
/// Storage is row-major: the pixel at `(x, y)` lives at index `y * width + x`. Color and depth
/// are separate contiguous `Vec`s so the 3D rasterizer can depth-test without touching color and
/// the encoders can read color without touching depth.
///
/// # Reuse
///
/// Never allocate a fresh `Framebuffer` per frame. Call [`Framebuffer::resize`] (which reuses
/// the backing allocation when the new size fits) and [`Framebuffer::clear`] instead. This keeps
/// per-frame allocation out of the hot path, as required by the rendering pipeline.
#[derive(Clone, Debug)]
pub struct Framebuffer {
    width: usize,
    height: usize,
    color: Vec<Color>,
    depth: Vec<f32>,
}

/// The depth value a cleared framebuffer is initialized to (the far plane).
pub const DEPTH_FAR: f32 = 1.0;

impl Framebuffer {
    /// Creates a framebuffer of `width` × `height` pixels, cleared to transparent with depth at
    /// the far plane. A `0`-sized dimension is allowed and yields an empty buffer.
    pub fn new(width: usize, height: usize) -> Self {
        let len = width * height;
        Self {
            width,
            height,
            color: vec![Color::TRANSPARENT; len],
            depth: vec![DEPTH_FAR; len],
        }
    }

    /// The width in pixels.
    #[inline]
    pub fn width(&self) -> usize {
        self.width
    }

    /// The height in pixels.
    #[inline]
    pub fn height(&self) -> usize {
        self.height
    }

    /// The number of pixels (`width * height`).
    #[inline]
    pub fn len(&self) -> usize {
        self.color.len()
    }

    /// Whether the framebuffer has zero pixels.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.color.is_empty()
    }

    /// Resizes the framebuffer, reusing the existing allocation when it is large enough.
    ///
    /// The buffers are only grown (never shrunk in capacity), so repeatedly resizing between
    /// sizes below a previous maximum performs no reallocation. Contents after a resize are
    /// unspecified; call [`Framebuffer::clear`] before rendering.
    pub fn resize(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height;
        let len = width * height;
        self.color.resize(len, Color::TRANSPARENT);
        self.depth.resize(len, DEPTH_FAR);
        // `Vec::resize` never shrinks capacity, so the allocation is reused when len decreases.
    }

    /// Clears the color buffer to `color` and resets depth to the far plane.
    pub fn clear(&mut self, color: Color) {
        self.color.iter_mut().for_each(|c| *c = color);
        self.depth.iter_mut().for_each(|d| *d = DEPTH_FAR);
    }

    #[inline]
    fn index(&self, x: usize, y: usize) -> usize {
        debug_assert!(x < self.width && y < self.height, "pixel out of bounds");
        y * self.width + x
    }

    /// Whether `(x, y)` is inside the framebuffer.
    #[inline]
    pub fn in_bounds(&self, x: usize, y: usize) -> bool {
        x < self.width && y < self.height
    }

    /// The color at `(x, y)`. Panics in debug builds if out of bounds.
    #[inline]
    pub fn get(&self, x: usize, y: usize) -> Color {
        self.color[self.index(x, y)]
    }

    /// Sets the color at `(x, y)`. Panics in debug builds if out of bounds.
    #[inline]
    pub fn set(&mut self, x: usize, y: usize, c: Color) {
        let i = self.index(x, y);
        self.color[i] = c;
    }

    /// The perceptual luminance (`0.0..=1.0`) of the pixel at `(x, y)`.
    #[inline]
    pub fn luma(&self, x: usize, y: usize) -> f32 {
        self.get(x, y).luma()
    }

    /// The full color buffer as a slice, row-major.
    #[inline]
    pub fn color(&self) -> &[Color] {
        &self.color
    }

    /// The full color buffer as a mutable slice, row-major.
    #[inline]
    pub fn color_mut(&mut self) -> &mut [Color] {
        &mut self.color
    }

    /// The full depth buffer as a slice, row-major.
    #[inline]
    pub fn depth(&self) -> &[f32] {
        &self.depth
    }

    /// The full depth buffer as a mutable slice, row-major.
    #[inline]
    pub fn depth_mut(&mut self) -> &mut [f32] {
        &mut self.depth
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_clears_to_transparent_and_far() {
        let fb = Framebuffer::new(3, 2);
        assert_eq!(fb.width(), 3);
        assert_eq!(fb.height(), 2);
        assert_eq!(fb.len(), 6);
        assert!(fb.color().iter().all(|c| *c == Color::TRANSPARENT));
        assert!(fb.depth().iter().all(|d| *d == DEPTH_FAR));
    }

    #[test]
    fn set_get_round_trip_row_major() {
        let mut fb = Framebuffer::new(4, 4);
        fb.set(1, 2, Color::WHITE);
        assert_eq!(fb.get(1, 2), Color::WHITE);
        // row-major: (1,2) is index 2*4 + 1 = 9
        assert_eq!(fb.color()[9], Color::WHITE);
    }

    #[test]
    fn clear_resets_color_and_depth() {
        let mut fb = Framebuffer::new(2, 2);
        fb.set(0, 0, Color::WHITE);
        fb.depth_mut()[0] = 0.25;
        fb.clear(Color::BLACK);
        assert!(fb.color().iter().all(|c| *c == Color::BLACK));
        assert!(fb.depth().iter().all(|d| *d == DEPTH_FAR));
    }

    #[test]
    fn resize_reuses_allocation_when_shrinking() {
        let mut fb = Framebuffer::new(100, 100);
        let cap = fb.color.capacity();
        fb.resize(10, 10);
        assert_eq!(fb.len(), 100);
        // Shrinking must not reallocate: capacity is preserved.
        assert!(fb.color.capacity() >= cap);
        fb.resize(50, 50); // still under the original max → no realloc
        assert!(fb.color.capacity() >= cap);
    }

    #[test]
    fn zero_sized_is_allowed() {
        let fb = Framebuffer::new(0, 0);
        assert!(fb.is_empty());
        assert!(!fb.in_bounds(0, 0));
    }
}
