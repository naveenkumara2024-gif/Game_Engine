//! GPU copies of assets (plan 0005, decision 9). Extract hands over `Arc`s of new or changed
//! meshes, textures and materials **once** ([`AssetUpload`]); the renderer keeps GPU versions
//! keyed by asset id. Draw items carry only ids.

use std::collections::HashMap;
use std::sync::Arc;

use ge_asset::{AssetId, MaterialAsset, Mesh, PixelFormat, Texture, UntypedId};
use ge_rhi::{
    AddressMode, BindGroup, BindGroupEntry, BindGroupLayout, BindGroupLayoutEntry, BindingResource,
    BindingType, FilterMode, Rhi, Sampler, SamplerDesc, ShaderStages, Texture as GpuTexture,
    TextureDesc, TextureFormat, TextureUsage,
};
use tracing::{debug, warn};

use crate::mesh::{GpuMesh, MeshData, Vertex};

/// One change to apply on the GPU side.
#[derive(Clone, Debug)]
pub enum AssetUpload {
    Mesh(AssetId<Mesh>, Arc<Mesh>),
    Texture(AssetId<Texture>, Arc<Texture>),
    Material(AssetId<MaterialAsset>, Arc<MaterialAsset>),
    Free(UntypedId),
}

fn gpu_format(f: PixelFormat) -> TextureFormat {
    match f {
        PixelFormat::Rgba8 => TextureFormat::Rgba8Unorm,
        PixelFormat::Rgba8Srgb => TextureFormat::Rgba8UnormSrgb,
        PixelFormat::Bc1 => TextureFormat::Bc1RgbaUnorm,
        PixelFormat::Bc1Srgb => TextureFormat::Bc1RgbaUnormSrgb,
        PixelFormat::Bc3 => TextureFormat::Bc3RgbaUnorm,
        PixelFormat::Bc3Srgb => TextureFormat::Bc3RgbaUnormSrgb,
        PixelFormat::Bc4 => TextureFormat::Bc4RUnorm,
        PixelFormat::Bc5 => TextureFormat::Bc5RgUnorm,
        PixelFormat::Bc7 => TextureFormat::Bc7RgbaUnorm,
        PixelFormat::Bc7Srgb => TextureFormat::Bc7RgbaUnormSrgb,
        PixelFormat::Rgba16Float => TextureFormat::Rgba16Float,
    }
}

pub(crate) struct GpuAssets {
    pub(crate) layout: BindGroupLayout,
    sampler: Sampler,
    white_srgb: GpuTexture,
    white_linear: GpuTexture,
    /// (0.5, 0.5, 1) — "no normal map".
    flat_normal: GpuTexture,
    default_group: BindGroup,
    meshes: HashMap<AssetId<Mesh>, GpuMesh>,
    textures: HashMap<AssetId<Texture>, GpuTexture>,
    /// Bumped on every texture upload (environment maps rebuild when it changes).
    texture_versions: HashMap<AssetId<Texture>, u64>,
    upload_counter: u64,
    materials: HashMap<AssetId<MaterialAsset>, (Arc<MaterialAsset>, BindGroup)>,
}

impl std::fmt::Debug for GpuAssets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuAssets")
            .field("meshes", &self.meshes.len())
            .field("textures", &self.textures.len())
            .field("materials", &self.materials.len())
            .finish_non_exhaustive()
    }
}

impl GpuAssets {
    pub(crate) fn new(rhi: &Rhi) -> Self {
        let tex = |binding| BindGroupLayoutEntry {
            binding,
            visibility: ShaderStages::Fragment,
            ty: BindingType::Texture2d,
        };
        let layout = rhi.create_bind_group_layout(
            "Material",
            &[
                tex(0),
                tex(1),
                tex(2),
                BindGroupLayoutEntry {
                    binding: 3,
                    visibility: ShaderStages::Fragment,
                    ty: BindingType::Sampler,
                },
                tex(4),
                tex(5),
            ],
        );
        let sampler = rhi.create_sampler(&SamplerDesc {
            label: "MaterialSampler",
            filter: FilterMode::Linear,
            address: AddressMode::Repeat,
            mipmaps: true,
            anisotropy: 8,
            compare: None,
        });
        let solid = |label, format, px: [u8; 4]| {
            let t = rhi.create_texture(&TextureDesc {
                label,
                size: (1, 1),
                format,
                usage: TextureUsage::TEXTURE_BINDING | TextureUsage::COPY_DST,
            });
            rhi.write_texture(&t, (0, 0), (1, 1), &px);
            t
        };
        let white_srgb = solid("WhiteSrgb", TextureFormat::Rgba8UnormSrgb, [255; 4]);
        let white_linear = solid("WhiteLinear", TextureFormat::Rgba8Unorm, [255; 4]);
        let flat_normal = solid(
            "FlatNormal",
            TextureFormat::Rgba8Unorm,
            [128, 128, 255, 255],
        );
        let default_group = Self::group(
            rhi,
            &layout,
            [
                &white_srgb,
                &white_linear,
                &white_srgb,
                &flat_normal,
                &white_linear,
            ],
            &sampler,
        );
        Self {
            layout,
            sampler,
            white_srgb,
            white_linear,
            flat_normal,
            default_group,
            meshes: HashMap::new(),
            textures: HashMap::new(),
            texture_versions: HashMap::new(),
            upload_counter: 0,
            materials: HashMap::new(),
        }
    }

    fn group(
        rhi: &Rhi,
        layout: &BindGroupLayout,
        textures: [&GpuTexture; 5],
        sampler: &Sampler,
    ) -> BindGroup {
        rhi.create_bind_group(
            "Material",
            layout,
            &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::Texture(textures[0]),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Texture(textures[1]),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: BindingResource::Texture(textures[2]),
                },
                BindGroupEntry {
                    binding: 3,
                    resource: BindingResource::Sampler(sampler),
                },
                BindGroupEntry {
                    binding: 4,
                    resource: BindingResource::Texture(textures[3]),
                },
                BindGroupEntry {
                    binding: 5,
                    resource: BindingResource::Texture(textures[4]),
                },
            ],
        )
    }

    /// Applies (and drains) pending uploads. Textures first, so materials can bind them.
    pub(crate) fn apply(&mut self, rhi: &Rhi, uploads: &mut Vec<AssetUpload>) {
        if uploads.is_empty() {
            return;
        }
        let _span = tracing::info_span!("Render::AssetUploads", count = uploads.len()).entered();
        let mut textures_changed = false;
        let mut materials = Vec::new();
        for upload in uploads.drain(..) {
            match upload {
                AssetUpload::Mesh(id, mesh) => {
                    let data = MeshData {
                        vertices: (0..mesh.vertex_count())
                            .map(|i| Vertex {
                                position: mesh.positions[i],
                                normal: mesh.normals[i],
                                color: [1.0; 3],
                                uv: mesh.uvs[i],
                                tangent: mesh.tangents[i],
                            })
                            .collect(),
                        indices: mesh.indices.clone(),
                    };
                    if data.is_valid() && !data.indices.is_empty() {
                        self.meshes
                            .insert(id, GpuMesh::upload(rhi, "AssetMesh", &data));
                    } else {
                        warn!(?id, "invalid mesh asset not uploaded");
                    }
                }
                AssetUpload::Texture(id, texture) => {
                    if let Some(t) = self.upload_texture(rhi, &texture) {
                        self.textures.insert(id, t);
                        self.upload_counter += 1;
                        self.texture_versions.insert(id, self.upload_counter);
                        textures_changed = true;
                    }
                }
                AssetUpload::Material(id, material) => materials.push((id, material)),
                AssetUpload::Free(id) => {
                    if let Some(m) = id.typed::<Mesh>() {
                        self.meshes.remove(&m);
                    } else if let Some(t) = id.typed::<Texture>() {
                        self.textures.remove(&t);
                        textures_changed = true;
                    } else if let Some(m) = id.typed::<MaterialAsset>() {
                        self.materials.remove(&m);
                    }
                }
            }
        }
        if textures_changed {
            // Rebuild every material so bind groups point at the new texture objects.
            let existing: Vec<_> = self
                .materials
                .iter()
                .map(|(id, (m, _))| (*id, m.clone()))
                .collect();
            materials.extend(existing);
        }
        for (id, material) in materials {
            let pick = |r: &Option<ge_asset::SubRef<Texture>>, fallback: &GpuTexture| {
                r.as_ref()
                    .and_then(|r| r.id)
                    .and_then(|id| self.textures.get(&id))
                    .unwrap_or(fallback)
                    .clone()
            };
            let base = pick(&material.base_color_texture, &self.white_srgb);
            let mr = pick(&material.metallic_roughness_texture, &self.white_linear);
            let emissive = pick(&material.emissive_texture, &self.white_srgb);
            let normal = pick(&material.normal_texture, &self.flat_normal);
            let occlusion = pick(&material.occlusion_texture, &self.white_linear);
            let group = Self::group(
                rhi,
                &self.layout,
                [&base, &mr, &emissive, &normal, &occlusion],
                &self.sampler,
            );
            self.materials.insert(id, (material, group));
        }
        debug!(
            meshes = self.meshes.len(),
            textures = self.textures.len(),
            materials = self.materials.len(),
            "asset uploads applied"
        );
    }

    fn upload_texture(&self, rhi: &Rhi, texture: &Texture) -> Option<GpuTexture> {
        if texture.format.is_compressed() && !rhi.supports_bc() {
            warn!(format = ?texture.format, "GPU lacks BC texture support; texture replaced by white");
            return None;
        }
        if !texture.is_valid() {
            warn!("invalid texture asset not uploaded");
            return None;
        }
        let gpu = rhi.create_texture_mips(
            &TextureDesc {
                label: "AssetTexture",
                size: (texture.width, texture.height),
                format: gpu_format(texture.format),
                usage: TextureUsage::TEXTURE_BINDING | TextureUsage::COPY_DST,
            },
            texture.mips.len() as u32,
        );
        for (level, data) in texture.mips.iter().enumerate() {
            if let Err(e) = rhi.write_texture_mip(&gpu, level as u32, data) {
                warn!(error = %e, "texture mip upload failed");
                return None;
            }
        }
        Some(gpu)
    }

    pub(crate) fn texture(&self, id: AssetId<Texture>) -> Option<&GpuTexture> {
        self.textures.get(&id)
    }

    pub(crate) fn texture_version(&self, id: AssetId<Texture>) -> u64 {
        self.texture_versions.get(&id).copied().unwrap_or(0)
    }

    pub(crate) fn mesh(&self, id: AssetId<Mesh>) -> Option<&GpuMesh> {
        self.meshes.get(&id)
    }

    pub(crate) fn material_group(&self, id: Option<AssetId<MaterialAsset>>) -> &BindGroup {
        id.and_then(|id| self.materials.get(&id))
            .map_or(&self.default_group, |(_, g)| g)
    }

    pub(crate) fn counts(&self) -> (usize, usize, usize) {
        (self.meshes.len(), self.textures.len(), self.materials.len())
    }
}
