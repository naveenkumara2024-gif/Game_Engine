//! Compute pipelines and passes (plan 0006: clustered light culling, IBL prefiltering, later
//! GPU culling at M12). Same rules as render passes: labelled, timestamped, no wgpu types in
//! the public API.

use crate::timestamps::FrameQueries;
use crate::{BindGroup, BindGroupLayout, Rhi, RhiError, ShaderModule};

#[derive(Copy, Clone, Debug)]
pub struct ComputePipelineDesc<'a> {
    pub label: &'a str,
    pub shader: &'a ShaderModule,
    pub entry: &'a str,
    pub bind_group_layouts: &'a [&'a BindGroupLayout],
}

#[derive(Clone, Debug)]
pub struct ComputePipeline {
    pub(crate) inner: wgpu::ComputePipeline,
}

/// A compute pass being recorded into a [`crate::Frame`]. Ends when dropped.
pub struct ComputePass<'f> {
    pub(crate) inner: wgpu::ComputePass<'f>,
    pub(crate) queries: Option<&'f mut FrameQueries>,
    pub(crate) _span: tracing::span::EnteredSpan,
}

impl std::fmt::Debug for ComputePass<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComputePass").finish_non_exhaustive()
    }
}

impl ComputePass<'_> {
    pub fn set_pipeline(&mut self, pipeline: &ComputePipeline) {
        self.inner.set_pipeline(&pipeline.inner);
    }

    pub fn set_bind_group(&mut self, index: u32, group: &BindGroup, dynamic_offsets: &[u32]) {
        self.inner
            .set_bind_group(index, &group.inner, dynamic_offsets);
    }

    /// Runs `x × y × z` workgroups.
    pub fn dispatch(&mut self, x: u32, y: u32, z: u32) {
        self.inner.dispatch_workgroups(x.max(1), y.max(1), z.max(1));
    }
}

impl Drop for ComputePass<'_> {
    fn drop(&mut self) {
        if let Some(q) = self.queries.as_deref_mut() {
            q.end_pass();
        }
    }
}

impl Rhi {
    /// Builds a compute pipeline; validation errors come back as [`RhiError::Pipeline`].
    pub fn create_compute_pipeline(
        &self,
        desc: &ComputePipelineDesc<'_>,
    ) -> Result<ComputePipeline, RhiError> {
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let groups: Vec<Option<&wgpu::BindGroupLayout>> = desc
            .bind_group_layouts
            .iter()
            .map(|l| Some(&l.inner))
            .collect();
        let layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(desc.label),
                bind_group_layouts: &groups,
                immediate_size: 0,
            });
        let inner = self
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(desc.label),
                layout: Some(&layout),
                module: &desc.shader.inner,
                entry_point: Some(desc.entry),
                compilation_options: Default::default(),
                cache: None,
            });
        match pollster::block_on(scope.pop()) {
            None => Ok(ComputePipeline { inner }),
            Some(e) => Err(RhiError::Pipeline {
                label: desc.label.to_owned(),
                message: e.to_string(),
            }),
        }
    }

    /// Device limits that shape engine buffers (logged at startup).
    pub fn limits(&self) -> RhiLimits {
        let l = self.device.limits();
        RhiLimits {
            max_texture_2d: l.max_texture_dimension_2d,
            max_texture_layers: l.max_texture_array_layers,
            max_storage_buffer_bytes: l.max_storage_buffer_binding_size,
            max_workgroup_invocations: l.max_compute_invocations_per_workgroup,
            max_storage_buffers_per_stage: l.max_storage_buffers_per_shader_stage,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RhiLimits {
    pub max_texture_2d: u32,
    pub max_texture_layers: u32,
    pub max_storage_buffer_bytes: u64,
    pub max_workgroup_invocations: u32,
    pub max_storage_buffers_per_stage: u32,
}
