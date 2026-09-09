---
name: rgfx-architecture
description: The frozen type and trait contracts for rgfx — Framebuffer layout, Color, the TerminalEncoder / SceneRenderer / FrameSource traits, and Braille bit math. Load this when implementing any rgfx crate so your types match what every other crate expects.
---

# rgfx architecture contract

These are the shared contracts defined in `rgfx-core`. Every other crate builds against them.
Do not redefine these types locally — import them from `rgfx_core`. If a contract is missing
something you need, note it in your PR rather than forking the definition.

## Framebuffer

The convergence point for all media. Color + depth, contiguous, reusable.

```rust
/// Linear-ish RGBA color, 0.0..=1.0 per channel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color { pub r: f32, pub g: f32, pub b: f32, pub a: f32 }

/// A CPU render target. Color and depth are separate contiguous buffers.
/// Row-major: index = y * width + x.
pub struct Framebuffer {
    width: usize,
    height: usize,
    color: Vec<Color>,   // len == width*height
    depth: Vec<f32>,     // len == width*height, 1.0 = far / cleared
}

impl Framebuffer {
    pub fn new(width: usize, height: usize) -> Self;
    pub fn width(&self) -> usize;
    pub fn height(&self) -> usize;
    /// Reuse allocation when possible; only grows the Vecs when needed.
    pub fn resize(&mut self, width: usize, height: usize);
    pub fn clear(&mut self, color: Color);           // resets depth to 1.0 too
    pub fn color(&self) -> &[Color];
    pub fn depth_mut(&mut self) -> &mut [f32];
    pub fn get(&self, x: usize, y: usize) -> Color;
    pub fn set(&mut self, x: usize, y: usize, c: Color);
    /// Perceptual luminance of a cell, 0.0..=1.0. Used by grayscale encoders.
    pub fn luma(&self, x: usize, y: usize) -> f32;
}
```

Rules: never allocate a new `Framebuffer` per frame — call `resize`/`clear` on a reused one.
Terminal-cell dimensions are NOT framebuffer dimensions (see viewport below).

## Errors

```rust
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("io: {0}")] Io(#[from] std::io::Error),
    #[error("unsupported format: {0}")] Unsupported(String),
    #[error("decode: {0}")] Decode(String),
    #[error("invalid geometry: {0}")] Geometry(String),
    // extend as needed, keep variants coarse
}
pub type Result<T> = std::result::Result<T, Error>;
```

## Core traits (the composition seams)

```rust
/// Anything that yields frames: image (1 frame), gif/video (N frames), 3d scene (on demand).
pub trait FrameSource {
    /// Render the next frame into `target`, resized to the requested viewport by the caller.
    /// Returns Ok(false) when the source is exhausted (e.g. end of video).
    fn next_frame(&mut self, target: &mut Framebuffer) -> Result<bool>;
    /// Optional frame delay for animated sources.
    fn frame_delay(&self) -> Option<std::time::Duration> { None }
}

/// Turns a framebuffer into terminal text. Implemented by braille/ascii/blocks encoders.
pub trait TerminalEncoder {
    fn encode(&self, frame: &Framebuffer, viewport: Viewport) -> TerminalFrame;
}

/// Rasterizes a 3D scene into a framebuffer. Implemented in rgfx-3d.
pub trait SceneRenderer {
    fn render(&mut self, scene: &Scene, camera: &Camera, target: &mut Framebuffer) -> Result<()>;
}
```

`Viewport` carries terminal columns/rows + subpixel factors; `TerminalFrame` is the encoded
cell grid (chars + optional ANSI color) plus its dimensions, ready for diffed output.

## Viewport / responsiveness

```rust
pub struct Viewport { pub cols: u16, pub rows: u16 }
// render_width  = viewport pixel width  (cols * subpixel_x, e.g. *2 for braille)
// render_height = viewport pixel height (rows * subpixel_y, e.g. *4 for braille)
```
On resize: read new cols/rows → recompute render size → `Framebuffer::resize` → (3D) update
camera aspect → render. Never rely on terminal line wrapping.

## Braille bit math (rgfx-terminal)

One cell = 2 columns × 4 rows of framebuffer pixels. Dot layout and masks:

```
positions:   masks:
1 4          dot1=0x01 dot4=0x08
2 5          dot2=0x02 dot5=0x10
3 6          dot3=0x04 dot6=0x20
7 8          dot7=0x40 dot8=0x80
base code point = U+2800
```
```rust
let mut mask = 0u8;
if p1 { mask |= 0x01 } // (col0,row0)
if p2 { mask |= 0x02 } // (col0,row1)
if p3 { mask |= 0x04 } // (col0,row2)
if p4 { mask |= 0x08 } // (col1,row0)
if p5 { mask |= 0x10 } // (col1,row1)
if p6 { mask |= 0x20 } // (col1,row2)
if p7 { mask |= 0x40 } // (col0,row3)
if p8 { mask |= 0x80 } // (col1,row3)
let ch = char::from_u32(0x2800 + mask as u32).unwrap(); // invariant: mask <= 0xFF
```
Encoder options: threshold, invert, dither (Floyd–Steinberg / Atkinson / Bayer), gamma,
contrast, edge enhance, color mode. Compare visual output against
https://lachlanarthur.github.io/Braille-ASCII-Art/

## ASCII / blocks

- ASCII density ramp (dark→light): `" .:-=+*#%@"`; pick glyph by cell luma.
- Blocks: use half-block `▀`/`▄` with fg/bg color to get 1×2 vertical subpixels per cell,
  plus `█ ▌ ▐ ░ ▒ ▓` for shading.

## 3D pipeline (rgfx-3d) summary

mesh → model/view/projection transform → clip → perspective divide → viewport transform →
backface cull → barycentric raster → z-buffer depth test → shade (unlit/flat/smooth/normals/
depth/wireframe) → write into Framebuffer. Auto-frame the camera from the mesh bounding sphere.
Use `glam` for Vec/Mat/Quat. No GPU/OpenGL/Vulkan — CPU rasterizer only.
