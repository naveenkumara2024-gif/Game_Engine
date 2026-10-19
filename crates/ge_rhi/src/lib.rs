//! Render Hardware Interface (lore §3.7, §6.7). The **only** crate that touches the graphics
//! API (law 12). Public types are opaque: no `wgpu` type appears in this crate's API, so a
//! native D3D12/Vulkan backend can slot in behind the same surface later.
//!
//! No scene concepts live here — just device, swapchain, frames and passes.

mod wgpu_backend;

use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

pub use wgpu_backend::{Frame, Rhi, Swapchain};

ge_core::declare_cvar!(pub static R_VSYNC: bool =
    ("r.VSync", true, "Wait for vertical blank when presenting"));
ge_core::declare_cvar!(pub static R_ALLOW_INTEGRATED_GPU: bool = (
    "r.AllowIntegratedGpu",
    false,
    "Allow picking an integrated GPU when a discrete one exists (testing the iGPU path)",
    ge_core::CVarFlags::READ_ONLY,
));

/// Anything that can provide native window + display handles (e.g. a platform window).
pub trait SurfaceSource: HasWindowHandle + HasDisplayHandle + Send + Sync {}
impl<T: HasWindowHandle + HasDisplayHandle + Send + Sync> SurfaceSource for T {}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum GpuKind {
    Discrete,
    Integrated,
    Virtual,
    Cpu,
    Other,
}

/// What we picked, for logs and the future `stat` HUD.
#[derive(Clone, Debug)]
pub struct AdapterSummary {
    pub name: String,
    pub kind: GpuKind,
    pub backend: String,
    pub driver: String,
    pub vendor_id: u32,
}

/// Linear RGBA.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct ClearColor(pub [f32; 4]);

#[derive(Debug, thiserror::Error)]
pub enum RhiError {
    #[error("failed to create surface: {0}")]
    Surface(String),
    #[error("no compatible GPU adapter found: {0}")]
    NoAdapter(String),
    #[error("failed to create device: {0}")]
    Device(String),
    #[error("surface lost and could not be recreated: {0}")]
    SurfaceLost(String),
}
