//! Renderer (lore §3.7, §6.7). Owns scene-level rendering: cameras, meshes, lights, uniforms,
//! passes. Everything GPU-facing goes through `ge_rhi`; this crate never names a graphics API
//! type (law 12).
//!
//! Scope so far: one forward pass with reversed-Z depth, lighting v0 (punctual lights, no
//! shadows), Blender-style color management, a grid overlay and a UI pass, plus a
//! single-threaded extract into [`RenderSnapshot`]. The render graph and the render thread
//! arrive in M5; shadows, IBL and clustering in M4.

#![forbid(unsafe_code)]

pub mod assets;
mod camera;
mod components;
pub mod demo;
mod frame;
mod ibl;
pub mod lighting;
mod mesh;
pub mod post;
pub mod primitives;
mod renderer;
pub mod scene;
pub mod settings;
pub mod shadows;
pub mod ui;

pub use camera::{Camera, aspect_ratio};
pub use components::{
    Camera3d, CameraOverride, MeshRenderer, RenderSnapshot, UiOutput, ViewportOverlays, Visibility,
    extract_render_snapshot,
};
pub use lighting::{
    AreaShape, Light, LightKind, Material, RenderMethod, RenderWorld, WorldSettings, WorldSurface,
};
pub use mesh::{GpuMesh, MeshData, Vertex};
pub use post::BloomSettings;
pub use primitives::{Primitive, PrimitiveMesh};
pub use renderer::{
    DrawAlpha, DrawItem, FrameInputs, MAX_OBJECTS, MeshKind, Overlays, R_DEBUG_NORMALS,
    RenderError, RenderView, Renderer,
};
pub use scene::{SceneNode, SceneRoot, SceneSpawned, spawn_scenes};
