# rgfx
[![CI](https://github.com/05vukasin/rgfx/actions/workflows/ci.yml/badge.svg)](https://github.com/05vukasin/rgfx/actions/workflows/ci.yml)
[![Release](https://github.com/05vukasin/rgfx/actions/workflows/release.yml/badge.svg)](https://github.com/05vukasin/rgfx/actions/workflows/release.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
![Rust](https://img.shields.io/badge/rust-1.85%2B-orange.svg)


> **A universal terminal graphics engine for images, video, animation, and interactive 3D model previews — written in Rust.**

`rgfx` is an open-source terminal rendering project designed to make rich visual media usable directly inside a terminal.

The target user experience is intentionally simple:

```bash
rgfx model.glb
rgfx model.obj
rgfx model.stl
rgfx image.png
rgfx animation.gif
rgfx video.mp4
```

`rgfx` should automatically detect the input type, inspect the available terminal dimensions and capabilities, select an appropriate rendering pipeline, and open a responsive terminal viewport.

For a 3D model, the user should be able to rotate, zoom, inspect, switch shading modes, toggle wireframe rendering, and preview the model without opening Blender or another GUI application.

For images, GIFs, and video, `rgfx` should convert raster frames into terminal graphics using Unicode Braille, ASCII density maps, Unicode block elements, and optional ANSI color.

The long-term objective is **not** to recreate Blender inside a terminal. The goal is to build a fast, composable, Unix-friendly terminal media and 3D preview engine that is useful:

- locally,
- over SSH,
- inside TUI applications,
- in file-manager preview panes,
- in developer tooling,
- in AI-agent interfaces,
- in dashboards,
- for terminal avatars and animations,
- and as a Rust graphics library that other projects can embed.

---

## Status

This README is the architectural specification and development roadmap for the project.

Suggested initial status:

```text
Project name: rgfx
Language: Rust
Primary platform: Linux
Primary target distribution: Arch Linux / CachyOS
Rendering target: Terminal / TTY
License: MIT or Apache-2.0
First public milestone: Interactive OBJ/STL 3D viewer using Unicode Braille
```

---

# Getting Started

## Requirements

- Rust 1.85+ (edition 2024). Install via [rustup](https://rustup.rs).
- Linux is the primary supported platform.
- `ffmpeg` is **optional** — it is only needed for video decoding (feature-gated).
  The base workspace builds and runs without it.

On Arch Linux / CachyOS:

```bash
sudo pacman -S --needed base-devel git rust cargo
# optional, for video support:
sudo pacman -S --needed ffmpeg
```

## Build

```bash
git clone https://github.com/05vukasin/rgfx.git
cd rgfx
cargo build --workspace              # debug build, no ffmpeg required
cargo build --workspace --release    # optimized build
```

## Run

Once the CLI crate is in place, run it through Cargo during development:

```bash
cargo run -p rgfx-cli -- model.obj
cargo run -p rgfx-cli -- image.png --renderer braille
```

Or run the compiled release binary directly:

```bash
./target/release/rgfx model.obj
```

## Test

The workspace mirrors the CI gate. Before opening a PR, run:

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace
cargo build --workspace
```

## Install

### One-line installer (recommended)

The installer detects your OS/architecture, resolves the latest release,
downloads the matching archive, **verifies its SHA-256 checksum**, and installs
`rgfx` to `~/.local/bin`:

```bash
curl -fsSL https://raw.githubusercontent.com/05vukasin/rgfx/main/install.sh | bash
```

Overridable via environment variables: `RGFX_REPO`, `RGFX_INSTALL_DIR`,
`RGFX_VERSION`. Make sure `~/.local/bin` is on your `PATH` (the script prints
guidance if it is not).

### From a release archive (manual)

Prebuilt Linux binaries are published on the
[Releases page](https://github.com/05vukasin/rgfx/releases) for
`x86_64` and `aarch64`. Each release ships:

```text
rgfx-linux-x86_64.tar.gz
rgfx-linux-aarch64.tar.gz
SHA256SUMS
```

Download the archive for your architecture, verify it against `SHA256SUMS`,
extract it, and place the `rgfx` binary on your `PATH`:

```bash
ARCH="$(uname -m)"   # x86_64 or aarch64
curl -fLO "https://github.com/05vukasin/rgfx/releases/latest/download/rgfx-linux-${ARCH}.tar.gz"
curl -fLO "https://github.com/05vukasin/rgfx/releases/latest/download/SHA256SUMS"
sha256sum --check --ignore-missing SHA256SUMS
tar -xzf "rgfx-linux-${ARCH}.tar.gz"
install -m755 rgfx "$HOME/.local/bin/rgfx"
```

Make sure `~/.local/bin` is on your `PATH`.

### Arch Linux / CachyOS (AUR)

PKGBUILD templates live in [`packaging/aur/`](packaging/aur/):

```bash
yay -S rgfx       # stable source build
yay -S rgfx-bin   # prebuilt release binary
yay -S rgfx-git   # latest main branch
```

### From source

```bash
cargo install --path crates/rgfx-cli
```

---

# License

`rgfx` is dual-licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or
  <http://opensource.org/licenses/MIT>)

at your option. Copyright the rgfx authors.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual-licensed as above, without any additional terms or conditions.

## Contributing

Contributions are welcome. Please read [CONTRIBUTING.md](CONTRIBUTING.md) for the
engineering conventions and PR flow, and note that this project follows the
[Contributor Covenant Code of Conduct](CODE_OF_CONDUCT.md). Security issues should
be reported per [SECURITY.md](SECURITY.md).

---

# Core Idea

The most important architectural rule is:

> **Do not render media directly as terminal characters. Render into an internal framebuffer first, then encode that framebuffer into terminal cells.**

This keeps images, video, animation, and 3D models on the same rendering pipeline.

```text
                      INPUT
          ┌────────────┼────────────┐
          │            │            │
        Image        Video        3D Scene
          │            │            │
          └───────┬────┴────┬───────┘
                  │         │
                  ▼         ▼
              Loader / Decoder
                     │
                     ▼
               Internal Frame
                     │
                     ▼
                Framebuffer
                     │
          ┌──────────┼──────────┐
          ▼          ▼          ▼
       Braille      ASCII      Blocks
          │          │          │
          └──────────┴──────────┘
                     │
                     ▼
                ANSI Output
                     │
                     ▼
                  Terminal
```

The source is irrelevant to the terminal encoder.

An image may produce a framebuffer.

A decoded video frame may produce a framebuffer.

A rasterized 3D scene may produce a framebuffer.

A procedural animation may produce a framebuffer.

The terminal layer only receives pixels and turns them into text.

This separation is what makes the project scalable.

---

# Primary Visual Reference

## LachlanArthur / Braille-ASCII-Art

This is the most important visual and technical reference for the desired Braille output.

Live tool:

https://lachlanarthur.github.io/Braille-ASCII-Art/

GitHub repository:

https://github.com/LachlanArthur/Braille-ASCII-Art

Important ideas demonstrated by this project:

- Unicode Braille rendering
- User-controlled output width in terminal characters
- Dithering
- Threshold control
- Inversion
- Preview font size
- Output that remains visually meaningful at different character widths
- Direct conversion from raster image data into a 2×4-dot Braille representation

The most important idea for `rgfx` is **output width in characters**.

A source image can be rendered at:

```text
100 characters wide
60 characters wide
30 characters wide
```

while still preserving the same underlying Braille rendering method.

`rgfx` should make this automatic.

Instead of asking the user for a width, the application should continuously calculate an appropriate render size from the current terminal dimensions.

Example:

```text
Terminal width: 132 columns
UI padding:       8 columns
Render width:   124 columns
```

After resizing:

```text
Terminal width: 44 columns
UI padding:      4 columns
Render width:   40 columns
```

The framebuffer is recreated at the new size and the media is rendered again.

---

# Other Useful Reference Projects

## SoufianoDev / img2ascii

Repository:

https://github.com/SoufianoDev/img2ascii

Live version:

https://soufianodev.github.io/img2ascii/

Useful ideas:

- Unicode Braille rendering
- real-time rendering
- configurable width
- gamma thresholding
- edge sharpening
- Floyd–Steinberg dithering
- invert mode
- local image processing
- text export

---

## TheFel0x / img2braille

Repository:

https://github.com/TheFel0x/img2braille

Useful ideas:

- 2×4 Braille mapping
- U+2800 mask construction
- ANSI color
- dithering
- width control
- auto-contrast

Conceptually:

```text
Image
  ↓
Divide into 2 × 4 pixel blocks
  ↓
Determine active Braille dots
  ↓
Build an 8-bit mask
  ↓
Add the mask to Unicode U+2800
  ↓
Output the resulting Braille character
```

---

## ading2210 / ascii-art

Repository:

https://github.com/ading2210/ascii-art

Useful ideas:

- simple image-to-Braille conversion
- custom image size
- scale control
- dithering
- threshold control
- inverted rendering
- clean CLI workflow

---

# Why Unicode Braille

Unicode Braille patterns occupy:

```text
U+2800 – U+28FF
```

A single Braille character represents:

```text
2 columns × 4 rows
```

or eight independent binary dots.

Standard dot positions:

```text
1 4
2 5
3 6
7 8
```

Each dot can be enabled or disabled.

Therefore:

```text
2^8 = 256
```

different patterns are possible.

A terminal viewport of:

```text
100 columns × 40 rows
```

can represent approximately:

```text
200 × 160
```

binary subpixels using Braille.

This makes Braille especially useful for responsive terminal graphics.

---

# Project Goals

`rgfx` should ultimately support four major use cases.

## 1. Interactive 3D Preview

```bash
rgfx model.obj
```

The user should be able to:

- rotate
- orbit
- zoom
- pan
- reset the camera
- toggle wireframe
- switch shading mode
- switch terminal renderer
- inspect model statistics

## 2. Image Preview

```bash
rgfx image.png
```

The image should be:

- decoded
- resized
- aspect-corrected
- converted to grayscale or color
- dithered if necessary
- rendered with Braille / ASCII / blocks

## 3. Video and Animated Image Playback

```bash
rgfx video.mp4
rgfx animation.gif
```

Features:

- playback
- pause
- seek
- frame timing
- adaptive FPS
- responsive resize

## 4. Embeddable Graphics Engine

Possible uses:

- terminal file managers
- AI agents
- CLI avatars
- dashboards
- terminal games
- model preview panes
- remote SSH tools
- TUI applications

---

# Non-Goals

Initial versions should not attempt to be a full editor.

Do not initially implement:

- mesh editing
- sculpting
- CAD operations
- ray tracing
- full PBR
- Blender-level materials
- complex compositing
- video editing

`rgfx` is primarily a **renderer and viewer**.

---

# Supported Media

Suggested support order:

## Phase 1

- PNG
- JPEG
- OBJ

## Phase 2

- WebP
- BMP
- STL
- GIF

## Phase 3

- glTF
- GLB
- PLY

## Phase 4

- MP4
- WebM
- MKV
- MOV through FFmpeg

## Later

- point clouds
- animated glTF
- voxel data
- frame streams
- programmatically generated frames

---

# Rendering Modes

## Braille

```bash
rgfx image.png --renderer braille
```

Best for:

- high spatial resolution
- silhouettes
- 3D geometry
- photographs after dithering
- low-bandwidth terminal graphics

## ASCII

```bash
rgfx image.png --renderer ascii
```

Example density map:

```text
 .:-=+*#%@
```

## Unicode Blocks

```bash
rgfx image.png --renderer blocks
```

Useful glyphs:

```text
█
▀
▄
▌
▐
░
▒
▓
```

## Auto Mode

```bash
rgfx image.png --renderer auto
```

The engine chooses the best renderer automatically.

---

# Responsive Terminal Rendering

Responsiveness is a core project requirement.

The renderer should continuously know:

```text
terminal columns
terminal rows
```

Preferred architecture:

```rust
render_width = terminal_width.saturating_sub(ui_width);
render_height = terminal_height.saturating_sub(ui_height);
```

On resize:

```text
Terminal resize
      ↓
Resize event
      ↓
Read new dimensions
      ↓
Recalculate viewport
      ↓
Resize or reuse framebuffer
      ↓
Update camera aspect ratio
      ↓
Render next frame
```

The application should never rely on line wrapping.

---

# Recommended Rust Stack

## CLI

### clap

https://crates.io/crates/clap

Use for:

- commands
- subcommands
- flags
- help output
- shell completions

---

## Terminal Interaction

### crossterm

https://crates.io/crates/crossterm

Use for:

- alternate screen
- cursor control
- keyboard events
- mouse events
- resize events
- raw mode
- terminal dimensions

---

## TUI Integration

### ratatui

https://crates.io/crates/ratatui

Potential future integration crate:

```text
rgfx-ratatui
```

---

## 3D Math

### glam

https://crates.io/crates/glam

Use for:

- Vec2
- Vec3
- Vec4
- Mat4
- quaternions
- transforms
- camera calculations

Alternative:

https://crates.io/crates/nalgebra

---

## OBJ Loading

### tobj

https://crates.io/crates/tobj

---

## glTF / GLB

### gltf

https://crates.io/crates/gltf

---

## STL

### stl_io

https://crates.io/crates/stl_io

---

## Image Decoding

### image

https://crates.io/crates/image

Use for:

- PNG
- JPEG
- GIF
- WebP
- BMP

---

## Video

Preferred initial strategy: FFmpeg.

Possible Rust crate:

https://crates.io/crates/ffmpeg-next

Recommended first implementation:

- use external `ffmpeg` / `ffprobe`
- decode frames into RGB
- pass frames through the normal image/framebuffer pipeline

---

## Parallelism

### rayon

https://crates.io/crates/rayon

Only use after profiling.

---

## Error Handling

### anyhow

https://crates.io/crates/anyhow

### thiserror

https://crates.io/crates/thiserror

---

## Logging

### tracing

https://crates.io/crates/tracing

---

## Configuration

### serde

https://crates.io/crates/serde

### toml

https://crates.io/crates/toml

---

# System Dependencies

For Arch Linux / CachyOS development:

```bash
sudo pacman -S --needed base-devel git rust cargo ffmpeg
```

For a minimal image + OBJ viewer, FFmpeg can remain optional.

---

# High-Level Architecture

```text
                       rgfx CLI
                          │
                          ▼
                    rgfx-core API
                          │
           ┌──────────────┼──────────────┐
           │              │              │
           ▼              ▼              ▼
       rgfx-image     rgfx-video      rgfx-3d
           │              │              │
           └──────────────┼──────────────┘
                          │
                          ▼
                     Framebuffer
                          │
                          ▼
                   rgfx-terminal
                          │
             ┌────────────┼────────────┐
             ▼            ▼            ▼
          Braille       ASCII        Blocks
             │            │            │
             └────────────┴────────────┘
                          │
                          ▼
                       Terminal
```

Important rules:

- loaders do not know about Braille
- Braille does not know about OBJ
- the 3D renderer does not call `println!()`
- all media sources converge into a framebuffer

---

# Suggested Cargo Workspace

```text
rgfx/
├── Cargo.toml
├── Cargo.lock
├── README.md
├── LICENSE
├── CHANGELOG.md
├── CONTRIBUTING.md
├── CODE_OF_CONDUCT.md
├── SECURITY.md
├── install.sh
│
├── .github/
│   ├── workflows/
│   │   ├── ci.yml
│   │   └── release.yml
│   └── ISSUE_TEMPLATE/
│
├── crates/
│   ├── rgfx-core/
│   ├── rgfx-terminal/
│   ├── rgfx-image/
│   ├── rgfx-3d/
│   ├── rgfx-video/
│   └── rgfx-cli/
│
├── examples/
├── tests/
├── benches/
└── assets/
```

For the MVP, start simpler if needed.

---

# Core Framebuffer Design

Minimal grayscale framebuffer:

```rust
pub struct Framebuffer {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<f32>,
}
```

Values:

```text
0.0 → black
1.0 → white
```

Future color framebuffer:

```rust
pub struct Framebuffer {
    width: usize,
    height: usize,
    color: Vec<Rgb>,
    depth: Vec<f32>,
}
```

Important implementation rules:

- reuse buffers
- avoid per-frame allocations
- keep memory contiguous
- separate color and depth storage
- keep terminal cell dimensions separate from framebuffer dimensions

---

# Braille Renderer

Each terminal cell consumes:

```text
2 × 4 framebuffer pixels
```

Dot positions:

```text
1 4
2 5
3 6
7 8
```

Unicode bit masks:

```text
dot 1 → 0x01
dot 2 → 0x02
dot 3 → 0x04
dot 4 → 0x08
dot 5 → 0x10
dot 6 → 0x20
dot 7 → 0x40
dot 8 → 0x80
```

Base code point:

```text
U+2800
```

Pseudo-code:

```rust
let mut mask = 0u8;

if pixel_1 { mask |= 0x01; }
if pixel_2 { mask |= 0x02; }
if pixel_3 { mask |= 0x04; }
if pixel_4 { mask |= 0x08; }
if pixel_5 { mask |= 0x10; }
if pixel_6 { mask |= 0x20; }
if pixel_7 { mask |= 0x40; }
if pixel_8 { mask |= 0x80; }

let ch = char::from_u32(0x2800 + mask as u32).unwrap();
```

Options:

```text
threshold
invert
dither
gamma
contrast
edge enhancement
color mode
```

---

# Image Pipeline

```text
Input file
   ↓
Decode
   ↓
RGBA / RGB image
   ↓
Determine target terminal size
   ↓
Aspect-ratio correction
   ↓
Resize
   ↓
Optional sharpen
   ↓
Luminance / color conversion
   ↓
Gamma / contrast
   ↓
Dithering / threshold
   ↓
Framebuffer
   ↓
Terminal encoder
```

Dithering options:

- none
- Floyd–Steinberg
- Atkinson
- Bayer ordered dithering
- threshold-only

---

# Video Pipeline

```text
video.mp4
    ↓
FFmpeg decoder
    ↓
RGB frame
    ↓
Resize
    ↓
Image preprocessing
    ↓
Framebuffer
    ↓
Braille / ASCII / Blocks
    ↓
Terminal diff renderer
```

Playback features:

- frame timestamps
- pause
- seek
- restart
- adaptive frame skipping
- responsive resize

Audio can be optional in early versions.

---

# 3D Rendering Pipeline

Use a CPU software rasterizer first.

No OpenGL, Vulkan, SDL, X11, or Wayland should be required for the MVP.

```text
3D file
   ↓
Mesh loader
   ↓
Vertices / indices / normals
   ↓
Model transform
   ↓
View transform
   ↓
Projection
   ↓
Clipping
   ↓
Screen-space triangles
   ↓
Backface culling
   ↓
Triangle rasterization
   ↓
Depth test
   ↓
Lighting / shading
   ↓
Framebuffer
   ↓
Braille / ASCII / block encoder
   ↓
Terminal
```

---

# Interactive 3D Viewer

Target invocation:

```bash
rgfx model.obj
```

Suggested controls:

```text
Left / Right        Orbit around Y (yaw)
Up / Down           Orbit around X (pitch)
z / x               Roll around the view axis (third rotation axis)
+ / -               Zoom
Mouse drag          Orbit (yaw + pitch)
Mouse wheel         Zoom
R                   Reset camera
W                   Toggle wireframe
S                   Cycle shading
C                   Toggle color
L                   Toggle lighting
F                   Toggle UI mode
Q / Esc             Quit
```

The viewer opens at a 3/4 angle so a model reads as 3D immediately. `.blend` files are
supported by exporting to glTF through a headless Blender (requires the `blender` binary on
`PATH`, or set `RGFX_BLENDER`).

Future:

```text
Shift + drag        Pan
```

---

# Camera

Implement:

```text
PerspectiveCamera
OrthographicCamera
```

Properties:

```text
position
target
up
field of view
near plane
far plane
aspect ratio
```

---

# Automatic Model Framing

On load:

1. calculate bounding box
2. calculate center
3. estimate bounding sphere radius
4. position camera automatically
5. fit model into viewport
6. choose reasonable near/far planes

---

# Triangle Rasterization

Required components:

- vertex transformation
- homogeneous projection
- perspective divide
- viewport transform
- clipping
- barycentric coordinates
- depth interpolation
- z-buffer
- optional normal interpolation

---

# Z-Buffer

For every framebuffer pixel:

```rust
depth[y * width + x]
```

Depth test:

```rust
if fragment_depth < depth[index] {
    depth[index] = fragment_depth;
    color[index] = fragment_color;
}
```

---

# Lighting and Shading

Initial modes:

- unlit
- flat shading
- smooth shading
- normals
- depth
- wireframe

Basic directional light:

```text
intensity = max(0, dot(normal, light_direction))
```

Then add ambient contribution.

---

# 3D File Formats

## OBJ

First target.

Crate:

https://crates.io/crates/tobj

## STL

Second target.

Crate:

https://crates.io/crates/stl_io

## glTF / GLB

Modern target.

Crate:

https://crates.io/crates/gltf

Support progressively:

1. static meshes
2. node transforms
3. scenes
4. materials
5. textures
6. animation

## PLY

Later for:

- scans
- point clouds
- research data

---

# Animation and Avatar System

Future reusable types:

```rust
Frame
Animation
Sprite
AnimationPlayer
```

Possible states:

```text
Idle
Thinking
Speaking
Loading
Success
Error
```

Small terminal animation example:

```text
·
•
✦
✧
✦
•
·
```

Larger animations can use Braille frame sequences.

---

# Streaming and Unix Pipes

Potential interfaces:

```bash
cat image.png | rgfx -
```

Future:

```bash
some_frame_generator | rgfx --stream
```

Sources may include:

- stdin
- named pipes
- Unix sockets
- child processes
- Rust frame producers

---

# Performance Strategy

## Buffer Reuse

Avoid allocating a new framebuffer every frame.

## Double Buffering

Maintain:

```text
front frame
back frame
```

## Terminal Diffing

Compare:

```text
previous terminal cells
current terminal cells
```

and write only changes when practical.

## Batched Writes

Avoid thousands of tiny `print!()` calls.

## Event-Driven 3D Rendering

For a static model:

```text
No input
→ no redraw required
```

When the user rotates or resizes:

```text
event
→ render new frame
```

## Adaptive Quality

If rendering is too slow:

```text
reduce resolution
reduce FPS
```

## Multithreading

Potentially use:

https://crates.io/crates/rayon

only after profiling.

---

# Terminal Safety

On entry:

```text
enable raw mode
enter alternate screen
hide cursor
enable mouse capture if needed
```

On exit:

```text
disable mouse capture
show cursor
leave alternate screen
disable raw mode
```

Cleanup must work on:

- normal exit
- Q
- Esc
- Ctrl+C
- expected errors
- panic where practical

---

# CLI Design

Basic:

```bash
rgfx <FILE>
```

Examples:

```bash
rgfx model.obj
rgfx scan.stl
rgfx scene.glb
rgfx photo.jpg
rgfx animation.gif
rgfx video.mp4
```

Renderer:

```bash
rgfx image.png --renderer braille
rgfx image.png --renderer ascii
rgfx image.png --renderer blocks
```

Explicit width:

```bash
rgfx image.png --width 100
rgfx image.png --width 30
```

3D:

```bash
rgfx dragon.glb --wireframe
rgfx dragon.glb --shading smooth
rgfx dragon.glb --renderer braille
```

Video:

```bash
rgfx movie.mp4 --fps 30
```

Information:

```bash
rgfx info model.glb
```

Example:

```text
File: model.glb
Type: glTF Binary
Meshes: 4
Vertices: 123,820
Triangles: 241,110
Materials: 5
Animations: 2
```

---

# Configuration

Linux location:

```text
~/.config/rgfx/config.toml
```

Example:

```toml
renderer = "braille"
fps = 30
dither = "floyd"
color = false
show_stats = true
mouse = true

[three_d]
shading = "smooth"
wireframe = false
auto_rotate = false

[video]
adaptive_fps = true
```

CLI flags override config values.

---

# Development Phases

## Phase 0 — Repository Foundation

Create:

- repository
- Cargo project
- README
- license
- CI
- coding standards
- issue templates
- milestones

## Phase 1 — Terminal Foundation

Implement:

- terminal dimensions
- raw mode
- alternate screen
- cursor management
- keyboard input
- resize events
- cleanup

## Phase 2 — Framebuffer

Implement:

- grayscale framebuffer
- resize
- clear
- pixel access
- reusable allocation

## Phase 3 — Braille Encoder

Implement:

- 2×4 grouping
- dot masks
- U+2800 encoding
- threshold
- invert

## Phase 4 — Static Image Renderer

Implement:

- PNG
- JPEG
- responsive width
- aspect correction
- grayscale

## Phase 5 — Image Quality

Add:

- Floyd–Steinberg
- Bayer
- Atkinson
- gamma
- contrast
- sharpening

Compare output quality against:

https://lachlanarthur.github.io/Braille-ASCII-Art/

## Phase 6 — Terminal Frame Engine

Add:

- front/back buffers
- batched output
- differential rendering
- frame timing

## Phase 7 — 3D Math and Camera

Add:

- transforms
- perspective
- camera orbit
- zoom
- aspect ratio

## Phase 8 — Wireframe Cube

First interactive 3D milestone.

## Phase 9 — Triangle Rasterizer

Add:

- barycentric rasterization
- clipping
- backface culling
- z-buffer

## Phase 10 — Lighting

Add:

- normals
- ambient light
- directional diffuse light
- flat shading
- smooth shading

## Phase 11 — OBJ Loader

Use `tobj`.

Deliverable:

```bash
rgfx monkey.obj
```

## Phase 12 — Responsive 3D

Test behavior at:

```text
100 columns
60 columns
30 columns
```

## Phase 13 — 3D Viewer UX

Add:

- status bar
- file name
- triangle count
- FPS
- renderer
- keyboard help

## Phase 14 — STL

Deliverable:

```bash
rgfx print.stl
```

## Phase 15 — glTF / GLB

Implement progressively.

## Phase 16 — ASCII and Block Renderers

Add shared framebuffer encoders.

## Phase 17 — ANSI Color

Add:

- 16-color
- 256-color
- TrueColor

## Phase 18 — GIF Playback

Add:

- frame delay
- looping
- pause
- resize

## Phase 19 — Video Prototype

Use FFmpeg.

Deliverable:

```bash
rgfx video.mp4
```

## Phase 20 — Video Controls

Add:

- pause
- seek
- restart
- adaptive FPS

## Phase 21 — Public Rust API

Stabilize reusable types.

## Phase 22 — Ratatui Integration

Create optional:

```text
rgfx-ratatui
```

## Phase 23 — Streaming API

Support programmatic frame sources.

## Phase 24 — Avatar / Procedural Animation

Add:

- state animations
- frame sequences
- small CLI avatars

## Phase 25 — Optimization

Profile, then consider:

- Rayon
- tiled rasterization
- SIMD
- mesh simplification
- adaptive resolution

## Phase 26 — Packaging

Provide:

- GitHub Release binaries
- install script
- crates.io
- AUR
- source build instructions

## Phase 27 — Ecosystem Integrations

Targets:

- Yazi
- Ranger
- SSH workflows
- TUI applications
- file preview panes

---

# Testing Strategy

Unit tests:

- Braille mapping
- framebuffer indexing
- resize logic
- clipping
- depth testing
- luminance conversion
- aspect calculations

Snapshot tests:

- known framebuffer → known Braille output
- triangle
- cube
- known OBJ

Parser tests:

- malformed OBJ
- malformed STL
- malformed glTF

---

# Benchmarking

Benchmark models:

```text
1,000 triangles
10,000 triangles
100,000 triangles
1,000,000 triangles
```

Measure:

```text
transform time
rasterization time
Braille encode time
terminal diff time
terminal write time
total frame time
FPS
memory usage
bytes written per frame
```

---

# GitHub Actions and Releases

CI:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace
cargo build --workspace
```

Release targets:

```text
x86_64-unknown-linux-gnu
aarch64-unknown-linux-gnu
```

Later:

```text
x86_64-apple-darwin
aarch64-apple-darwin
x86_64-pc-windows-msvc
```

Release files:

```text
rgfx-linux-x86_64.tar.gz
rgfx-linux-aarch64.tar.gz
SHA256SUMS
```

---

# Installation

Desired final command:

```bash
curl -fsSL https://raw.githubusercontent.com/<OWNER>/rgfx/main/install.sh | sh
```

The installer should:

1. detect OS
2. detect CPU architecture
3. determine latest release
4. download the correct binary archive
5. verify checksum
6. install to `~/.local/bin/rgfx`
7. ensure executable permissions
8. explain PATH setup if necessary

---

# Example install.sh

```bash
#!/usr/bin/env bash

set -euo pipefail

REPO="${RGFX_REPO:-<OWNER>/rgfx}"
INSTALL_DIR="${RGFX_INSTALL_DIR:-$HOME/.local/bin}"

log() {
    printf '[rgfx] %s\n' "$1"
}

fail() {
    printf '[rgfx] error: %s\n' "$1" >&2
    exit 1
}

command -v curl >/dev/null 2>&1 || fail "curl is required"
command -v tar >/dev/null 2>&1 || fail "tar is required"

OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
    Linux) OS_NAME="linux" ;;
    *) fail "Unsupported operating system: $OS" ;;
esac

case "$ARCH" in
    x86_64|amd64) ARCH_NAME="x86_64" ;;
    aarch64|arm64) ARCH_NAME="aarch64" ;;
    *) fail "Unsupported architecture: $ARCH" ;;
esac

log "Detecting latest release..."

LATEST_URL="$(
    curl -fsSL         -o /dev/null         -w '%{url_effective}'         "https://github.com/${REPO}/releases/latest"
)"

VERSION="${LATEST_URL##*/}"

[ -n "$VERSION" ] || fail "Could not determine the latest release"

ARCHIVE="rgfx-${OS_NAME}-${ARCH_NAME}.tar.gz"
URL="https://github.com/${REPO}/releases/download/${VERSION}/${ARCHIVE}"

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

log "Downloading rgfx ${VERSION}..."

curl -fL "$URL" -o "$TMP_DIR/$ARCHIVE"

log "Extracting..."

tar -xzf "$TMP_DIR/$ARCHIVE" -C "$TMP_DIR"

BINARY="$(find "$TMP_DIR" -type f -name rgfx -print -quit)"

[ -n "$BINARY" ] || fail "rgfx binary was not found"

mkdir -p "$INSTALL_DIR"
install -m755 "$BINARY" "$INSTALL_DIR/rgfx"

log "Installed to $INSTALL_DIR/rgfx"

case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *)
        printf '\nAdd this to your shell configuration:\n\n'
        printf '    export PATH="$HOME/.local/bin:$PATH"\n\n'
        ;;
esac

log "Installation complete."

printf '\nTry:\n\n'
printf '    rgfx --help\n'
printf '    rgfx model.obj\n'
```

Production releases should add SHA-256 verification.

---

# Arch Linux / CachyOS Packaging

Desired usage:

```bash
yay -S rgfx
```

Possible AUR packages:

```text
rgfx
rgfx-bin
rgfx-git
```

Typical meaning:

```text
rgfx      → stable source build
rgfx-bin  → prebuilt release binary
rgfx-git  → latest main branch
```

---

# Rust Library API

Possible high-level API:

```rust
use rgfx::Viewer;

fn main() -> anyhow::Result<()> {
    Viewer::open("dragon.glb")?.run()?;
    Ok(())
}
```

Low-level API:

```rust
let mut framebuffer = Framebuffer::new(width, height);

scene_renderer.render(&scene, &camera, &mut framebuffer)?;

let output = braille_encoder.encode(&framebuffer)?;

terminal.present(output)?;
```

Potential traits:

```rust
pub trait FrameSource {
    fn next_frame(&mut self) -> Result<Option<Frame>>;
}
```

```rust
pub trait TerminalEncoder {
    fn encode(
        &self,
        frame: &Framebuffer,
        viewport: Viewport
    ) -> TerminalFrame;
}
```

```rust
pub trait SceneRenderer {
    fn render(
        &mut self,
        scene: &Scene,
        camera: &Camera,
        target: &mut Framebuffer,
    ) -> Result<()>;
}
```

---

# Practical Integrations

## SSH

```bash
ssh server
rgfx model.obj
```

No need for:

- X11 forwarding
- Wayland
- VNC
- browser UI
- remote Blender session

## Yazi

Potential preview integration:

```text
select model.obj
→ rgfx renders preview
```

## Ranger

Similar preview support.

## Ratatui

Embed an `rgfx` viewport in other Rust TUI applications.

## AI CLI Interfaces

Possible avatar states:

```text
Idle
Thinking
Speaking
ToolRunning
Success
Error
```

## 3D Printing

```bash
rgfx part.stl
```

Useful for quick geometry inspection before opening a slicer.

## Remote Servers

Useful on headless machines without desktop environments.

---

# Recommended First Release

A strong `v0.1.0` should include:

```text
Linux
OBJ
STL
interactive terminal viewer
Braille renderer
responsive resize
orbit
zoom
z-buffer
flat/smooth shading
wireframe
automatic camera framing
basic status bar
safe terminal restoration
```

CLI:

```bash
rgfx model.obj
rgfx model.stl
```

Controls:

```text
← →        orbit horizontally
↑ ↓        orbit vertically
+ -        zoom
W          wireframe
S          shading
R          reset
Q          quit
```

This is already a complete and useful open-source project.

---

# Suggested Release Roadmap

## v0.1 — 3D Core

- OBJ
- STL
- Braille
- interactive camera
- responsive resize
- wireframe
- shading

## v0.2 — Images

- PNG
- JPEG
- WebP
- dithering
- ASCII
- block rendering

## v0.3 — Modern 3D

- glTF
- GLB
- scenes
- basic materials

## v0.4 — Animation

- GIF
- frame animation API
- CLI avatars

## v0.5 — Video

- FFmpeg
- MP4
- WebM
- MKV
- playback controls

## v0.6 — Color and Integrations

- ANSI color
- Ratatui
- Yazi / Ranger previews

## v0.7 — Advanced 3D

- textures
- glTF animation
- point clouds

## v1.0

- stable public Rust API
- documented extension points
- mature release pipeline
- AUR
- crates.io
- broader platform support

---

# Suggested MVP Cargo Dependencies

Conceptual starting point:

```toml
[dependencies]
anyhow = "1"
clap = { version = "4", features = ["derive"] }
crossterm = "0.29"
glam = "0.30"
image = "0.25"
thiserror = "2"
tobj = "4"
```

Later:

```toml
gltf = "1"
stl_io = "0.8"
rayon = "1"
serde = { version = "1", features = ["derive"] }
toml = "0.9"
tracing = "0.1"
tracing-subscriber = "0.3"
```

Versions above are examples for planning. Verify actual compatible versions when implementation begins.

---

# Development Environment on Arch Linux / CachyOS

Install development tools:

```bash
sudo pacman -S --needed base-devel git rust cargo ffmpeg
```

Clone:

```bash
git clone https://github.com/<OWNER>/rgfx.git
cd rgfx
```

Build:

```bash
cargo build
```

Run:

```bash
cargo run -- model.obj
```

Release build:

```bash
cargo build --release
```

Run the release binary:

```bash
./target/release/rgfx model.obj
```

---

# Example Final Usage

Preview a model:

```bash
rgfx dragon.glb
```

Preview an STL:

```bash
rgfx bracket.stl
```

Force Braille:

```bash
rgfx dragon.obj --renderer braille
```

Wireframe:

```bash
rgfx dragon.obj --wireframe
```

Image:

```bash
rgfx portrait.png
```

Explicit width:

```bash
rgfx portrait.png --width 100
```

or:

```bash
rgfx portrait.png --width 30
```

Video:

```bash
rgfx demo.mp4
```

Traditional ASCII:

```bash
rgfx demo.mp4 --renderer ascii
```

Save generated text:

```bash
rgfx image.png --renderer braille --output image.txt
```

---

# Main Engineering Principle

Do not do this:

```text
OBJ → Braille directly
Video → ASCII directly
Image → terminal directly
```

Do this instead:

```text
OBJ ───────┐
Image ─────┼──→ Framebuffer ──→ Terminal Encoder ──→ Terminal
Video ─────┤
Animation ─┘
```

This creates one reusable graphics engine.

---

# Reference Links

## Primary Braille Reference

**Braille ASCII Art — Lachlan Arthur**

Live:

https://lachlanarthur.github.io/Braille-ASCII-Art/

Repository:

https://github.com/LachlanArthur/Braille-ASCII-Art

This is the primary visual reference for the intended Braille style and responsive width-based output.

---

## img2ascii — SoufianoDev

https://github.com/SoufianoDev/img2ascii

Live:

https://soufianodev.github.io/img2ascii/

---

## img2braille — TheFel0x

https://github.com/TheFel0x/img2braille

---

## ascii-art — ading2210

https://github.com/ading2210/ascii-art

---

## Rust Crates

clap:

https://crates.io/crates/clap

crossterm:

https://crates.io/crates/crossterm

ratatui:

https://crates.io/crates/ratatui

glam:

https://crates.io/crates/glam

nalgebra:

https://crates.io/crates/nalgebra

image:

https://crates.io/crates/image

tobj:

https://crates.io/crates/tobj

gltf:

https://crates.io/crates/gltf

stl_io:

https://crates.io/crates/stl_io

ffmpeg-next:

https://crates.io/crates/ffmpeg-next

rayon:

https://crates.io/crates/rayon

anyhow:

https://crates.io/crates/anyhow

thiserror:

https://crates.io/crates/thiserror

serde:

https://crates.io/crates/serde

tracing:

https://crates.io/crates/tracing

---

# Final Vision

The long-term product should feel this simple:

```bash
rgfx anything_visual
```

and the terminal becomes the viewport.

A model:

```bash
rgfx model.glb
```

becomes an interactive 3D preview.

An image:

```bash
rgfx image.png
```

becomes responsive Braille or ASCII art.

A video:

```bash
rgfx video.mp4
```

becomes a terminal video stream.

A remote machine:

```bash
ssh server
rgfx asset.stl
```

becomes capable of visual asset inspection without a desktop session.

A TUI application can embed the same renderer.

An AI CLI can use the same engine for an animated avatar.

A file manager can use it for image, video, and 3D previews.

The project should therefore be positioned as:

> **rgfx — a universal media and 3D graphics engine for the terminal.**

