//! Frame-global GPU state (plan 0006): the view uniform, the light list, the cluster grid and
//! the group-0 bind group every scene pass shares (view, lights, clusters, shadows,
//! environment). Also the light-culling compute pass.

use bytemuck::{Pod, Zeroable};
use ge_rhi::{
    BindGroup, BindGroupEntry, BindGroupLayout, BindGroupLayoutEntry, BindingResource, BindingType,
    Buffer, BufferDesc, BufferUsage, ComputePipeline, ComputePipelineDesc, Frame, Rhi, SampleKind,
    Sampler, ShaderStages, Texture, TextureDim,
};
use ge_shader::ShaderLibrary;
use tracing::error;

use crate::lighting::{LightUniform, MAX_LIGHTS};

/// Cluster grid (plan 0006, decision 2): 16 × 9 screen tiles × 24 exponential depth slices.
pub(crate) const CLUSTERS: [u32; 3] = [16, 9, 24];
/// Mirrors `MAX_LIGHTS_PER_CLUSTER` in `shaders/common/lighting.wesl`.
pub(crate) const MAX_LIGHTS_PER_CLUSTER: u32 = 128;
pub(crate) const CLUSTER_COUNT: u32 = CLUSTERS[0] * CLUSTERS[1] * CLUSTERS[2];

ge_core::declare_cvar!(pub static R_CLUSTERS_FAR: f32 = (
    "r.Clusters.Far",
    300.0,
    "View depth (m) where the last cluster slice begins; lights farther away share it"
));

ge_core::declare_cvar!(pub static R_CLUSTERS_BRUTE_FORCE: bool = (
    "r.Clusters.BruteForce",
    false,
    "Debug: shade every light for every pixel instead of the cluster lists (the reference      for clustered shading; slow)",
    ge_core::cvar::CVarFlags::CHEAT
));

/// Matches `View` in `shaders/common/view.wesl` (400 bytes).
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub(crate) struct ViewUniform {
    pub clip_from_world: [[f32; 4]; 4],
    pub world_from_clip: [[f32; 4]; 4],
    pub view_from_world: [[f32; 4]; 4],
    pub view_from_clip: [[f32; 4]; 4],
    pub position_time: [f32; 4],
    pub color_management: [f32; 4],
    pub white_balance: [f32; 4],
    pub ambient: [f32; 4],
    pub screen: [f32; 4],
    pub cluster_dims: [u32; 4],
    pub counts: [u32; 4],
    pub params: [f32; 4],
    pub background: [f32; 4],
}

/// Exponential slice mapping: `slice = log(z) · scale + bias` (matches `cluster_index`).
pub(crate) fn slice_params(near: f32, far: f32) -> (f32, f32) {
    let ratio = (far / near).max(1.0001).ln();
    let z = CLUSTERS[2] as f32;
    (z / ratio, -z * near.ln() / ratio)
}

/// Textures and buffers from other systems that group 0 points at.
pub(crate) struct GroupInputs<'a> {
    pub cascade_maps: &'a Texture,
    pub shadow_atlas: &'a Texture,
    pub shadow_sampler: &'a Sampler,
    pub shadow_data: &'a Buffer,
    pub env_cube: &'a Texture,
    pub env_sampler: &'a Sampler,
    pub brdf_lut: &'a Texture,
    pub env_sh: &'a Buffer,
}

pub(crate) struct FrameResources {
    pub layout: BindGroupLayout,
    cull_layout: BindGroupLayout,
    pub view_buffer: Buffer,
    pub lights_buffer: Buffer,
    cluster_counts: Buffer,
    cluster_lights: Buffer,
    cull_group: BindGroup,
    cull: Option<ComputePipeline>,
    pub group: Option<BindGroup>,
    light_staging: Vec<LightUniform>,
}

impl std::fmt::Debug for FrameResources {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameResources").finish_non_exhaustive()
    }
}

fn entry(binding: u32, visibility: ShaderStages, ty: BindingType) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding,
        visibility,
        ty,
    }
}

fn buffer(buffer: &Buffer) -> BindingResource<'_> {
    BindingResource::Buffer {
        buffer,
        offset: 0,
        size: None,
    }
}

impl FrameResources {
    pub(crate) fn new(rhi: &Rhi) -> Self {
        let view_size = size_of::<ViewUniform>() as u64;
        let uniform = BindingType::UniformBuffer {
            has_dynamic_offset: false,
            min_binding_size: view_size,
        };
        let read = BindingType::StorageBuffer { read_only: true };
        let f = ShaderStages::Fragment;
        let layout = rhi.create_bind_group_layout(
            "FrameGroup",
            &[
                entry(0, ShaderStages::VertexFragment, uniform),
                entry(1, f, read),
                entry(2, f, read),
                entry(3, f, read),
                entry(
                    4,
                    f,
                    BindingType::Texture {
                        sample: SampleKind::Depth,
                        dim: TextureDim::D2Array,
                    },
                ),
                entry(
                    5,
                    f,
                    BindingType::Texture {
                        sample: SampleKind::Depth,
                        dim: TextureDim::D2,
                    },
                ),
                entry(6, f, BindingType::ComparisonSampler),
                entry(7, f, read),
                entry(
                    8,
                    f,
                    BindingType::Texture {
                        sample: SampleKind::Float { filterable: true },
                        dim: TextureDim::Cube,
                    },
                ),
                entry(9, f, BindingType::Sampler),
                entry(10, f, BindingType::Texture2d),
                entry(11, f, read),
            ],
        );
        let c = ShaderStages::Compute;
        let cull_layout = rhi.create_bind_group_layout(
            "LightCulling",
            &[
                entry(0, c, uniform),
                entry(1, c, read),
                entry(2, c, BindingType::StorageBuffer { read_only: false }),
                entry(3, c, BindingType::StorageBuffer { read_only: false }),
            ],
        );
        let view_buffer = rhi.create_buffer(&BufferDesc {
            label: "View",
            size: view_size,
            usage: BufferUsage::UNIFORM | BufferUsage::COPY_DST,
        });
        let lights_buffer = rhi.create_buffer(&BufferDesc {
            label: "Lights",
            size: (size_of::<LightUniform>() * MAX_LIGHTS) as u64,
            usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
        });
        let cluster_counts = rhi.create_buffer(&BufferDesc {
            label: "ClusterCounts",
            size: u64::from(CLUSTER_COUNT) * 4,
            usage: BufferUsage::STORAGE,
        });
        let cluster_lights = rhi.create_buffer(&BufferDesc {
            label: "ClusterLights",
            size: u64::from(CLUSTER_COUNT * MAX_LIGHTS_PER_CLUSTER) * 4,
            usage: BufferUsage::STORAGE,
        });
        let cull_group = rhi.create_bind_group(
            "LightCulling",
            &cull_layout,
            &[
                BindGroupEntry {
                    binding: 0,
                    resource: buffer(&view_buffer),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: buffer(&lights_buffer),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: buffer(&cluster_counts),
                },
                BindGroupEntry {
                    binding: 3,
                    resource: buffer(&cluster_lights),
                },
            ],
        );
        Self {
            layout,
            cull_layout,
            view_buffer,
            lights_buffer,
            cluster_counts,
            cluster_lights,
            cull_group,
            cull: None,
            group: None,
            light_staging: Vec::new(),
        }
    }

    pub(crate) fn rebuild_pipelines(&mut self, rhi: &Rhi, shaders: &ShaderLibrary) -> bool {
        let built = shaders
            .compile("cluster_cull", &[])
            .map_err(|e| e.to_string())
            .and_then(|c| {
                rhi.create_shader_module("cluster_cull", &c.wgsl)
                    .map_err(|e| e.to_string())
            })
            .and_then(|m| {
                rhi.create_compute_pipeline(&ComputePipelineDesc {
                    label: "LightCulling",
                    shader: &m,
                    entry: "cull",
                    bind_group_layouts: &[&self.cull_layout],
                })
                .map_err(|e| e.to_string())
            });
        match built {
            Ok(p) => {
                self.cull = Some(p);
                true
            }
            Err(e) => {
                error!("light culling shader rejected:\n{e}");
                false
            }
        }
    }

    pub(crate) fn has_cull(&self) -> bool {
        self.cull.is_some()
    }

    /// (Re)builds group 0 after the shadow or environment textures changed.
    pub(crate) fn build_group(&mut self, rhi: &Rhi, inputs: &GroupInputs<'_>) {
        self.group = Some(rhi.create_bind_group(
            "FrameGroup",
            &self.layout,
            &[
                BindGroupEntry {
                    binding: 0,
                    resource: buffer(&self.view_buffer),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: buffer(&self.lights_buffer),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: buffer(&self.cluster_counts),
                },
                BindGroupEntry {
                    binding: 3,
                    resource: buffer(&self.cluster_lights),
                },
                BindGroupEntry {
                    binding: 4,
                    resource: BindingResource::Texture(inputs.cascade_maps),
                },
                BindGroupEntry {
                    binding: 5,
                    resource: BindingResource::Texture(inputs.shadow_atlas),
                },
                BindGroupEntry {
                    binding: 6,
                    resource: BindingResource::Sampler(inputs.shadow_sampler),
                },
                BindGroupEntry {
                    binding: 7,
                    resource: buffer(inputs.shadow_data),
                },
                BindGroupEntry {
                    binding: 8,
                    resource: BindingResource::Texture(inputs.env_cube),
                },
                BindGroupEntry {
                    binding: 9,
                    resource: BindingResource::Sampler(inputs.env_sampler),
                },
                BindGroupEntry {
                    binding: 10,
                    resource: BindingResource::Texture(inputs.brdf_lut),
                },
                BindGroupEntry {
                    binding: 11,
                    resource: buffer(inputs.env_sh),
                },
            ],
        ));
    }

    pub(crate) fn upload(&mut self, rhi: &Rhi, view: &ViewUniform, lights: &[LightUniform]) {
        rhi.write_buffer(&self.view_buffer, 0, bytemuck::bytes_of(view));
        self.light_staging.clear();
        self.light_staging
            .extend_from_slice(&lights[..lights.len().min(MAX_LIGHTS)]);
        if self.light_staging.is_empty() {
            self.light_staging.push(LightUniform::zeroed());
        }
        rhi.write_buffer(
            &self.lights_buffer,
            0,
            bytemuck::cast_slice(&self.light_staging),
        );
    }

    /// Light culling: one invocation per cluster.
    pub(crate) fn cull(&self, frame: &mut Frame) {
        let Some(pipeline) = &self.cull else { return };
        let mut pass = frame.compute_pass("LightCulling");
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &self.cull_group, &[]);
        pass.dispatch(CLUSTER_COUNT.div_ceil(64), 1, 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_uniform_matches_wgsl() {
        assert_eq!(size_of::<ViewUniform>(), 400);
    }

    /// The fragment-side slice formula maps each slice's near depth to its index.
    #[test]
    fn slice_boundaries_round_trip() {
        let (near, far) = (0.1f32, 300.0f32);
        let (scale, bias) = slice_params(near, far);
        for k in 0..CLUSTERS[2] {
            let z = near * (far / near).powf((k as f32 + 0.5) / CLUSTERS[2] as f32);
            let slice = (z.ln() * scale + bias).floor();
            assert_eq!(slice as u32, k, "depth {z}");
        }
    }
}
