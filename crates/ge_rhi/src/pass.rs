//! Render pass recording. Every pass has a label (debug markers in RenderDoc/PIX) and, when
//! the device supports it, a begin/end GPU timestamp pair (lore §6.7 "Debuggability").
//! Passes are recorded directly for now; the render graph (M5) will own ordering, barriers and
//! transient resources.

use std::ops::Range;

use crate::timestamps::FrameQueries;
use crate::{BindGroup, Buffer, ClearColor, RenderPipeline, Texture};

#[derive(Copy, Clone, PartialEq, Debug)]
pub enum LoadOp<T> {
    Clear(T),
    Load,
}

#[derive(Copy, Clone, Debug)]
pub struct DepthAttachment<'a> {
    pub texture: &'a Texture,
    pub load: LoadOp<f32>,
    /// Keep the depth result after the pass (needed later for e.g. a prepass).
    pub store: bool,
}

#[derive(Copy, Clone, Debug)]
pub struct RenderPassDesc<'a> {
    /// Shows up in GPU debuggers, timings and Tracy (UE-style: `Opaque`, `DepthPrepass`).
    pub label: &'static str,
    /// Writes the frame's color target; `None` for depth-only passes.
    pub color: Option<LoadOp<ClearColor>>,
    pub depth: Option<DepthAttachment<'a>>,
}

/// A pass being recorded into a [`crate::Frame`]. Ends when dropped.
pub struct RenderPass<'f> {
    pub(crate) inner: wgpu::RenderPass<'f>,
    pub(crate) queries: Option<&'f mut FrameQueries>,
    pub(crate) _span: tracing::span::EnteredSpan,
}

impl std::fmt::Debug for RenderPass<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderPass").finish_non_exhaustive()
    }
}

impl RenderPass<'_> {
    pub fn set_pipeline(&mut self, pipeline: &RenderPipeline) {
        self.inner.set_pipeline(&pipeline.inner);
    }

    pub fn set_bind_group(&mut self, index: u32, group: &BindGroup, dynamic_offsets: &[u32]) {
        self.inner
            .set_bind_group(index, &group.inner, dynamic_offsets);
    }

    pub fn set_vertex_buffer(&mut self, slot: u32, buffer: &Buffer) {
        self.inner.set_vertex_buffer(slot, buffer.inner.slice(..));
    }

    /// 32-bit indices. 16-bit index buffers can come with the mesh cooker (M3/M10).
    pub fn set_index_buffer(&mut self, buffer: &Buffer) {
        self.inner
            .set_index_buffer(buffer.inner.slice(..), wgpu::IndexFormat::Uint32);
    }

    pub fn draw_indexed(&mut self, indices: Range<u32>, base_vertex: i32, instances: Range<u32>) {
        self.inner.draw_indexed(indices, base_vertex, instances);
    }

    pub fn draw(&mut self, vertices: Range<u32>, instances: Range<u32>) {
        self.inner.draw(vertices, instances);
    }
}

impl Drop for RenderPass<'_> {
    fn drop(&mut self) {
        if let Some(q) = self.queries.as_deref_mut() {
            q.end_pass();
        }
    }
}

pub(crate) fn load_color(op: LoadOp<ClearColor>) -> wgpu::LoadOp<wgpu::Color> {
    match op {
        LoadOp::Clear(ClearColor([r, g, b, a])) => wgpu::LoadOp::Clear(wgpu::Color {
            r: f64::from(r),
            g: f64::from(g),
            b: f64::from(b),
            a: f64::from(a),
        }),
        LoadOp::Load => wgpu::LoadOp::Load,
    }
}

pub(crate) fn load_depth(op: LoadOp<f32>) -> wgpu::LoadOp<f32> {
    match op {
        LoadOp::Clear(d) => wgpu::LoadOp::Clear(d),
        LoadOp::Load => wgpu::LoadOp::Load,
    }
}
