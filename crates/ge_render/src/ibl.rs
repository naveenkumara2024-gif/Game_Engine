//! Image-based lighting (plan 0006, decision 4). Builds, on the GPU and only when the world
//! changes, a source cube (from the world color, our procedural sky or an HDRI), its mip
//! chain, a GGX-prefiltered specular cube and SH9 diffuse coefficients; plus the split-sum
//! BRDF lookup once at startup.

use bytemuck::{Pod, Zeroable};
use ge_rhi::{
    AddressMode, BindGroupEntry, BindGroupLayout, BindGroupLayoutEntry, BindingResource,
    BindingType, Buffer, BufferDesc, BufferUsage, ComputePipeline, ComputePipelineDesc, FilterMode,
    Frame, Rhi, SampleKind, Sampler, SamplerDesc, ShaderStages, Texture, TextureDesc,
    TextureDescEx, TextureDim, TextureFormat, TextureUsage, TextureView, ViewRange,
};
use ge_shader::ShaderLibrary;
use glam::Vec3;
use tracing::{error, info};

use crate::lighting::WorldSurface;

const BRDF_LUT_SIZE: u32 = 64;
/// Source mip sampled for SH9 projection (small: SH only needs low frequencies).
const SH_SOURCE_SIZE: u32 = 32;
const SPECULAR_MAX: u32 = 512;
const SPECULAR_MIPS: u32 = 6;

/// Matches `EnvParams` in `shaders/env.wesl`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, Pod, Zeroable)]
struct EnvParams {
    a: [f32; 4],
    b: [f32; 4],
    c: [f32; 4],
    _pad: [f32; 4],
}

/// What the current environment was built from; a change rebuilds it.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct EnvKey {
    pub surface: WorldSurface,
    pub color: [f32; 3],
    pub strength: f32,
    pub sun: [f32; 3],
    pub hdri: Option<(ge_asset::AssetId<ge_asset::Texture>, u64)>,
    pub rotation: f32,
    pub resolution: u32,
}

pub(crate) struct Environment {
    pub specular: Texture,
    pub sampler: Sampler,
    pub brdf_lut: Texture,
    pub sh: Buffer,
    pub mip_count: u32,
    pub ready: bool,
    source: Option<Texture>,
    key: Option<EnvKey>,
    layout: BindGroupLayout,
    sh_layout: BindGroupLayout,
    params: Buffer,
    params_stride: u64,
    pipelines: Option<Pipelines>,
    dummy_2d: Texture,
    dummy_cube: Texture,
    dummy_storage: Texture,
    /// Bound at the LUT slot by every dispatch except the LUT one (a texture can't be sampled
    /// and written in the same dispatch).
    dummy_lut: Texture,
    lut_done: bool,
}

struct Pipelines {
    make_source: ComputePipeline,
    downsample: ComputePipeline,
    prefilter: ComputePipeline,
    project_sh: ComputePipeline,
    brdf_lut: ComputePipeline,
}

impl std::fmt::Debug for Environment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Environment")
            .field("key", &self.key)
            .field("ready", &self.ready)
            .finish_non_exhaustive()
    }
}

fn storage_usage() -> TextureUsage {
    TextureUsage::TEXTURE_BINDING | TextureUsage::STORAGE_BINDING
}

fn cube(rhi: &Rhi, label: &str, size: u32, mips: u32) -> Texture {
    rhi.create_texture_ex(&TextureDescEx {
        label,
        size: (size, size),
        format: TextureFormat::Rgba16Float,
        usage: storage_usage(),
        mip_levels: mips,
        layers: 6,
        dim: TextureDim::Cube,
    })
}

impl Environment {
    pub(crate) fn new(rhi: &Rhi) -> Self {
        let c = ShaderStages::Compute;
        let e = |binding, ty| BindGroupLayoutEntry {
            binding,
            visibility: c,
            ty,
        };
        let layout = rhi.create_bind_group_layout(
            "Env",
            &[
                e(
                    0,
                    BindingType::UniformBuffer {
                        has_dynamic_offset: true,
                        min_binding_size: size_of::<EnvParams>() as u64,
                    },
                ),
                e(1, BindingType::Texture2d),
                e(
                    2,
                    BindingType::Texture {
                        sample: SampleKind::Float { filterable: true },
                        dim: TextureDim::Cube,
                    },
                ),
                e(3, BindingType::Sampler),
                e(
                    4,
                    BindingType::StorageTexture {
                        format: TextureFormat::Rgba16Float,
                        dim: TextureDim::D2Array,
                    },
                ),
                e(
                    5,
                    BindingType::StorageTexture {
                        format: TextureFormat::Rgba16Float,
                        dim: TextureDim::D2,
                    },
                ),
            ],
        );
        let sh_layout = rhi.create_bind_group_layout(
            "EnvSh",
            &[e(0, BindingType::StorageBuffer { read_only: false })],
        );
        let params_stride =
            (size_of::<EnvParams>() as u64).next_multiple_of(rhi.uniform_offset_alignment());
        let params = rhi.create_buffer(&BufferDesc {
            label: "EnvParams",
            size: params_stride * 64,
            usage: BufferUsage::UNIFORM | BufferUsage::COPY_DST,
        });
        let sh = rhi.create_buffer(&BufferDesc {
            label: "EnvSh",
            size: 9 * 16,
            usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
        });
        // A flat mid-grey default so the first frame isn't black before the env is built.
        let mut grey = [0.0f32; 36];
        grey[0] = 0.0;
        rhi.write_buffer(&sh, 0, bytemuck::cast_slice(&grey));
        let sampler = rhi.create_sampler(&SamplerDesc {
            label: "Env",
            filter: FilterMode::Linear,
            address: AddressMode::ClampToEdge,
            mipmaps: true,
            anisotropy: 1,
            compare: None,
        });
        let brdf_lut = rhi.create_texture(&TextureDesc {
            label: "BrdfLut",
            size: (BRDF_LUT_SIZE, BRDF_LUT_SIZE),
            format: TextureFormat::Rgba16Float,
            usage: storage_usage(),
        });
        let dummy_2d = rhi.create_texture(&TextureDesc {
            label: "EnvDummy2d",
            size: (1, 1),
            format: TextureFormat::Rgba16Float,
            usage: storage_usage(),
        });
        let dummy_lut = rhi.create_texture(&TextureDesc {
            label: "EnvDummyLut",
            size: (1, 1),
            format: TextureFormat::Rgba16Float,
            usage: storage_usage(),
        });
        Self {
            specular: cube(rhi, "EnvSpecular", 16, 1),
            sampler,
            brdf_lut,
            sh,
            mip_count: 1,
            ready: false,
            source: None,
            key: None,
            layout,
            sh_layout,
            params,
            params_stride,
            pipelines: None,
            dummy_2d,
            dummy_cube: cube(rhi, "EnvDummyCube", 1, 1),
            dummy_storage: cube(rhi, "EnvDummyStorage", 1, 1),
            dummy_lut,
            lut_done: false,
        }
    }

    pub(crate) fn rebuild_pipelines(&mut self, rhi: &Rhi, shaders: &ShaderLibrary) -> bool {
        let built = shaders
            .compile("env", &[])
            .map_err(|e| e.to_string())
            .and_then(|c| {
                rhi.create_shader_module("env", &c.wgsl)
                    .map_err(|e| e.to_string())
            })
            .and_then(|m| {
                let make = |entry: &str, layouts: &[&BindGroupLayout]| {
                    rhi.create_compute_pipeline(&ComputePipelineDesc {
                        label: "Env",
                        shader: &m,
                        entry,
                        bind_group_layouts: layouts,
                    })
                    .map_err(|e| e.to_string())
                };
                Ok(Pipelines {
                    make_source: make("make_source", &[&self.layout])?,
                    downsample: make("downsample", &[&self.layout])?,
                    prefilter: make("prefilter", &[&self.layout])?,
                    project_sh: make("project_sh", &[&self.layout, &self.sh_layout])?,
                    brdf_lut: make("brdf_lut", &[&self.layout])?,
                })
            });
        match built {
            Ok(p) => {
                self.pipelines = Some(p);
                self.key = None; // rebuild with the new shader
                self.lut_done = false;
                true
            }
            Err(e) => {
                error!("environment shader rejected:\n{e}");
                false
            }
        }
    }

    pub(crate) fn has_pipelines(&self) -> bool {
        self.pipelines.is_some()
    }

    /// Rebuilds the environment if `key` changed. Returns true when the specular cube
    /// texture object was replaced (group 0 must be rebuilt).
    pub(crate) fn update(
        &mut self,
        rhi: &Rhi,
        frame: &mut Frame,
        key: EnvKey,
        hdri: Option<&Texture>,
    ) -> bool {
        let Some(p) = &self.pipelines else {
            return false;
        };
        let mut replaced = false;
        let mut params: Vec<EnvParams> = Vec::new();
        let mut push = |p: EnvParams| -> u32 {
            params.push(p);
            ((params.len() - 1) as u64 * self.params_stride) as u32
        };

        let lut_offset = (!self.lut_done).then(|| {
            push(EnvParams {
                a: [BRDF_LUT_SIZE as f32, 0.0, 0.0, 0.0],
                ..Default::default()
            })
        });
        let rebuild = self.key != Some(key);
        if !rebuild && lut_offset.is_none() {
            return false;
        }

        let size = key.resolution.clamp(16, 2048);
        let src_mips = size.ilog2() + 1;
        let spec_size = size.min(SPECULAR_MAX);
        let spec_mips = SPECULAR_MIPS.min(spec_size.ilog2() + 1);
        if rebuild {
            if self.source.as_ref().is_none_or(|t| t.size().0 != size) {
                self.source = Some(cube(rhi, "EnvSource", size, src_mips));
            }
            if self.specular.size().0 != spec_size || self.specular.mip_levels() != spec_mips {
                self.specular = cube(rhi, "EnvSpecular", spec_size, spec_mips);
                replaced = true;
            }
        }
        let source = self.source.clone();

        // Parameters for every dispatch, uploaded together.
        let (mode, b) = match (key.surface, hdri) {
            (WorldSurface::Hdri, Some(_)) => (2.0, [0.0, 0.0, 0.0, key.strength]),
            (WorldSurface::Sky, _) => (1.0, [key.sun[0], key.sun[1], key.sun[2], key.strength]),
            _ => (
                0.0,
                [
                    key.color[0] * key.strength,
                    key.color[1] * key.strength,
                    key.color[2] * key.strength,
                    1.0,
                ],
            ),
        };
        let tint = if key.surface == WorldSurface::Sky {
            // The world color tints the sky (white = neutral).
            let c = Vec3::from(key.color);
            (c / c.max_element().max(1e-4)).to_array()
        } else {
            [1.0; 3]
        };
        let source_offset = rebuild.then(|| {
            push(EnvParams {
                a: [size as f32, mode, key.rotation, 0.0],
                b,
                c: [tint[0], tint[1], tint[2], 0.0],
                ..Default::default()
            })
        });
        let down_offsets: Vec<u32> = if rebuild {
            (1..src_mips)
                .map(|m| {
                    push(EnvParams {
                        a: [(size >> m).max(1) as f32, 0.0, 0.0, 0.0],
                        ..Default::default()
                    })
                })
                .collect()
        } else {
            Vec::new()
        };
        let prefilter_offsets: Vec<u32> = if rebuild {
            (0..spec_mips)
                .map(|m| {
                    let roughness = m as f32 / (spec_mips - 1).max(1) as f32;
                    push(EnvParams {
                        a: [(spec_size >> m).max(1) as f32, 0.0, 0.0, roughness],
                        b: [size as f32, src_mips as f32, 0.0, 0.0],
                        ..Default::default()
                    })
                })
                .collect()
        } else {
            Vec::new()
        };
        let sh_mip = src_mips
            .saturating_sub(SH_SOURCE_SIZE.ilog2() + 1)
            .min(src_mips - 1);
        let sh_offset = rebuild.then(|| {
            push(EnvParams {
                a: [(size >> sh_mip).max(1) as f32, 0.0, 0.0, sh_mip as f32],
                ..Default::default()
            })
        });
        let mut bytes = vec![0u8; params.len() * self.params_stride as usize];
        for (i, p) in params.iter().enumerate() {
            let o = i * self.params_stride as usize;
            bytes[o..o + size_of::<EnvParams>()].copy_from_slice(bytemuck::bytes_of(p));
        }
        rhi.write_buffer(&self.params, 0, &bytes);

        let group =
            |src_2d: &Texture, src_cube: &TextureView, target: &TextureView, lut: &Texture| {
                rhi.create_bind_group(
                    "Env",
                    &self.layout,
                    &[
                        BindGroupEntry {
                            binding: 0,
                            resource: BindingResource::Buffer {
                                buffer: &self.params,
                                offset: 0,
                                size: Some(size_of::<EnvParams>() as u64),
                            },
                        },
                        BindGroupEntry {
                            binding: 1,
                            resource: BindingResource::Texture(src_2d),
                        },
                        BindGroupEntry {
                            binding: 2,
                            resource: BindingResource::View(src_cube),
                        },
                        BindGroupEntry {
                            binding: 3,
                            resource: BindingResource::Sampler(&self.sampler),
                        },
                        BindGroupEntry {
                            binding: 4,
                            resource: BindingResource::View(target),
                        },
                        BindGroupEntry {
                            binding: 5,
                            resource: BindingResource::Texture(lut),
                        },
                    ],
                )
            };
        let dummy_cube = self.dummy_cube.full_view();
        let dummy_target = self.dummy_storage.view(ViewRange {
            dim: TextureDim::D2Array,
            base_mip: 0,
            mips: 1,
            base_layer: 0,
            layers: 6,
        });
        let array_level = |t: &Texture, mip: u32| {
            t.view(ViewRange {
                dim: TextureDim::D2Array,
                base_mip: mip,
                mips: 1,
                base_layer: 0,
                layers: 6,
            })
        };
        let cube_levels = |t: &Texture, base: u32, count: u32| {
            t.view(ViewRange {
                dim: TextureDim::Cube,
                base_mip: base,
                mips: count,
                base_layer: 0,
                layers: 6,
            })
        };

        let mut pass = frame.compute_pass("Environment");
        if let Some(offset) = lut_offset {
            let g = group(&self.dummy_2d, &dummy_cube, &dummy_target, &self.brdf_lut);
            pass.set_pipeline(&p.brdf_lut);
            pass.set_bind_group(0, &g, &[offset]);
            pass.dispatch(BRDF_LUT_SIZE.div_ceil(8), BRDF_LUT_SIZE.div_ceil(8), 1);
            self.lut_done = true;
        }
        if let (Some(source), Some(offset)) = (&source, source_offset) {
            let equirect = hdri.unwrap_or(&self.dummy_2d);
            let g = group(
                equirect,
                &dummy_cube,
                &array_level(source, 0),
                &self.dummy_lut,
            );
            pass.set_pipeline(&p.make_source);
            pass.set_bind_group(0, &g, &[offset]);
            pass.dispatch(size.div_ceil(8), size.div_ceil(8), 6);
            for (i, &offset) in down_offsets.iter().enumerate() {
                let m = i as u32 + 1;
                let s = (size >> m).max(1);
                let g = group(
                    &self.dummy_2d,
                    &cube_levels(source, m - 1, 1),
                    &array_level(source, m),
                    &self.dummy_lut,
                );
                pass.set_pipeline(&p.downsample);
                pass.set_bind_group(0, &g, &[offset]);
                pass.dispatch(s.div_ceil(8), s.div_ceil(8), 6);
            }
            let full = cube_levels(source, 0, src_mips);
            for (m, &offset) in prefilter_offsets.iter().enumerate() {
                let s = (spec_size >> m).max(1);
                let g = group(
                    &self.dummy_2d,
                    &full,
                    &array_level(&self.specular, m as u32),
                    &self.dummy_lut,
                );
                pass.set_pipeline(&p.prefilter);
                pass.set_bind_group(0, &g, &[offset]);
                pass.dispatch(s.div_ceil(8), s.div_ceil(8), 6);
            }
            if let Some(offset) = sh_offset {
                let g = group(&self.dummy_2d, &full, &dummy_target, &self.dummy_lut);
                let sh_group = rhi.create_bind_group(
                    "EnvSh",
                    &self.sh_layout,
                    &[BindGroupEntry {
                        binding: 0,
                        resource: BindingResource::Buffer {
                            buffer: &self.sh,
                            offset: 0,
                            size: None,
                        },
                    }],
                );
                pass.set_pipeline(&p.project_sh);
                pass.set_bind_group(0, &g, &[offset]);
                pass.set_bind_group(1, &sh_group, &[]);
                pass.dispatch(1, 1, 1);
            }
            self.mip_count = spec_mips;
            self.key = Some(key);
            self.ready = true;
            info!(surface = ?key.surface, size, spec_size, "environment rebuilt");
        }
        drop(pass);
        replaced
    }
}
