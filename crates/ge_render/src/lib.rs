//! Renderer (lore §3.7, §6.7). Owns scene-level rendering: cameras, meshes, uniforms, passes.
//! Everything GPU-facing goes through `ge_rhi`; this crate never names a graphics API type
//! (law 12).
//!
//! Scope so far: one opaque pass, reversed-Z depth, indexed meshes, hot-reloadable WESL shaders,
//! and a single-threaded extract into [`RenderSnapshot`]. The render graph and the render thread
//! arrive in M5.

#![forbid(unsafe_code)]

mod camera;
mod components;
pub mod demo;
mod mesh;
mod renderer;

pub use camera::{Camera, aspect_ratio};
pub use components::{Camera3d, MeshRenderer, RenderSnapshot, extract_render_snapshot};
pub use mesh::{GpuMesh, MeshData, Vertex};
pub use renderer::{
    DrawItem, MAX_OBJECTS, MeshKind, R_DEBUG_NORMALS, RenderError, RenderView, Renderer,
};
