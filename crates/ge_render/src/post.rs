//! HDR targets and post-processing (plan 0006, decisions 1 and 5): the scene renders into
//! `SceneColor` (Rgba16Float) + `SceneDepth`; bloom (dual filter) reads it; tonemap writes the
//! display target. The transient textures are owned here until the render graph (M5).

use bytemuck::{Pod, Zeroable};
use ge_rhi::{
    AddressMode, BindGroup, BindGroupEntry, BindGroupLayout, BindGroupLayoutEntry, BindingResource,
    BindingType, BlendMode, Buffer, BufferDesc, BufferUsage, ClearColor, ColorTarget, CullMode,
    FilterMode, Frame, LoadOp, RenderPipeline, RenderPipelineDesc, Rhi, Sampler, SamplerDesc,
    ShaderStages, TargetPassDesc, Texture, TextureDesc, TextureDescEx, TextureDim, TextureFormat,
    TextureUsage, TextureView, ViewRange,
};
use ge_shader::ShaderLibrary;
use tracing::error;

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

pub(crate) struct Targets {
    pub size: (u32, u32),
    // The textures are kept alive here; passes use the views.
    #[allow(dead_code)]
    pub color: Texture,
    pub color_view: TextureView,
    #[allow(dead_code)]
    pub depth: Texture,
    pub depth_view: TextureView,
    #[allow(dead_code)]
    bloom: Texture,
    bloom_views: Vec<TextureView>,
    bloom_mips: u32,
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
    pub targets: Option<Targets>,
    tonemap_group: Option<BindGroup>,
    bloom_groups: Vec<BindGroup>,
}

impl std::fmt::Debug for Post {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Post")
            .field("size", &self.targets.as_ref().map(|t| t.size))
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
            targets: None,
            tonemap_group: None,
            bloom_groups: Vec::new(),
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

    /// (Re)creates the HDR targets for `size`. Returns true when they were replaced.
    pub(crate) fn ensure_targets(
        &mut self,
        rhi: &Rhi,
        size: (u32, u32),
        view_buffer: &Buffer,
    ) -> bool {
        if self.targets.as_ref().is_some_and(|t| t.size == size) && self.tonemap_group.is_some() {
            return false;
        }
        let color = rhi.create_texture(&TextureDesc {
            label: "SceneColor",
            size,
            format: SCENE_COLOR_FORMAT,
            usage: TextureUsage::RENDER_ATTACHMENT | TextureUsage::TEXTURE_BINDING,
        });
        let depth = rhi.create_texture(&TextureDesc {
            label: "SceneDepth",
            size,
            format: DEPTH_FORMAT,
            usage: TextureUsage::RENDER_ATTACHMENT | TextureUsage::TEXTURE_BINDING,
        });
        let half = ((size.0 / 2).max(1), (size.1 / 2).max(1));
        let bloom_mips = BLOOM_MIPS.min(half.0.min(half.1).max(1).ilog2() + 1);
        let bloom = rhi.create_texture_ex(&TextureDescEx {
            label: "Bloom",
            size: half,
            format: SCENE_COLOR_FORMAT,
            usage: TextureUsage::RENDER_ATTACHMENT | TextureUsage::TEXTURE_BINDING,
            mip_levels: bloom_mips,
            layers: 1,
            dim: TextureDim::D2,
        });
        let bloom_views: Vec<TextureView> = (0..bloom_mips)
            .map(|m| bloom.view(ViewRange::level(m, 0)))
            .collect();

        let color_view = color.full_view();
        self.tonemap_group = Some(rhi.create_bind_group(
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
                    resource: BindingResource::View(&color_view),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: BindingResource::View(&bloom_views[0]),
                },
                BindGroupEntry {
                    binding: 3,
                    resource: BindingResource::Sampler(&self.sampler),
                },
            ],
        ));
        // Bloom groups: [down sources: scene, mip0..mip(n-2)] then [up sources: mip(n-1)..mip1].
        let params_binding = BindGroupEntry {
            binding: 0,
            resource: BindingResource::Buffer {
                buffer: &self.params,
                offset: 0,
                size: Some(size_of::<BloomParams>() as u64),
            },
        };
        let group = |src: &TextureView| {
            rhi.create_bind_group(
                "Bloom",
                &self.bloom_layout,
                &[
                    params_binding,
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
        };
        let mut groups = vec![group(&color_view)];
        for m in 0..bloom_mips.saturating_sub(1) {
            groups.push(group(&bloom_views[m as usize]));
        }
        for m in (1..bloom_mips).rev() {
            groups.push(group(&bloom_views[m as usize]));
        }
        self.bloom_groups = groups;
        self.targets = Some(Targets {
            size,
            depth_view: depth.full_view(),
            color_view,
            color,
            depth,
            bloom,
            bloom_views,
            bloom_mips,
        });
        true
    }

    /// Bloom chain into the bloom texture's mip 0 (skipped when `settings.intensity` is 0).
    pub(crate) fn bloom(&self, rhi: &Rhi, frame: &mut Frame, settings: BloomSettings) {
        let (Some(t), Some(down), Some(up)) = (&self.targets, &self.bloom_down, &self.bloom_up)
        else {
            return;
        };
        if settings.intensity <= 0.0 {
            return;
        }
        let n = t.bloom_mips as usize;
        // Params: down passes 0..n (source = scene, then mips), up passes.
        let mut params = Vec::new();
        let mut src = t.size;
        for m in 0..n {
            params.push(BloomParams {
                src_texel: [1.0 / src.0 as f32, 1.0 / src.1 as f32],
                threshold: settings.threshold,
                knee: settings.knee,
                radius: settings.radius,
                first: if m == 0 { 1.0 } else { 0.0 },
                _pad: [0.0; 2],
            });
            src = t.bloom_views[m].size();
        }
        for m in (1..n).rev() {
            let s = t.bloom_views[m].size();
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

        for m in 0..n {
            let mut pass = frame.pass_to(&TargetPassDesc {
                label: "Bloom.Down",
                color: &[ColorTarget {
                    view: &t.bloom_views[m],
                    load: LoadOp::Clear(ClearColor([0.0; 4])),
                }],
                depth: None,
            });
            pass.set_pipeline(down);
            pass.set_bind_group(0, &self.bloom_groups[m], &[(m * stride) as u32]);
            pass.draw(0..3, 0..1);
        }
        for (k, m) in (1..n).rev().enumerate() {
            let mut pass = frame.pass_to(&TargetPassDesc {
                label: "Bloom.Up",
                color: &[ColorTarget {
                    view: &t.bloom_views[m - 1],
                    load: LoadOp::Load,
                }],
                depth: None,
            });
            pass.set_pipeline(up);
            pass.set_bind_group(0, &self.bloom_groups[n + k], &[((n + k) * stride) as u32]);
            pass.draw(0..3, 0..1);
        }
    }

    /// Tonemap the scene (+ bloom) into `output`.
    pub(crate) fn tonemap(&self, frame: &mut Frame, output: &TextureView) {
        let (Some(pipeline), Some(group)) = (&self.tonemap, &self.tonemap_group) else {
            return;
        };
        let mut pass = frame.pass_to(&TargetPassDesc {
            label: "Tonemap",
            color: &[ColorTarget {
                view: output,
                load: LoadOp::Clear(ClearColor([0.0, 0.0, 0.0, 1.0])),
            }],
            depth: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, group, &[]);
        pass.draw(0..3, 0..1);
    }
}
