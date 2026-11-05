//! HDR targets and post-processing (plan 0006, decisions 1 and 5): the scene renders into
//! `SceneColor` (Rgba16Float) + `SceneDepth`; bloom (dual filter) reads it; tonemap writes the
//! display target. All three are render-graph transients (plan 0008). The graph culls bloom's
//! passes when the tonemap doesn't read their result.

use bytemuck::{Pod, Zeroable};
use ge_rhi::{
    AddressMode, BindGroup, BindGroupEntry, BindGroupLayout, BindGroupLayoutEntry, BindingResource,
    BindingType, BlendMode, Buffer, BufferDesc, BufferUsage, ClearColor, CullMode, FilterMode,
    LoadOp, RenderPipeline, RenderPipelineDesc, Rhi, Sampler, SamplerDesc, ShaderStages, Texture,
    TextureDesc, TextureDim, TextureFormat, TextureUsage, TextureView,
};
use ge_shader::ShaderLibrary;
use tracing::error;

use crate::graph::{GraphTexture, RenderGraph, TransientDesc};

ge_core::declare_cvar!(pub static R_BLOOM_INTENSITY: f32 = (
    "r.Bloom.Intensity",
    0.04,
    "How much of the bloom is added to the image (0 = off)"
));
ge_core::declare_cvar!(pub static R_BLOOM_THRESHOLD: f32 = (
    "r.Bloom.Threshold",
    1.0,
    "Brightness (scene-linear) where bloom starts"
));
ge_core::declare_cvar!(pub static R_BLOOM_KNEE: f32 = (
    "r.Bloom.Knee",
    0.5,
    "Softness of the threshold: how gradually bloom fades in below it"
));
ge_core::declare_cvar!(pub static R_BLOOM_RADIUS: f32 = (
    "r.Bloom.Radius",
    1.0,
    "Spread of each upsample step (larger = wider glow)"
));

pub(crate) const SCENE_COLOR_FORMAT: TextureFormat = TextureFormat::Rgba16Float;
pub(crate) const DEPTH_FORMAT: TextureFormat = TextureFormat::Depth32Float;
const BLOOM_MIPS: u32 = 6;

/// Bloom settings for one frame (read from cvars by the caller; `intensity` 0 skips bloom).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BloomSettings {
    pub intensity: f32,
    pub threshold: f32,
    pub knee: f32,
    pub radius: f32,
}

impl Default for BloomSettings {
    fn default() -> Self {
        Self::OFF
    }
}

impl BloomSettings {
    pub const OFF: Self = Self {
        intensity: 0.0,
        threshold: 1.0,
        knee: 0.5,
        radius: 1.0,
    };

    pub fn from_cvars() -> Self {
        Self {
            intensity: R_BLOOM_INTENSITY.get().max(0.0),
            threshold: R_BLOOM_THRESHOLD.get().max(0.0),
            knee: R_BLOOM_KNEE.get().max(0.0),
            radius: R_BLOOM_RADIUS.get().clamp(0.1, 4.0),
        }
    }
}

/// Matches `Params` in `shaders/bloom.wesl`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, Pod, Zeroable)]
struct BloomParams {
    src_texel: [f32; 2],
    threshold: f32,
    knee: f32,
    radius: f32,
    first: f32,
    _pad: [f32; 2],
}

pub(crate) struct Post {
    sampler: Sampler,
    tonemap_layout: BindGroupLayout,
    bloom_layout: BindGroupLayout,
    tonemap: Option<RenderPipeline>,
    bloom_down: Option<RenderPipeline>,
    bloom_up: Option<RenderPipeline>,
    params: Buffer,
    params_stride: u64,
    output_format: TextureFormat,
    /// Bound in place of the bloom texture when bloom is culled (intensity 0).
    #[allow(dead_code)]
    black: Texture,
    black_view: TextureView,
}

impl std::fmt::Debug for Post {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Post")
            .field("pipelines", &self.has_pipelines())
            .finish_non_exhaustive()
    }
}

impl Post {
    pub(crate) fn new(rhi: &Rhi, output_format: TextureFormat, view_layout_size: u64) -> Self {
        let f = ShaderStages::Fragment;
        let e = |binding, visibility, ty| BindGroupLayoutEntry {
            binding,
            visibility,
            ty,
        };
        let tonemap_layout = rhi.create_bind_group_layout(
            "Tonemap",
            &[
                e(
                    0,
                    ShaderStages::VertexFragment,
                    BindingType::UniformBuffer {
                        has_dynamic_offset: false,
                        min_binding_size: view_layout_size,
                    },
                ),
                e(
                    1,
                    f,
                    BindingType::Texture {
                        sample: ge_rhi::SampleKind::Float { filterable: false },
                        dim: TextureDim::D2,
                    },
                ),
                e(2, f, BindingType::Texture2d),
                e(3, f, BindingType::Sampler),
            ],
        );
        let bloom_layout = rhi.create_bind_group_layout(
            "Bloom",
            &[
                e(
                    0,
                    f,
                    BindingType::UniformBuffer {
                        has_dynamic_offset: true,
                        min_binding_size: size_of::<BloomParams>() as u64,
                    },
                ),
                e(1, f, BindingType::Texture2d),
                e(2, f, BindingType::Sampler),
            ],
        );
        let params_stride =
            (size_of::<BloomParams>() as u64).next_multiple_of(rhi.uniform_offset_alignment());
        let params = rhi.create_buffer(&BufferDesc {
            label: "BloomParams",
            size: params_stride * u64::from(BLOOM_MIPS * 2 + 2),
            usage: BufferUsage::UNIFORM | BufferUsage::COPY_DST,
        });
        let sampler = rhi.create_sampler(&SamplerDesc {
            label: "PostLinear",
            filter: FilterMode::Linear,
            address: AddressMode::ClampToEdge,
            mipmaps: false,
            anisotropy: 1,
            compare: None,
        });
        // wgpu zero-initializes textures, so this samples as black.
        let black = rhi.create_texture(&TextureDesc {
            label: "BloomOff",
            size: (1, 1),
            format: SCENE_COLOR_FORMAT,
            usage: TextureUsage::TEXTURE_BINDING,
        });
        Self {
            sampler,
            tonemap_layout,
            bloom_layout,
            tonemap: None,
            bloom_down: None,
            bloom_up: None,
            params,
            params_stride,
            output_format,
            black_view: black.full_view(),
            black,
        }
    }

    pub(crate) fn rebuild_pipelines(&mut self, rhi: &Rhi, shaders: &ShaderLibrary) -> bool {
        let fullscreen = |name: &str, layout: &BindGroupLayout, fs: &'static str, format, blend| {
            shaders
                .compile(name, &[])
                .map_err(|e| e.to_string())
                .and_then(|c| {
                    rhi.create_shader_module(name, &c.wgsl)
                        .map_err(|e| e.to_string())
                })
                .and_then(|m| {
                    rhi.create_render_pipeline(&RenderPipelineDesc {
                        label: fs,
                        shader: &m,
                        vertex_entry: "vs_main",
                        fragment_entry: Some(fs),
                        vertex_layouts: &[],
                        bind_group_layouts: &[layout],
                        color_formats: &[format],
                        depth: None,
                        cull_mode: CullMode::None,
                        blend,
                        depth_bias: None,
                    })
                    .map_err(|e| e.to_string())
                })
        };
        let mut ok = true;
        let mut keep = |slot: &mut Option<RenderPipeline>,
                        r: Result<RenderPipeline, String>,
                        what: &str| match r {
            Ok(p) => *slot = Some(p),
            Err(e) => {
                error!("{what} shader rejected:\n{e}");
                ok = false;
            }
        };
        keep(
            &mut self.tonemap,
            fullscreen(
                "tonemap",
                &self.tonemap_layout,
                "fs_main",
                self.output_format,
                None,
            ),
            "tonemap",
        );
        keep(
            &mut self.bloom_down,
            fullscreen(
                "bloom",
                &self.bloom_layout,
                "fs_down",
                SCENE_COLOR_FORMAT,
                None,
            ),
            "bloom",
        );
        keep(
            &mut self.bloom_up,
            fullscreen(
                "bloom",
                &self.bloom_layout,
                "fs_up",
                SCENE_COLOR_FORMAT,
                Some(BlendMode::Additive),
            ),
            "bloom",
        );
        ok
    }

    pub(crate) fn has_pipelines(&self) -> bool {
        self.tonemap.is_some() && self.bloom_down.is_some() && self.bloom_up.is_some()
    }

    /// Bloom chain sizes for a `size` frame: half resolution, then halving per level.
    pub(crate) fn bloom_sizes(size: (u32, u32)) -> Vec<(u32, u32)> {
        let half = ((size.0 / 2).max(1), (size.1 / 2).max(1));
        let levels = BLOOM_MIPS.min(half.0.min(half.1).max(1).ilog2() + 1);
        (0..levels)
            .map(|m| ((half.0 >> m).max(1), (half.1 >> m).max(1)))
            .collect()
    }

    fn bloom_group(&self, rhi: &Rhi, src: &TextureView) -> BindGroup {
        rhi.create_bind_group(
            "Bloom",
            &self.bloom_layout,
            &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::Buffer {
                        buffer: &self.params,
                        offset: 0,
                        size: Some(size_of::<BloomParams>() as u64),
                    },
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::View(src),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: BindingResource::Sampler(&self.sampler),
                },
            ],
        )
    }

    /// Adds the dual-filter bloom chain reading `scene` and returns its result (level 0).
    /// The passes are always declared; when the tonemap doesn't read the result (intensity
    /// 0) the graph culls them. `None` when the bloom shaders failed to build.
    pub(crate) fn add_bloom<'a>(
        &'a self,
        g: &mut RenderGraph<'a>,
        rhi: &'a Rhi,
        scene: GraphTexture,
        size: (u32, u32),
        settings: BloomSettings,
    ) -> Option<GraphTexture> {
        let (Some(down), Some(up)) = (&self.bloom_down, &self.bloom_up) else {
            return None;
        };
        let sizes = Self::bloom_sizes(size);
        let n = sizes.len();
        // Params: down passes 0..n (source = scene, then levels), then up passes.
        let mut params = Vec::with_capacity(2 * n);
        let mut src = size;
        for (m, &level) in sizes.iter().enumerate() {
            params.push(BloomParams {
                src_texel: [1.0 / src.0 as f32, 1.0 / src.1 as f32],
                threshold: settings.threshold,
                knee: settings.knee,
                radius: settings.radius,
                first: if m == 0 { 1.0 } else { 0.0 },
                _pad: [0.0; 2],
            });
            src = level;
        }
        for &s in sizes[1..].iter().rev() {
            params.push(BloomParams {
                src_texel: [1.0 / s.0 as f32, 1.0 / s.1 as f32],
                radius: settings.radius,
                ..Default::default()
            });
        }
        let stride = self.params_stride as usize;
        let mut bytes = vec![0u8; params.len() * stride];
        for (i, p) in params.iter().enumerate() {
            bytes[i * stride..i * stride + size_of::<BloomParams>()]
                .copy_from_slice(bytemuck::bytes_of(p));
        }
        rhi.write_buffer(&self.params, 0, &bytes);

        let levels: Vec<GraphTexture> = sizes
            .iter()
            .map(|&s| g.create_texture("Bloom", TransientDesc::target(s, SCENE_COLOR_FORMAT)))
            .collect();
        for m in 0..n {
            let source = if m == 0 { scene } else { levels[m - 1] };
            let offset = (m * stride) as u32;
            g.raster_pass("Bloom.Down")
                .color(levels[m], LoadOp::Clear(ClearColor([0.0; 4])))
                .read(source)
                .run(move |pass, res| {
                    let group = self.bloom_group(rhi, res.texture(source));
                    pass.set_pipeline(down);
                    pass.set_bind_group(0, &group, &[offset]);
                    pass.draw(0..3, 0..1);
                });
        }
        for (k, m) in (1..n).rev().enumerate() {
            let (source, target) = (levels[m], levels[m - 1]);
            let offset = ((n + k) * stride) as u32;
            g.raster_pass("Bloom.Up")
                .color(target, LoadOp::Load)
                .read(source)
                .run(move |pass, res| {
                    let group = self.bloom_group(rhi, res.texture(source));
                    pass.set_pipeline(up);
                    pass.set_bind_group(0, &group, &[offset]);
                    pass.draw(0..3, 0..1);
                });
        }
        levels.first().copied()
    }

    /// Tonemaps `scene` (+ `bloom`, when it's on) into `output`.
    pub(crate) fn add_tonemap<'a>(
        &'a self,
        g: &mut RenderGraph<'a>,
        rhi: &'a Rhi,
        view_buffer: &'a Buffer,
        scene: GraphTexture,
        bloom: Option<GraphTexture>,
        output: GraphTexture,
    ) {
        let mut pass = g
            .raster_pass("Tonemap")
            .color(output, LoadOp::Clear(ClearColor([0.0, 0.0, 0.0, 1.0])))
            .read(scene);
        if let Some(bloom) = bloom {
            pass = pass.read(bloom);
        }
        pass.run(move |pass, res| {
            let Some(pipeline) = &self.tonemap else {
                return;
            };
            let bloom_view = bloom.map_or(&self.black_view, |b| res.texture(b));
            let group = rhi.create_bind_group(
                "Tonemap",
                &self.tonemap_layout,
                &[
                    BindGroupEntry {
                        binding: 0,
                        resource: BindingResource::Buffer {
                            buffer: view_buffer,
                            offset: 0,
                            size: None,
                        },
                    },
                    BindGroupEntry {
                        binding: 1,
                        resource: BindingResource::View(res.texture(scene)),
                    },
                    BindGroupEntry {
                        binding: 2,
                        resource: BindingResource::View(bloom_view),
                    },
                    BindGroupEntry {
                        binding: 3,
                        resource: BindingResource::Sampler(&self.sampler),
                    },
                ],
            );
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bloom_levels_halve_from_half_resolution() {
        assert_eq!(
            Post::bloom_sizes((1920, 1080)),
            [
                (960, 540),
                (480, 270),
                (240, 135),
                (120, 67),
                (60, 33),
                (30, 16)
            ]
        );
        // Tiny frames get fewer levels, never a zero size.
        assert_eq!(Post::bloom_sizes((4, 2)), [(2, 1)]);
    }
}
