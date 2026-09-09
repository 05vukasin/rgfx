//! The CPU software rasterizer: the heart of the 3D engine.
//!
//! [`Rasterizer`] implements [`rgfx_core::SceneRenderer`], turning a [`Scene`] seen through a
//! [`Camera`] into a [`Framebuffer`]'s color and depth buffers. The pipeline is the classic CPU
//! path:
//!
//! 1. **Transform** each vertex model → world → view → clip with the camera's
//!    view-projection matrix (mesh transforms are baked into positions, so the model matrix is the
//!    identity here).
//! 2. **Near-plane clip** each triangle so no vertex sits on or behind the camera plane. This is
//!    what makes the perspective divide safe (every surviving vertex has `w > 0`).
//! 3. **Perspective divide** to normalized device coordinates, then the **viewport transform** to
//!    pixel space (with the Y axis flipped so `+Y` is up on screen).
//! 4. **Backface cull** by screen-space winding (configurable front face; cull back, front, or
//!    neither).
//! 5. **Barycentric fill** with a consistent top-left edge rule (no gaps or double-coverage on
//!    shared edges), interpolating depth linearly in screen space and attributes
//!    perspective-correctly.
//! 6. **Depth test** against [`Framebuffer::depth_mut`] (`frag_depth < depth` writes).
//!
//! The rasterizer never allocates per frame and never touches stdout. Off-screen or degenerate
//! triangles are dropped without panicking or writing out of bounds.

use glam::{Mat4, Vec2, Vec3, Vec4, Vec4Swizzles};
use rgfx_core::{Camera, Color, Error, Framebuffer, Result, Scene, SceneRenderer, Vertex};

/// The depth value of a cleared framebuffer (the far plane), matching `rgfx_core`'s framebuffer
/// contract. Fragments must be strictly nearer than this (and any prior fragment) to be written.
const DEPTH_FAR: f32 = 1.0;

/// Which triangle winding, in normalized device coordinates, counts as the front face.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrontFace {
    /// Counter-clockwise vertices (in NDC, `+Y` up) are front-facing. This is the OpenGL default.
    CounterClockwise,
    /// Clockwise vertices (in NDC, `+Y` up) are front-facing.
    Clockwise,
}

/// Which triangles to discard based on their facing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cull {
    /// Draw both front and back faces.
    None,
    /// Discard back faces (the usual choice for solid, closed meshes).
    Back,
    /// Discard front faces.
    Front,
}

/// How the rasterizer colors each covered fragment.
///
/// Lighting-based modes (flat and smooth shading) are added in a later task; this enum is
/// deliberately marked non-exhaustive and its fragment stage kept simple so those variants slot
/// in without reworking the pipeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ShadingMode {
    /// Fill every fragment with the rasterizer's flat base color (no lighting).
    Unlit,
    /// Visualize depth as grayscale: nearer fragments are brighter, farther ones darker.
    Depth,
    /// Visualize the interpolated surface normal as an RGB color (`n * 0.5 + 0.5`).
    Normals,
}

/// A CPU software rasterizer that renders a [`Scene`] into a [`Framebuffer`].
///
/// Construct one with [`Rasterizer::new`] (or [`Default`]) and tweak the public fields, then call
/// [`SceneRenderer::render`]. The renderer clears `target` before drawing, using and updating its
/// depth buffer for hidden-surface removal.
#[derive(Clone, Copy, Debug)]
pub struct Rasterizer {
    /// How covered fragments are colored.
    pub shading: ShadingMode,
    /// Which NDC winding is the front face.
    pub front_face: FrontFace,
    /// Which faces to discard.
    pub cull: Cull,
    /// The flat color used by [`ShadingMode::Unlit`] and as the clear color's opaque base.
    pub base_color: Color,
    /// The color the framebuffer is cleared to before rendering.
    pub clear_color: Color,
}

impl Default for Rasterizer {
    fn default() -> Self {
        Self::new(ShadingMode::Unlit)
    }
}

impl Rasterizer {
    /// Creates a rasterizer with the given shading mode and sensible defaults: counter-clockwise
    /// front faces, back-face culling, an opaque-white base color, and a transparent clear color.
    pub fn new(shading: ShadingMode) -> Self {
        Self {
            shading,
            front_face: FrontFace::CounterClockwise,
            cull: Cull::Back,
            base_color: Color::WHITE,
            clear_color: Color::TRANSPARENT,
        }
    }

    /// Renders a single triangle's three transformed clip-space vertices, performing near-plane
    /// clipping, the viewport transform, culling, and the depth-tested fill.
    fn draw_clip_triangle(&self, tri: [ClipVertex; 3], target: &mut Framebuffer) {
        let (poly, n) = clip_triangle_near(&tri);
        if n < 3 {
            return;
        }
        let (w, h) = (target.width(), target.height());
        if w == 0 || h == 0 {
            return;
        }
        // Project the clipped polygon to screen space once, then fan-triangulate it. A single
        // near-plane clip of a triangle yields at most four vertices.
        let mut screen = [ScreenVertex::default(); 4];
        for k in 0..n {
            match project(&poly[k], w, h) {
                Some(sv) => screen[k] = sv,
                None => return, // non-finite projection: drop the whole polygon defensively
            }
        }
        for i in 1..n - 1 {
            self.fill_triangle([screen[0], screen[i], screen[i + 1]], target);
        }
    }

    /// Rasterizes one screen-space triangle with the top-left fill rule and depth test.
    fn fill_triangle(&self, verts: [ScreenVertex; 3], target: &mut Framebuffer) {
        let (w, h) = (target.width(), target.height());
        let p = [verts[0].pos, verts[1].pos, verts[2].pos];

        // Signed area (twice) in screen space (Y-down). `orient` is positive for vertices that are
        // counter-clockwise in a Y-up frame, i.e. clockwise on screen.
        let area = orient(p[0], p[1], p[2]);
        if area == 0.0 || !area.is_finite() {
            return; // degenerate (zero-area) triangle
        }

        // Backface culling by facing. A front CCW-in-NDC triangle becomes CW on the Y-down screen,
        // giving a negative `orient`.
        let front_is_negative = self.front_face == FrontFace::CounterClockwise;
        let is_front = if front_is_negative {
            area < 0.0
        } else {
            area > 0.0
        };
        match self.cull {
            Cull::Back if !is_front => return,
            Cull::Front if is_front => return,
            _ => {}
        }

        // Reorient to positive area so the top-left rule and barycentric weights are consistent,
        // swapping attributes along with positions.
        let (v, area) = if area < 0.0 {
            ([verts[0], verts[2], verts[1]], -area)
        } else {
            (verts, area)
        };
        let p = [v[0].pos, v[1].pos, v[2].pos];
        let inv_area = 1.0 / area;

        // Which edges are top-left (fill boundary fragments that lie exactly on them).
        let tl = [
            is_top_left(p[1], p[2]),
            is_top_left(p[2], p[0]),
            is_top_left(p[0], p[1]),
        ];

        // Bounding box, clamped to the framebuffer.
        let min_x = p[0].x.min(p[1].x).min(p[2].x).floor().max(0.0) as usize;
        let min_y = p[0].y.min(p[1].y).min(p[2].y).floor().max(0.0) as usize;
        let max_x = (p[0].x.max(p[1].x).max(p[2].x).ceil() as usize).min(w.saturating_sub(1));
        let max_y = (p[0].y.max(p[1].y).max(p[2].y).ceil() as usize).min(h.saturating_sub(1));
        if min_x > max_x || min_y > max_y {
            return;
        }

        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let sample = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                let w0 = orient(p[1], p[2], sample);
                let w1 = orient(p[2], p[0], sample);
                let w2 = orient(p[0], p[1], sample);

                let inside = edge_ok(w0, tl[0]) && edge_ok(w1, tl[1]) && edge_ok(w2, tl[2]);
                if !inside {
                    continue;
                }

                let (b0, b1, b2) = (w0 * inv_area, w1 * inv_area, w2 * inv_area);

                // Depth is affine in screen space, so plain barycentric interpolation is exact.
                let frag_depth = b0 * v[0].depth + b1 * v[1].depth + b2 * v[2].depth;
                if !(0.0..=DEPTH_FAR).contains(&frag_depth) {
                    continue;
                }

                let idx = y * w + x;
                // Depth test: only nearer fragments win. Borrow depth per pixel so the color write
                // below does not conflict.
                if frag_depth >= target.depth()[idx] {
                    continue;
                }

                let color = self.shade(&v, (b0, b1, b2), frag_depth);
                target.depth_mut()[idx] = frag_depth;
                target.set(x, y, color);
            }
        }
    }

    /// Computes the fragment color for the current shading mode.
    fn shade(&self, v: &[ScreenVertex; 3], bary: (f32, f32, f32), frag_depth: f32) -> Color {
        match self.shading {
            ShadingMode::Unlit => {
                Color::new(self.base_color.r, self.base_color.g, self.base_color.b, 1.0)
            }
            ShadingMode::Depth => {
                let g = (1.0 - frag_depth).clamp(0.0, 1.0);
                Color::rgb(g, g, g)
            }
            ShadingMode::Normals => {
                let n = perspective_normal(v, bary);
                Color::rgb(n.x * 0.5 + 0.5, n.y * 0.5 + 0.5, n.z * 0.5 + 0.5)
            }
        }
    }
}

impl SceneRenderer for Rasterizer {
    fn render(&mut self, scene: &Scene, camera: &Camera, target: &mut Framebuffer) -> Result<()> {
        target.clear(self.clear_color);
        let view_proj = camera.view_projection();
        for mesh in &scene.meshes {
            let verts = &mesh.vertices;
            for tri in mesh.indices.chunks_exact(3) {
                let a = fetch(verts, tri[0])?;
                let b = fetch(verts, tri[1])?;
                let c = fetch(verts, tri[2])?;
                let clip = [
                    ClipVertex::new(view_proj, a),
                    ClipVertex::new(view_proj, b),
                    ClipVertex::new(view_proj, c),
                ];
                self.draw_clip_triangle(clip, target);
            }
        }
        Ok(())
    }
}

/// Looks up a vertex by index, returning a geometry error rather than panicking on an out-of-range
/// index (malformed input must never crash the renderer).
fn fetch(verts: &[Vertex], index: u32) -> Result<Vertex> {
    verts
        .get(index as usize)
        .copied()
        .ok_or_else(|| Error::Geometry(format!("triangle index {index} out of range")))
}

/// A vertex in homogeneous clip space, carrying the attributes interpolated during clipping.
#[derive(Clone, Copy, Debug, Default)]
struct ClipVertex {
    clip: Vec4,
    normal: Vec3,
}

impl ClipVertex {
    fn new(view_proj: Mat4, v: Vertex) -> Self {
        Self {
            clip: view_proj * v.position.extend(1.0),
            normal: v.normal,
        }
    }

    /// Linearly interpolates between two clip-space vertices at parameter `t` (used by clipping;
    /// linear interpolation in clip space is exact for the clip intersection point).
    fn lerp(self, other: Self, t: f32) -> Self {
        Self {
            clip: self.clip.lerp(other.clip, t),
            normal: self.normal.lerp(other.normal, t),
        }
    }
}

/// A vertex after the perspective divide and viewport transform, ready to rasterize.
#[derive(Clone, Copy, Debug, Default)]
struct ScreenVertex {
    /// Pixel-space position.
    pos: Vec2,
    /// Depth in `0.0..=1.0` (near..far), affine in screen space.
    depth: f32,
    /// Reciprocal clip `w`, for perspective-correct attribute interpolation.
    inv_w: f32,
    /// World-space normal scaled by `inv_w` (so a plain barycentric sum recovers the
    /// perspective-correct value after multiplying by the interpolated `w`).
    normal_over_w: Vec3,
}

/// Clips a triangle against the near plane (`z_clip >= 0`) using Sutherland–Hodgman, returning the
/// resulting polygon (0, 3, or 4 vertices) in a fixed-size buffer. Keeping every surviving vertex
/// at `z_clip >= 0` guarantees `w > 0`, so the later perspective divide can never divide by zero.
fn clip_triangle_near(tri: &[ClipVertex; 3]) -> ([ClipVertex; 4], usize) {
    let mut out = [ClipVertex::default(); 4];
    let mut n = 0;
    for i in 0..3 {
        let cur = tri[i];
        let nxt = tri[(i + 1) % 3];
        let dc = cur.clip.z;
        let dn = nxt.clip.z;
        let cur_in = dc >= 0.0;
        let nxt_in = dn >= 0.0;
        if cur_in {
            out[n] = cur;
            n += 1;
        }
        if cur_in != nxt_in {
            // The edge crosses the plane; add the intersection. `dc - dn` is non-zero because the
            // endpoints are on opposite sides.
            let t = dc / (dc - dn);
            out[n] = cur.lerp(nxt, t);
            n += 1;
        }
    }
    (out, n)
}

/// Applies the perspective divide and viewport transform, producing a [`ScreenVertex`]. Returns
/// `None` if the result is not finite (a degenerate transform), so callers can drop it.
fn project(v: &ClipVertex, width: usize, height: usize) -> Option<ScreenVertex> {
    let w = v.clip.w;
    if w <= 0.0 || !w.is_finite() {
        return None;
    }
    let inv_w = 1.0 / w;
    let ndc = v.clip.xyz() * inv_w;
    let sx = (ndc.x * 0.5 + 0.5) * width as f32;
    // Flip Y so +Y in NDC is up on screen (row 0 is the top).
    let sy = (1.0 - (ndc.y * 0.5 + 0.5)) * height as f32;
    let sv = ScreenVertex {
        pos: Vec2::new(sx, sy),
        depth: ndc.z,
        inv_w,
        normal_over_w: v.normal * inv_w,
    };
    if sv.pos.is_finite() && sv.depth.is_finite() {
        Some(sv)
    } else {
        None
    }
}

/// Recovers the perspective-correct interpolated surface normal at a fragment.
fn perspective_normal(v: &[ScreenVertex; 3], bary: (f32, f32, f32)) -> Vec3 {
    let (b0, b1, b2) = bary;
    let inv_w = b0 * v[0].inv_w + b1 * v[1].inv_w + b2 * v[2].inv_w;
    if inv_w.abs() <= f32::EPSILON {
        return Vec3::ZERO;
    }
    let n = (b0 * v[0].normal_over_w + b1 * v[1].normal_over_w + b2 * v[2].normal_over_w) / inv_w;
    n.normalize_or_zero()
}

/// Twice the signed area of triangle `(a, b, c)` — the 2D cross product of `b - a` and `c - a`.
/// Also used as an edge function: `orient(a, b, p)` is positive when `p` is left of the directed
/// edge `a → b` in a Y-up frame.
#[inline]
fn orient(a: Vec2, b: Vec2, c: Vec2) -> f32 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

/// Whether a fragment with edge value `w` (for a positively-oriented triangle) is covered: strictly
/// inside, or exactly on a top-left edge.
#[inline]
fn edge_ok(w: f32, top_left: bool) -> bool {
    w > 0.0 || (w == 0.0 && top_left)
}

/// Whether the directed edge `a → b` is a top or left edge of a positively-oriented (`orient > 0`)
/// screen-space triangle, per the standard top-left fill rule. In this orientation (CW on the
/// Y-down screen) a top edge is horizontal and runs right-to-left; a left edge runs upward.
#[inline]
fn is_top_left(a: Vec2, b: Vec2) -> bool {
    let edge = b - a;
    let is_top = edge.y == 0.0 && edge.x < 0.0;
    let is_left = edge.y < 0.0;
    is_top || is_left
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
    use rgfx_core::{Mesh, Projection, Scene};

    // --- Low-level fill: exact pixel coverage -----------------------------------------------------

    /// Builds a screen-space vertex directly (orthographic-style: `w = 1`, depth constant).
    fn sv(x: f32, y: f32, depth: f32) -> ScreenVertex {
        ScreenVertex {
            pos: Vec2::new(x, y),
            depth,
            inv_w: 1.0,
            normal_over_w: Vec3::Z,
        }
    }

    fn opaque_pixels(fb: &Framebuffer) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for y in 0..fb.height() {
            for x in 0..fb.width() {
                if fb.get(x, y).a > 0.0 {
                    out.push((x, y));
                }
            }
        }
        out
    }

    #[test]
    fn single_triangle_fills_expected_pixel_set() {
        let mut fb = Framebuffer::new(8, 8);
        let r = Rasterizer {
            cull: Cull::None,
            ..Rasterizer::new(ShadingMode::Unlit)
        };
        // A right triangle with the right-angle at (1,1); hypotenuse from (5,1)->(1,5).
        let tri = [sv(1.0, 1.0, 0.5), sv(5.0, 1.0, 0.5), sv(1.0, 5.0, 0.5)];
        r.fill_triangle(tri, &mut fb);

        // Pixel centers strictly inside must be covered.
        for &(x, y) in &[(1usize, 1usize), (2, 1), (1, 2), (2, 2), (1, 3)] {
            assert!(fb.get(x, y).a > 0.0, "expected ({x},{y}) inside");
        }
        // Pixels clearly outside the triangle must not be.
        for &(x, y) in &[(5usize, 5usize), (4, 4), (0, 0), (6, 1), (1, 6)] {
            assert!(fb.get(x, y).a == 0.0, "expected ({x},{y}) outside");
        }
    }

    #[test]
    fn shared_edge_has_no_gaps_or_double_cover() {
        // Two triangles split a rectangle along a diagonal; each interior pixel must be covered by
        // exactly one of them (top-left rule => watertight).
        let (w, h) = (16usize, 16usize);
        let r = Rasterizer {
            cull: Cull::None,
            ..Rasterizer::new(ShadingMode::Unlit)
        };
        let (p00, p10) = (sv(2.0, 2.0, 0.5), sv(12.0, 2.0, 0.5));
        let (p01, p11) = (sv(2.0, 12.0, 0.5), sv(12.0, 12.0, 0.5));

        let mut fb_a = Framebuffer::new(w, h);
        let mut fb_b = Framebuffer::new(w, h);
        r.fill_triangle([p00, p10, p01], &mut fb_a);
        r.fill_triangle([p10, p11, p01], &mut fb_b);

        for y in 0..h {
            for x in 0..w {
                let a = fb_a.get(x, y).a > 0.0;
                let b = fb_b.get(x, y).a > 0.0;
                assert!(!(a && b), "double cover at ({x},{y})");
            }
        }
        // No gaps in the interior of the rectangle.
        for y in 3..11 {
            for x in 3..11 {
                let a = fb_a.get(x, y).a > 0.0;
                let b = fb_b.get(x, y).a > 0.0;
                assert!(a || b, "gap at ({x},{y})");
            }
        }
    }

    #[test]
    fn nearer_triangle_occludes_farther() {
        let mut fb = Framebuffer::new(8, 8);
        let r = Rasterizer {
            cull: Cull::None,
            base_color: Color::rgb(1.0, 0.0, 0.0),
            ..Rasterizer::new(ShadingMode::Unlit)
        };
        let far = [sv(0.0, 0.0, 0.9), sv(8.0, 0.0, 0.9), sv(0.0, 8.0, 0.9)];
        r.fill_triangle(far, &mut fb);
        let mut r2 = r;
        r2.base_color = Color::rgb(0.0, 1.0, 0.0);
        let near = [sv(0.0, 0.0, 0.2), sv(8.0, 0.0, 0.2), sv(0.0, 8.0, 0.2)];
        r2.fill_triangle(near, &mut fb);

        // Overlap region should show the nearer (green) triangle and its depth.
        let c = fb.get(1, 1);
        assert!(
            c.g > 0.5 && c.r < 0.5,
            "nearer triangle must win the depth test"
        );
        assert!((fb.depth()[8 + 1] - 0.2).abs() < 1e-6);

        // Drawing the far triangle again must not overwrite the nearer one.
        let mut r3 = r;
        r3.base_color = Color::rgb(0.0, 0.0, 1.0);
        r3.fill_triangle(far, &mut fb);
        let c = fb.get(1, 1);
        assert!(c.g > 0.5, "farther triangle must not overwrite nearer");
    }

    #[test]
    fn backface_culling_respects_winding() {
        // Screen-space CCW triangle (Y-down): (0,0) -> (0,8) -> (8,0).
        let ccw = [sv(0.0, 0.0, 0.5), sv(0.0, 8.0, 0.5), sv(8.0, 0.0, 0.5)];
        // Reverse winding.
        let cw = [sv(0.0, 0.0, 0.5), sv(8.0, 0.0, 0.5), sv(0.0, 8.0, 0.5)];

        // With CCW-in-NDC as front and back-face culling, exactly one of these is drawn.
        let front = Rasterizer {
            cull: Cull::Back,
            front_face: FrontFace::CounterClockwise,
            ..Rasterizer::new(ShadingMode::Unlit)
        };
        let mut fb_ccw = Framebuffer::new(8, 8);
        let mut fb_cw = Framebuffer::new(8, 8);
        front.fill_triangle(ccw, &mut fb_ccw);
        front.fill_triangle(cw, &mut fb_cw);
        let drawn_ccw = !opaque_pixels(&fb_ccw).is_empty();
        let drawn_cw = !opaque_pixels(&fb_cw).is_empty();
        assert!(
            drawn_ccw != drawn_cw,
            "exactly one winding survives back-face cull"
        );

        // Flipping the front face flips which one survives.
        let flipped = Rasterizer {
            front_face: FrontFace::Clockwise,
            ..front
        };
        let mut fb2 = Framebuffer::new(8, 8);
        flipped.fill_triangle(if drawn_ccw { ccw } else { cw }, &mut fb2);
        assert!(
            opaque_pixels(&fb2).is_empty(),
            "the face that was front is now culled"
        );
    }

    // --- Clipping ---------------------------------------------------------------------------------

    #[test]
    fn near_clip_drops_fully_behind_triangle() {
        let behind = ClipVertex {
            clip: Vec4::new(0.0, 0.0, -1.0, 1.0),
            normal: Vec3::Z,
        };
        let (_, n) = clip_triangle_near(&[behind, behind, behind]);
        assert_eq!(n, 0, "a triangle fully behind the near plane is removed");
    }

    #[test]
    fn near_clip_splits_crossing_triangle_into_quad() {
        // Two vertices in front (z >= 0), one behind: clipping yields a 4-vertex polygon.
        let front_a = ClipVertex {
            clip: Vec4::new(-1.0, -1.0, 1.0, 1.0),
            normal: Vec3::Z,
        };
        let front_b = ClipVertex {
            clip: Vec4::new(1.0, -1.0, 1.0, 1.0),
            normal: Vec3::Z,
        };
        let behind = ClipVertex {
            clip: Vec4::new(0.0, 1.0, -1.0, 1.0),
            normal: Vec3::Z,
        };
        let (poly, n) = clip_triangle_near(&[front_a, front_b, behind]);
        assert_eq!(n, 4);
        for v in poly.iter().take(n) {
            assert!(
                v.clip.z >= -1e-6,
                "clipped vertices stay in front of near plane"
            );
        }
    }

    // --- Full pipeline through SceneRenderer ------------------------------------------------------

    fn unit_cube() -> Mesh {
        // 24 vertices (per-face normals), 12 triangles, wound CCW when viewed from outside.
        let faces: [(Vec3, [Vec3; 4]); 6] = [
            (
                Vec3::Z,
                [
                    Vec3::new(-1.0, -1.0, 1.0),
                    Vec3::new(1.0, -1.0, 1.0),
                    Vec3::new(1.0, 1.0, 1.0),
                    Vec3::new(-1.0, 1.0, 1.0),
                ],
            ),
            (
                Vec3::NEG_Z,
                [
                    Vec3::new(1.0, -1.0, -1.0),
                    Vec3::new(-1.0, -1.0, -1.0),
                    Vec3::new(-1.0, 1.0, -1.0),
                    Vec3::new(1.0, 1.0, -1.0),
                ],
            ),
            (
                Vec3::X,
                [
                    Vec3::new(1.0, -1.0, 1.0),
                    Vec3::new(1.0, -1.0, -1.0),
                    Vec3::new(1.0, 1.0, -1.0),
                    Vec3::new(1.0, 1.0, 1.0),
                ],
            ),
            (
                Vec3::NEG_X,
                [
                    Vec3::new(-1.0, -1.0, -1.0),
                    Vec3::new(-1.0, -1.0, 1.0),
                    Vec3::new(-1.0, 1.0, 1.0),
                    Vec3::new(-1.0, 1.0, -1.0),
                ],
            ),
            (
                Vec3::Y,
                [
                    Vec3::new(-1.0, 1.0, 1.0),
                    Vec3::new(1.0, 1.0, 1.0),
                    Vec3::new(1.0, 1.0, -1.0),
                    Vec3::new(-1.0, 1.0, -1.0),
                ],
            ),
            (
                Vec3::NEG_Y,
                [
                    Vec3::new(-1.0, -1.0, -1.0),
                    Vec3::new(1.0, -1.0, -1.0),
                    Vec3::new(1.0, -1.0, 1.0),
                    Vec3::new(-1.0, -1.0, 1.0),
                ],
            ),
        ];
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        for (normal, quad) in faces {
            let base = vertices.len() as u32;
            for &p in &quad {
                vertices.push(Vertex::new(p, normal));
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        Mesh::new(vertices, indices)
    }

    fn cube_scene() -> Scene {
        Scene::new("cube", vec![unit_cube()])
    }

    fn front_camera() -> Camera {
        let mut cam = Camera::perspective(1.0, 60_f32.to_radians());
        cam.position = Vec3::new(0.0, 0.0, 5.0);
        cam.target = Vec3::ZERO;
        cam
    }

    #[test]
    fn renders_cube_depth_into_small_framebuffer() {
        let mut fb = Framebuffer::new(24, 24);
        let mut r = Rasterizer::new(ShadingMode::Depth);
        r.render(&cube_scene(), &front_camera(), &mut fb).unwrap();

        let lit = opaque_pixels(&fb);
        assert!(!lit.is_empty(), "cube must cover some pixels");
        // The center pixel faces the camera and must be nearer than the far plane.
        let center = fb.depth()[12 * 24 + 12];
        assert!(
            center < DEPTH_FAR,
            "center of cube must be in front of far plane"
        );
        // Depth shading makes the front face non-black (nearer => brighter).
        assert!(fb.get(12, 12).luma() > 0.0);
    }

    #[test]
    fn renders_cube_normals_face_toward_camera() {
        let mut fb = Framebuffer::new(24, 24);
        let mut r = Rasterizer::new(ShadingMode::Normals);
        r.render(&cube_scene(), &front_camera(), &mut fb).unwrap();

        // The visible face is +Z; encoded as normal * 0.5 + 0.5 that is blue ~1.0, r/g ~0.5.
        let c = fb.get(12, 12);
        assert!(
            c.b > 0.9,
            "front (+Z) face should encode blue-ish, got {c:?}"
        );
        assert!((c.r - 0.5).abs() < 0.1 && (c.g - 0.5).abs() < 0.1);
    }

    #[test]
    fn offscreen_and_empty_targets_do_not_panic() {
        // Empty framebuffer.
        let mut empty = Framebuffer::new(0, 0);
        let mut r = Rasterizer::new(ShadingMode::Unlit);
        r.render(&cube_scene(), &front_camera(), &mut empty)
            .unwrap();

        // Camera looking away from the cube: nothing on screen, no panic, no writes.
        let mut fb = Framebuffer::new(16, 16);
        let mut away = front_camera();
        away.position = Vec3::new(0.0, 0.0, 5.0);
        away.target = Vec3::new(0.0, 0.0, 50.0); // look away from origin
        r.render(&cube_scene(), &away, &mut fb).unwrap();
        // Nothing should be nearer than far.
        assert!(fb.depth().iter().all(|&d| d == DEPTH_FAR));
    }

    #[test]
    fn out_of_range_index_errors_without_panicking() {
        let mesh = Mesh::new(vec![Vertex::from_position(Vec3::ZERO)], vec![0, 1, 2]);
        let scene = Scene::new("bad", vec![mesh]);
        let mut fb = Framebuffer::new(4, 4);
        let mut r = Rasterizer::new(ShadingMode::Unlit);
        let err = r.render(&scene, &front_camera(), &mut fb).unwrap_err();
        assert!(matches!(err, Error::Geometry(_)));
    }

    #[test]
    fn orthographic_camera_renders_without_divide_issues() {
        let mut fb = Framebuffer::new(16, 16);
        let mut cam = Camera::orthographic(1.0, 2.0);
        cam.position = Vec3::new(0.0, 0.0, 5.0);
        let mut r = Rasterizer::new(ShadingMode::Unlit);
        assert!(matches!(cam.projection, Projection::Orthographic { .. }));
        r.render(&cube_scene(), &cam, &mut fb).unwrap();
        assert!(!opaque_pixels(&fb).is_empty());
    }
}
