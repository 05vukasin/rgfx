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

use std::collections::HashSet;

use glam::{Mat4, Vec2, Vec3, Vec4, Vec4Swizzles};
use rgfx_core::{Camera, Color, Error, Framebuffer, Result, Scene, SceneRenderer, Vertex};

use crate::line::draw_line;

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
/// The lit modes ([`ShadingMode::Flat`] and [`ShadingMode::Smooth`]) apply an ambient +
/// directional-diffuse (Lambert) term using the rasterizer's [`Rasterizer::light_direction`],
/// [`Rasterizer::ambient`], and [`Rasterizer::base_color`]. Flat lights each triangle by its face
/// normal (a hard-faceted look); smooth lights each fragment by the perspective-correct
/// interpolation of the per-vertex normals (a rounded look). When a mesh supplies no per-vertex
/// normals, both modes fall back to the triangle's geometric face normal.
///
/// This enum is marked non-exhaustive: new variants may be added without a breaking change, so
/// external `match`es must include a wildcard arm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ShadingMode {
    /// Fill every fragment with the rasterizer's flat base color (no lighting).
    Unlit,
    /// Lambert-shade each triangle by its single face normal (hard, faceted lighting).
    Flat,
    /// Lambert-shade each fragment by the perspective-correct interpolation of the per-vertex
    /// normals (smooth, Gouraud-style lighting with no visible faceting on a dense mesh).
    Smooth,
    /// Visualize the interpolated surface normal as an RGB color (`n * 0.5 + 0.5`).
    Normals,
    /// Visualize depth as grayscale: nearer fragments are brighter, farther ones darker.
    Depth,
    /// Draw only the mesh edges as lines in the base color, leaving faces empty.
    ///
    /// Unlike the filled modes, wireframe has a dedicated pass (see
    /// [`SceneRenderer::render`]): it deduplicates the triangle edges, clips each edge segment to
    /// the near plane, projects it, and rasterizes it with [`crate::draw_line`]. It performs no
    /// depth test, so every edge is drawn (no hidden-line removal).
    Wireframe,
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
    /// The surface color lit by the shading modes, and the flat color used by
    /// [`ShadingMode::Unlit`].
    pub base_color: Color,
    /// The color the framebuffer is cleared to before rendering.
    pub clear_color: Color,
    /// World-space direction *toward* the directional light. The diffuse term is
    /// `max(0, dot(normal, light_direction))`, so surfaces whose normal points along this
    /// direction are brightest. Need not be a unit vector — it is normalized when shading. Used by
    /// [`ShadingMode::Flat`] and [`ShadingMode::Smooth`].
    pub light_direction: Vec3,
    /// Ambient light level in `0.0..=1.0`, added to the diffuse term before clamping. Acts as a
    /// floor so faces turned away from the light are never fully black.
    pub ambient: f32,
}

impl Default for Rasterizer {
    fn default() -> Self {
        Self::new(ShadingMode::Unlit)
    }
}

impl Rasterizer {
    /// Creates a rasterizer with the given shading mode and sensible defaults: counter-clockwise
    /// front faces, back-face culling, an opaque-white base color, a transparent clear color, a
    /// directional light coming from the upper-front-right, and a low ambient floor.
    pub fn new(shading: ShadingMode) -> Self {
        Self {
            shading,
            front_face: FrontFace::CounterClockwise,
            cull: Cull::Back,
            base_color: Color::WHITE,
            clear_color: Color::TRANSPARENT,
            light_direction: Vec3::new(0.5, 0.7, 1.0).normalize(),
            ambient: 0.15,
        }
    }

    /// Sets the directional light direction, normalizing it. A zero vector is ignored so the
    /// existing direction is kept (shading never divides by a zero-length light).
    pub fn set_light_direction(&mut self, direction: Vec3) {
        let n = direction.normalize_or_zero();
        if n != Vec3::ZERO {
            self.light_direction = n;
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
            ShadingMode::Unlit => self.base_opaque(),
            ShadingMode::Flat => {
                // One face normal shared by all three vertices: hard-faceted lighting.
                let intensity = lambert(v[0].face_normal, self.light_direction, self.ambient);
                self.lit_color(intensity)
            }
            ShadingMode::Smooth => {
                let n = perspective_normal(v, bary);
                let intensity = lambert(n, self.light_direction, self.ambient);
                self.lit_color(intensity)
            }
            ShadingMode::Normals => {
                let n = perspective_normal(v, bary);
                Color::rgb(n.x * 0.5 + 0.5, n.y * 0.5 + 0.5, n.z * 0.5 + 0.5)
            }
            ShadingMode::Depth => {
                let g = (1.0 - frag_depth).clamp(0.0, 1.0);
                Color::rgb(g, g, g)
            }
            // Wireframe is drawn by a dedicated edge pass in `render`, so this fill path is not
            // reached for it; the arm keeps `shade` total and paints the base color as a
            // defensive fallback.
            ShadingMode::Wireframe => self.base_opaque(),
        }
    }

    /// The opaque base color (alpha forced to 1.0), used by the unlit and placeholder modes.
    fn base_opaque(&self) -> Color {
        Color::rgb(self.base_color.r, self.base_color.g, self.base_color.b)
    }

    /// The base color scaled by a diffuse `intensity` in `0.0..=1.0`, kept opaque.
    fn lit_color(&self, intensity: f32) -> Color {
        Color::rgb(
            self.base_color.r * intensity,
            self.base_color.g * intensity,
            self.base_color.b * intensity,
        )
    }
}

/// The Lambert diffuse intensity for a surface `normal` lit by a directional light shining from
/// `light_direction`, with an `ambient` floor: `clamp(ambient + max(0, dot(n, l)), 0, 1)`.
///
/// Both vectors are normalized defensively, so a zero-length or unnormalized input yields a
/// finite result (the ambient level for a zero vector) rather than a NaN.
fn lambert(normal: Vec3, light_direction: Vec3, ambient: f32) -> f32 {
    let n = normal.normalize_or_zero();
    let l = light_direction.normalize_or_zero();
    let diffuse = n.dot(l).max(0.0);
    (ambient + diffuse).clamp(0.0, 1.0)
}

/// The geometric (face) normal of triangle `(a, b, c)` via the right-hand rule on its winding,
/// or `Vec3::ZERO` for a degenerate triangle. Used for flat shading and as the fallback normal
/// when a mesh supplies no per-vertex normals.
fn geometric_normal(a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    (b - a).cross(c - a).normalize_or_zero()
}

impl Rasterizer {
    /// Renders `scene` as a wireframe: deduplicated mesh edges drawn as lines in the base color.
    ///
    /// This is the pass used by [`SceneRenderer::render`] when [`Rasterizer::shading`] is
    /// [`ShadingMode::Wireframe`]. It clears `target`, then for each mesh transforms and
    /// near-clips every unique triangle edge, projects it, and rasterizes it with
    /// [`crate::draw_line`]. No depth test is applied (all edges are drawn). An out-of-range
    /// triangle index yields an [`Error::Geometry`] rather than a panic.
    pub fn render_wireframe(
        &self,
        scene: &Scene,
        camera: &Camera,
        target: &mut Framebuffer,
    ) -> Result<()> {
        target.clear(self.clear_color);
        let (w, h) = (target.width(), target.height());
        if w == 0 || h == 0 {
            return Ok(());
        }
        let view_proj = camera.view_projection();
        let color = self.base_opaque();
        for mesh in &scene.meshes {
            let verts = &mesh.vertices;
            for (i0, i1) in dedup_edges(&mesh.indices) {
                let a = fetch(verts, i0)?;
                let b = fetch(verts, i1)?;
                let ca = view_proj * a.position.extend(1.0);
                let cb = view_proj * b.position.extend(1.0);
                let Some((ca, cb)) = clip_segment_near(ca, cb) else {
                    continue;
                };
                let (Some(pa), Some(pb)) = (project_clip(ca, w, h), project_clip(cb, w, h)) else {
                    continue;
                };
                draw_line(target, pa.0, pa.1, pb.0, pb.1, color);
            }
        }
        Ok(())
    }
}

impl SceneRenderer for Rasterizer {
    fn render(&mut self, scene: &Scene, camera: &Camera, target: &mut Framebuffer) -> Result<()> {
        if self.shading == ShadingMode::Wireframe {
            return self.render_wireframe(scene, camera, target);
        }
        target.clear(self.clear_color);
        let view_proj = camera.view_projection();
        for mesh in &scene.meshes {
            let verts = &mesh.vertices;
            for tri in mesh.indices.chunks_exact(3) {
                let a = fetch(verts, tri[0])?;
                let b = fetch(verts, tri[1])?;
                let c = fetch(verts, tri[2])?;
                // Face normal from geometry (world space; mesh transforms are baked into
                // positions). Flip it for a clockwise front-face convention so it points toward
                // the front of the surface, and reuse it as the fallback per-vertex normal for
                // meshes that ship without normals.
                let mut face_normal = geometric_normal(a.position, b.position, c.position);
                if self.front_face == FrontFace::Clockwise {
                    face_normal = -face_normal;
                }
                let clip = [
                    ClipVertex::new(view_proj, a, face_normal),
                    ClipVertex::new(view_proj, b, face_normal),
                    ClipVertex::new(view_proj, c, face_normal),
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

/// Collects the unique undirected edges of a triangle-index list for wireframe drawing.
///
/// Each triangle contributes its three edges; an edge is keyed by its ordered `(min, max)` vertex
/// index pair so an edge shared by two triangles (a quad's interior diagonal, or a seam between
/// adjacent triangles that reuse vertices) is emitted only once. Edges are returned in first-seen
/// order. A trailing partial triple (indices not a multiple of three) is ignored.
fn dedup_edges(indices: &[u32]) -> Vec<(u32, u32)> {
    let mut seen = HashSet::new();
    let mut edges = Vec::new();
    for tri in indices.chunks_exact(3) {
        for &(a, b) in &[(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
            let key = if a <= b { (a, b) } else { (b, a) };
            if seen.insert(key) {
                edges.push(key);
            }
        }
    }
    edges
}

/// Clips a clip-space segment against the near plane (`clip.z >= 0`, matching
/// [`clip_triangle_near`]), returning the visible portion or `None` if the whole segment is behind
/// the plane. Keeping both surviving endpoints at `z >= 0` guarantees `w > 0`, so the subsequent
/// perspective divide in [`project_clip`] never divides by zero.
fn clip_segment_near(a: Vec4, b: Vec4) -> Option<(Vec4, Vec4)> {
    let (za, zb) = (a.z, b.z);
    match (za >= 0.0, zb >= 0.0) {
        (false, false) => None,
        (true, true) => Some((a, b)),
        // Exactly one endpoint is behind; move it to the plane crossing. `za - zb` is non-zero
        // because the endpoints straddle the plane.
        (true, false) => {
            let t = za / (za - zb);
            Some((a, a.lerp(b, t)))
        }
        (false, true) => {
            let t = za / (za - zb);
            Some((a.lerp(b, t), b))
        }
    }
}

/// Applies the perspective divide and viewport transform to a clip-space point, returning rounded
/// integer pixel coordinates. Returns `None` for a point on or behind the camera plane, or a
/// non-finite projection, so callers can drop the segment.
fn project_clip(clip: Vec4, width: usize, height: usize) -> Option<(i32, i32)> {
    let w = clip.w;
    if w <= 0.0 || !w.is_finite() {
        return None;
    }
    let ndc = clip.xyz() / w;
    let sx = (ndc.x * 0.5 + 0.5) * width as f32;
    // Flip Y so +Y in NDC is up on screen (row 0 is the top).
    let sy = (1.0 - (ndc.y * 0.5 + 0.5)) * height as f32;
    if !sx.is_finite() || !sy.is_finite() {
        return None;
    }
    Some((sx.round() as i32, sy.round() as i32))
}

/// A vertex in homogeneous clip space, carrying the attributes interpolated during clipping.
#[derive(Clone, Copy, Debug, Default)]
struct ClipVertex {
    clip: Vec4,
    /// The per-vertex shading normal (world space), used for smooth shading and the normals debug
    /// mode. Falls back to the triangle's face normal when the source vertex has none.
    normal: Vec3,
    /// The triangle's face normal (world space), constant across the triangle, used by flat
    /// shading. Interpolation during clipping preserves it since all three vertices share it.
    face_normal: Vec3,
}

impl ClipVertex {
    fn new(view_proj: Mat4, v: Vertex, face_normal: Vec3) -> Self {
        // Use the supplied per-vertex normal when present, otherwise generate one from geometry by
        // adopting the triangle's face normal (so meshes without normals still shade).
        let normal = if v.normal.length_squared() > 1e-12 {
            v.normal.normalize()
        } else {
            face_normal
        };
        Self {
            clip: view_proj * v.position.extend(1.0),
            normal,
            face_normal,
        }
    }

    /// Linearly interpolates between two clip-space vertices at parameter `t` (used by clipping;
    /// linear interpolation in clip space is exact for the clip intersection point).
    fn lerp(self, other: Self, t: f32) -> Self {
        Self {
            clip: self.clip.lerp(other.clip, t),
            normal: self.normal.lerp(other.normal, t),
            face_normal: self.face_normal.lerp(other.face_normal, t),
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
    /// The triangle's face normal (world space), constant across the triangle, for flat shading.
    face_normal: Vec3,
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
        face_normal: v.face_normal,
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
            face_normal: Vec3::Z,
        }
    }

    /// Builds a screen-space vertex with explicit shading normals (position/depth irrelevant to
    /// the shading tests).
    fn sv_shaded(vertex_normal: Vec3, face_normal: Vec3) -> ScreenVertex {
        ScreenVertex {
            pos: Vec2::ZERO,
            depth: 0.5,
            inv_w: 1.0,
            normal_over_w: vertex_normal,
            face_normal,
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
            face_normal: Vec3::Z,
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
            face_normal: Vec3::Z,
        };
        let front_b = ClipVertex {
            clip: Vec4::new(1.0, -1.0, 1.0, 1.0),
            normal: Vec3::Z,
            face_normal: Vec3::Z,
        };
        let behind = ClipVertex {
            clip: Vec4::new(0.0, 1.0, -1.0, 1.0),
            normal: Vec3::Z,
            face_normal: Vec3::Z,
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

    // --- Wireframe --------------------------------------------------------------------------------

    #[test]
    fn dedup_edges_drops_shared_diagonal() {
        // A quad split into two triangles sharing the (0,2) diagonal: 6 directed edges collapse to
        // 5 unique undirected edges.
        let edges = dedup_edges(&[0, 1, 2, 0, 2, 3]);
        assert_eq!(edges.len(), 5);
        // The shared diagonal appears exactly once, canonicalized as (min, max).
        assert_eq!(edges.iter().filter(|&&e| e == (0, 2)).count(), 1);
    }

    #[test]
    fn dedup_edges_of_cube_recovers_expected_edge_count() {
        // 8-corner cube = 12 box edges + one diagonal per quad face (6) = 18 unique edges.
        let cube = crate::primitives::cube(1.0);
        assert_eq!(dedup_edges(&cube.indices).len(), 18);
    }

    #[test]
    fn dedup_edges_ignores_trailing_partial_triple() {
        // Two full triangles plus two stray indices: the stray pair forms no triangle and is
        // dropped by `chunks_exact(3)`.
        let edges = dedup_edges(&[0, 1, 2, 0, 2, 3, 9, 9]);
        assert_eq!(edges.len(), 5);
    }

    #[test]
    fn wireframe_draws_edges_but_leaves_interior_empty() {
        // A projected cube from a deterministic front camera: edges are drawn, but the enclosed
        // region is not fully filled (that is what makes it a wireframe rather than a solid).
        let mut fb = Framebuffer::new(24, 24);
        let mut r = Rasterizer::new(ShadingMode::Wireframe);
        r.base_color = Color::WHITE;
        r.render(&cube_scene(), &front_camera(), &mut fb).unwrap();

        let lit = opaque_pixels(&fb);
        assert!(!lit.is_empty(), "wireframe must draw some edge pixels");

        // Within the bounding box of the drawn pixels there must be at least one empty pixel: a
        // solid fill would cover the whole box.
        let min_x = lit.iter().map(|p| p.0).min().unwrap();
        let max_x = lit.iter().map(|p| p.0).max().unwrap();
        let min_y = lit.iter().map(|p| p.1).min().unwrap();
        let max_y = lit.iter().map(|p| p.1).max().unwrap();
        let mut empty_inside = 0usize;
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                if fb.get(x, y).a == 0.0 {
                    empty_inside += 1;
                }
            }
        }
        assert!(
            empty_inside > 0,
            "a wireframe must leave interior pixels empty"
        );

        // A solid unlit render of the same scene covers strictly more pixels than the wireframe.
        let mut solid = Framebuffer::new(24, 24);
        let mut rs = Rasterizer::new(ShadingMode::Unlit);
        rs.render(&cube_scene(), &front_camera(), &mut solid)
            .unwrap();
        assert!(
            opaque_pixels(&solid).len() > lit.len(),
            "solid fill must cover more pixels than the wireframe"
        );
    }

    #[test]
    fn wireframe_snapshot_is_deterministic() {
        // The same scene and camera must produce an identical edge pixel set every time.
        let render = || {
            let mut fb = Framebuffer::new(20, 20);
            let mut r = Rasterizer::new(ShadingMode::Wireframe);
            r.render(&cube_scene(), &front_camera(), &mut fb).unwrap();
            opaque_pixels(&fb)
        };
        assert_eq!(render(), render());
    }

    #[test]
    fn wireframe_handles_empty_and_offscreen_targets() {
        let mut r = Rasterizer::new(ShadingMode::Wireframe);

        // Zero-sized target: no panic, no writes.
        let mut empty = Framebuffer::new(0, 0);
        r.render(&cube_scene(), &front_camera(), &mut empty)
            .unwrap();

        // Camera looking away: nothing drawn, no panic.
        let mut fb = Framebuffer::new(16, 16);
        let mut away = front_camera();
        away.target = Vec3::new(0.0, 0.0, 50.0);
        r.render(&cube_scene(), &away, &mut fb).unwrap();
        assert!(opaque_pixels(&fb).is_empty());
    }

    #[test]
    fn wireframe_out_of_range_index_errors_without_panicking() {
        let mesh = Mesh::new(vec![Vertex::from_position(Vec3::ZERO)], vec![0, 1, 2]);
        let scene = Scene::new("bad", vec![mesh]);
        let mut fb = Framebuffer::new(8, 8);
        let mut r = Rasterizer::new(ShadingMode::Wireframe);
        let err = r.render(&scene, &front_camera(), &mut fb).unwrap_err();
        assert!(matches!(err, Error::Geometry(_)));
    }

    #[test]
    fn clip_segment_near_trims_and_rejects() {
        // Fully behind the near plane -> rejected.
        let behind_a = Vec4::new(0.0, 0.0, -1.0, 1.0);
        let behind_b = Vec4::new(1.0, 0.0, -2.0, 1.0);
        assert!(clip_segment_near(behind_a, behind_b).is_none());

        // Straddling the plane -> the behind endpoint is moved onto z = 0.
        let front = Vec4::new(0.0, 0.0, 1.0, 1.0);
        let (a, b) = clip_segment_near(front, behind_a).unwrap();
        assert_eq!(a, front, "the in-front endpoint is preserved");
        assert!(
            b.z.abs() < 1e-6,
            "the clipped endpoint lands on the near plane"
        );
    }

    // --- Lighting / shading -----------------------------------------------------------------------

    const THIRD: (f32, f32, f32) = (1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0);

    #[test]
    fn lambert_matches_known_normal_light_pairs() {
        // Facing the light head-on with no ambient => full intensity.
        assert!((lambert(Vec3::Z, Vec3::Z, 0.0) - 1.0).abs() < 1e-6);
        // Perpendicular to the light => diffuse is zero, only ambient remains.
        assert!(lambert(Vec3::X, Vec3::Z, 0.0).abs() < 1e-6);
        // Turned away => the negative dot clamps to zero, leaving the ambient floor.
        assert!((lambert(-Vec3::Z, Vec3::Z, 0.2) - 0.2).abs() < 1e-6);
        // 45° between normal and light => cos(45°) diffuse.
        let n = Vec3::new(1.0, 0.0, 1.0).normalize();
        assert!((lambert(n, Vec3::Z, 0.0) - n.dot(Vec3::Z)).abs() < 1e-6);
        // Ambient plus full diffuse saturates at 1.0 (clamped, never above).
        assert!((lambert(Vec3::Z, Vec3::Z, 0.5) - 1.0).abs() < 1e-6);
        // Light direction need not be normalized: only its direction matters.
        assert!((lambert(Vec3::Z, Vec3::Z * 7.0, 0.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn flat_light_facing_face_brighter_than_away_with_ambient_floor() {
        let mut r = Rasterizer::new(ShadingMode::Flat);
        r.base_color = Color::WHITE;
        r.ambient = 0.1;
        r.light_direction = Vec3::Z;

        let toward = [sv_shaded(Vec3::Z, Vec3::Z); 3];
        let away = [sv_shaded(Vec3::NEG_Z, Vec3::NEG_Z); 3];
        let lit = r.shade(&toward, THIRD, 0.5);
        let dark = r.shade(&away, THIRD, 0.5);

        assert!(
            lit.luma() > dark.luma(),
            "face toward the light must be brighter"
        );
        assert!(
            dark.luma() > 0.0,
            "ambient floor must prevent a fully-black visible face"
        );
        assert!(
            (dark.luma() - 0.1).abs() < 1e-6,
            "away face lit only by ambient"
        );
        // Lit fragments are always opaque so they compose into the framebuffer.
        assert_eq!(lit.a, 1.0);
    }

    #[test]
    fn flat_and_smooth_differ_on_quad_with_differing_vertex_normals() {
        // A triangle carrying three different vertex normals but a single face normal. Flat uses
        // the constant face normal; smooth interpolates the per-vertex normals, so at an interior
        // point the two intensities differ.
        let face = Vec3::Z;
        let verts = [
            sv_shaded(Vec3::X, face),
            sv_shaded(Vec3::Y, face),
            sv_shaded(Vec3::Z, face),
        ];

        let mut flat = Rasterizer::new(ShadingMode::Flat);
        flat.base_color = Color::WHITE;
        flat.ambient = 0.0;
        flat.light_direction = Vec3::Z;
        let mut smooth = flat;
        smooth.shading = ShadingMode::Smooth;

        let c_flat = flat.shade(&verts, THIRD, 0.5);
        let c_smooth = smooth.shade(&verts, THIRD, 0.5);

        // Flat: dot(face=Z, Z) = 1 => full white. Smooth: normalize(1,1,1)·Z ≈ 0.577.
        assert!((c_flat.luma() - 1.0).abs() < 1e-4);
        assert!(
            (c_flat.luma() - c_smooth.luma()).abs() > 1e-2,
            "flat and smooth must produce different shading on differing vertex normals"
        );
    }

    #[test]
    fn normals_and_depth_modes_map_deterministically() {
        let verts = [sv_shaded(Vec3::Z, Vec3::Z); 3];

        // Normals: n * 0.5 + 0.5, so +Z encodes (0.5, 0.5, 1.0).
        let n = Rasterizer::new(ShadingMode::Normals).shade(&verts, THIRD, 0.5);
        assert!((n.r - 0.5).abs() < 1e-6 && (n.g - 0.5).abs() < 1e-6 && (n.b - 1.0).abs() < 1e-6);

        // Depth: grayscale = 1 - depth (nearer is brighter), equal across channels.
        let d = Rasterizer::new(ShadingMode::Depth).shade(&verts, THIRD, 0.25);
        assert!((d.r - 0.75).abs() < 1e-6);
        assert_eq!(d.r, d.g);
        assert_eq!(d.g, d.b);
    }

    #[test]
    fn flat_generates_face_normal_when_mesh_lacks_normals() {
        // A triangle in the z = 0 plane, wound CCW as seen from +Z, with no vertex normals. Flat
        // shading must synthesize a +Z face normal from geometry and catch the +Z light.
        let mesh = Mesh::new(
            vec![
                Vertex::from_position(Vec3::new(-1.0, -1.0, 0.0)),
                Vertex::from_position(Vec3::new(1.0, -1.0, 0.0)),
                Vertex::from_position(Vec3::new(0.0, 1.0, 0.0)),
            ],
            vec![0, 1, 2],
        );
        let scene = Scene::new("tri", vec![mesh]);

        let mut fb = Framebuffer::new(16, 16);
        let mut r = Rasterizer::new(ShadingMode::Flat);
        r.cull = Cull::None;
        r.base_color = Color::WHITE;
        r.ambient = 0.1;
        r.light_direction = Vec3::Z;
        r.render(&scene, &front_camera(), &mut fb).unwrap();

        let max_luma = (0..fb.height())
            .flat_map(|y| (0..fb.width()).map(move |x| (x, y)))
            .map(|(x, y)| fb.get(x, y))
            .filter(|c| c.a > 0.0)
            .map(|c| c.luma())
            .fold(0.0_f32, f32::max);
        assert!(
            max_luma > 0.1 + 1e-3,
            "a generated face normal facing the light must shade brighter than ambient, got {max_luma}"
        );
    }

    #[test]
    fn smooth_interpolates_intensity_across_a_face() {
        // A single triangle whose vertex normals fan from +X to +Y to +Z. Under a +Z light the
        // per-fragment intensity must vary across the face (no single flat value).
        let normals = [Vec3::X, Vec3::Y, Vec3::Z];
        let verts = [
            sv_shaded(normals[0], Vec3::Z),
            sv_shaded(normals[1], Vec3::Z),
            sv_shaded(normals[2], Vec3::Z),
        ];
        let mut r = Rasterizer::new(ShadingMode::Smooth);
        r.base_color = Color::WHITE;
        r.ambient = 0.0;
        r.light_direction = Vec3::Z;

        // At the vertex weighted fully toward +Z, intensity is 1; toward +X it is ~0.
        let at_z = r.shade(&verts, (0.0, 0.0, 1.0), 0.5);
        let at_x = r.shade(&verts, (1.0, 0.0, 0.0), 0.5);
        assert!((at_z.luma() - 1.0).abs() < 1e-4);
        assert!(at_x.luma() < 1e-4);
        assert!(at_z.luma() > at_x.luma());
    }
}
