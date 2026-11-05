//! UI pass (plan 0004, decision 6): draws already-tessellated 2D triangles with clip rects and
//! textures on top of the scene. The data is a plain [`UiDrawList`] that travels in the
//! `RenderSnapshot` (law 2), so the renderer never sees the UI library that produced it — the
//! editor fills it from egui today, and the game UI (M9) can fill it from our own widgets.
//!
//! Colors follow egui's convention: **premultiplied alpha, sRGB-encoded** bytes, both in vertex
//! colors and texture pixels. The pass decodes them to linear for the sRGB backbuffer.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use ge_rhi::{
    AddressMode, BindGroup, BindGroupEntry, BindGroupLayout, BindGroupLayoutEntry, BindingResource,
    BindingType, BlendMode, Buffer, BufferDesc, BufferUsage, CullMode, FilterMode, RenderPass,
    RenderPipeline, RenderPipelineDesc, Rhi, Sampler, SamplerDesc, ShaderStages, Texture,
    TextureDesc, TextureFormat, TextureUsage, VertexAttribute, VertexFormat, VertexLayout,
};
use ge_shader::ShaderLibrary;
use tracing::{error, info_span, warn};

/// Identifies a UI texture (egui's `TextureId`, flattened).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct UiTextureId(pub u64);

/// One UI vertex in **physical pixels** from the target's top-left.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct UiVertex {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    /// Premultiplied sRGBA.
    pub color: [u8; 4],
}

const UI_ATTRIBUTES: [VertexAttribute; 3] = [
    VertexAttribute {
        location: 0,
        offset: 0,
        format: VertexFormat::Float32x2,
    },
    VertexAttribute {
        location: 1,
        offset: 8,
        format: VertexFormat::Float32x2,
    },
    VertexAttribute {
        location: 2,
        offset: 16,
        format: VertexFormat::Unorm8x4,
    },
];

impl UiVertex {
    pub const LAYOUT: VertexLayout<'static> = VertexLayout {
        stride: size_of::<UiVertex>() as u64,
        attributes: &UI_ATTRIBUTES,
    };
}

/// Triangles sharing one texture and clip rect.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiMesh {
    /// `[x, y, width, height]` in physical pixels; clamped to the target when drawn.
    pub clip: [u32; 4],
    pub texture: Option<UiTextureId>,
    pub vertices: Vec<UiVertex>,
    pub indices: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum UiTextureUpdate {
    /// Create or replace a texture (`origin = None`), or patch a sub-rectangle of it.
    Set {
        id: UiTextureId,
        origin: Option<[u32; 2]>,
        size: [u32; 2],
        /// Premultiplied sRGBA8, tightly packed rows.
        pixels: Vec<u8>,
        linear_filter: bool,
    },
    Free(UiTextureId),
}

/// Everything the UI pass draws this frame.
///
/// `textures` is a **queue**: producers append and the renderer drains it when it actually
/// renders, so an update is never lost on a skipped (minimized) frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiDrawList {
    pub meshes: Vec<UiMesh>,
    pub textures: Vec<UiTextureUpdate>,
}

impl UiDrawList {
    pub fn is_empty(&self) -> bool {
        self.meshes.is_empty()
    }
}

/// Matches `Screen` in `shaders/ui.wesl`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct ScreenUniform {
    size: [f32; 2],
    srgb_target: f32,
    _pad: f32,
}

struct UiTexture {
    texture: Texture,
    group: BindGroup,
    linear: bool,
}

/// GPU side of the UI pass.
pub struct UiRenderer {
    pipeline: Option<RenderPipeline>,
    color_format: TextureFormat,
    screen_layout: BindGroupLayout,
    texture_layout: BindGroupLayout,
    screen_buffer: Buffer,
    screen_group: BindGroup,
    linear_sampler: Sampler,
    nearest_sampler: Sampler,
    textures: HashMap<UiTextureId, UiTexture>,
    /// 1×1 white, for meshes without a texture.
    white: BindGroup,
    vertex_buffer: Option<Buffer>,
    index_buffer: Option<Buffer>,
    vertex_staging: Vec<UiVertex>,
    index_staging: Vec<u32>,
}

impl std::fmt::Debug for UiRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UiRenderer")
            .field("textures", &self.textures.len())
            .finish_non_exhaustive()
    }
}

impl UiRenderer {
    pub fn new(rhi: &Rhi, color_format: TextureFormat, shaders: &ShaderLibrary) -> Self {
        let screen_layout = rhi.create_bind_group_layout(
            "UiScreen",
            &[BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::VertexFragment,
                ty: BindingType::UniformBuffer {
                    has_dynamic_offset: false,
                    min_binding_size: size_of::<ScreenUniform>() as u64,
                },
            }],
        );
        let texture_layout = rhi.create_bind_group_layout(
            "UiTexture",
            &[
                BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::Fragment,
                    ty: BindingType::Texture2d,
                },
                BindGroupLayoutEntry {
                    binding: 1,
                    visibility: ShaderStages::Fragment,
                    ty: BindingType::Sampler,
                },
            ],
        );
        let screen_buffer = rhi.create_buffer(&BufferDesc {
            label: "UiScreen",
            size: size_of::<ScreenUniform>() as u64,
            usage: BufferUsage::UNIFORM | BufferUsage::COPY_DST,
        });
        let screen_group = rhi.create_bind_group(
            "UiScreen",
            &screen_layout,
            &[BindGroupEntry {
                binding: 0,
                resource: BindingResource::Buffer {
                    buffer: &screen_buffer,
                    offset: 0,
                    size: None,
                },
            }],
        );
        let sampler = |label, filter| {
            rhi.create_sampler(&SamplerDesc {
                label,
                filter,
                address: AddressMode::ClampToEdge,
                mipmaps: false,
                anisotropy: 1,
                compare: None,
            })
        };
        let linear_sampler = sampler("UiLinear", FilterMode::Linear);
        let nearest_sampler = sampler("UiNearest", FilterMode::Nearest);
        let white_tex = rhi.create_texture(&TextureDesc {
            label: "UiWhite",
            size: (1, 1),
            format: TextureFormat::Rgba8UnormSrgb,
            usage: TextureUsage::TEXTURE_BINDING | TextureUsage::COPY_DST,
        });
        rhi.write_texture(&white_tex, (0, 0), (1, 1), &[255; 4]);
        let white = Self::make_group(rhi, &texture_layout, &white_tex, &nearest_sampler);

        let mut ui = Self {
            pipeline: None,
            color_format,
            screen_layout,
            texture_layout,
            screen_buffer,
            screen_group,
            linear_sampler,
            nearest_sampler,
            textures: HashMap::new(),
            white,
            vertex_buffer: None,
            index_buffer: None,
            vertex_staging: Vec::new(),
            index_staging: Vec::new(),
        };
        ui.rebuild_pipeline(rhi, shaders);
        ui
    }

    fn make_group(
        rhi: &Rhi,
        layout: &BindGroupLayout,
        texture: &Texture,
        sampler: &Sampler,
    ) -> BindGroup {
        rhi.create_bind_group(
            "UiTexture",
            layout,
            &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::Texture(texture),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Sampler(sampler),
                },
            ],
        )
    }

    pub fn has_pipeline(&self) -> bool {
        self.pipeline.is_some()
    }

    /// Recompiles `ui.wesl` (hot reload). Keeps the old pipeline on failure.
    pub fn rebuild_pipeline(&mut self, rhi: &Rhi, shaders: &ShaderLibrary) {
        let built = shaders
            .compile("ui", &[])
            .map_err(|e| e.to_string())
            .and_then(|c| {
                rhi.create_shader_module("ui", &c.wgsl)
                    .map_err(|e| e.to_string())
            })
            .and_then(|module| {
                rhi.create_render_pipeline(&RenderPipelineDesc {
                    label: "Ui",
                    shader: &module,
                    vertex_entry: "vs_main",
                    fragment_entry: Some("fs_main"),
                    vertex_layouts: &[UiVertex::LAYOUT],
                    bind_group_layouts: &[&self.screen_layout, &self.texture_layout],
                    color_formats: &[self.color_format],
                    depth: None,
                    cull_mode: CullMode::None,
                    blend: Some(BlendMode::PremultipliedAlpha),
                    depth_bias: None,
                })
                .map_err(|e| e.to_string())
            });
        match built {
            Ok(p) => self.pipeline = Some(p),
            Err(e) => error!("ui shader rejected:\n{e}"),
        }
    }

    pub(crate) fn apply_textures(&mut self, rhi: &Rhi, updates: &mut Vec<UiTextureUpdate>) {
        for update in updates.drain(..) {
            match update {
                UiTextureUpdate::Set {
                    id,
                    origin,
                    size,
                    pixels,
                    linear_filter,
                } => {
                    let size = (size[0], size[1]);
                    if pixels.len() != (size.0 * size.1 * 4) as usize {
                        warn!(?id, ?size, "ui texture update has the wrong byte count");
                        continue;
                    }
                    match origin {
                        Some([x, y]) => {
                            let Some(t) = self.textures.get(&id) else {
                                warn!(?id, "patch for unknown ui texture");
                                continue;
                            };
                            let (w, h) = t.texture.size();
                            if x + size.0 > w || y + size.1 > h {
                                warn!(?id, "ui texture patch out of bounds");
                                continue;
                            }
                            rhi.write_texture(&t.texture, (x, y), size, &pixels);
                            if t.linear != linear_filter {
                                let sampler = self.sampler(linear_filter).clone();
                                let group = Self::make_group(
                                    rhi,
                                    &self.texture_layout,
                                    &t.texture,
                                    &sampler,
                                );
                                if let Some(t) = self.textures.get_mut(&id) {
                                    t.group = group;
                                    t.linear = linear_filter;
                                }
                            }
                        }
                        None => {
                            let texture = rhi.create_texture(&TextureDesc {
                                label: "UiTexture",
                                size,
                                format: TextureFormat::Rgba8UnormSrgb,
                                usage: TextureUsage::TEXTURE_BINDING | TextureUsage::COPY_DST,
                            });
                            rhi.write_texture(&texture, (0, 0), size, &pixels);
                            let group = Self::make_group(
                                rhi,
                                &self.texture_layout,
                                &texture,
                                self.sampler(linear_filter),
                            );
                            self.textures.insert(
                                id,
                                UiTexture {
                                    texture,
                                    group,
                                    linear: linear_filter,
                                },
                            );
                        }
                    }
                }
                UiTextureUpdate::Free(id) => {
                    self.textures.remove(&id);
                }
            }
        }
    }

    fn sampler(&self, linear: bool) -> &Sampler {
        if linear {
            &self.linear_sampler
        } else {
            &self.nearest_sampler
        }
    }

    /// Grows a dynamic buffer to fit `bytes` (doubling) and uploads.
    fn upload(rhi: &Rhi, slot: &mut Option<Buffer>, label: &str, usage: BufferUsage, bytes: &[u8]) {
        let needed = (bytes.len() as u64).next_multiple_of(4).max(4);
        if slot.as_ref().is_none_or(|b| b.size() < needed) {
            let size = needed.next_power_of_two().max(64 * 1024);
            *slot = Some(rhi.create_buffer(&BufferDesc {
                label,
                size,
                usage: usage | BufferUsage::COPY_DST,
            }));
        }
        if let Some(buffer) = slot {
            // write_buffer needs a multiple of 4 bytes.
            if bytes.len().is_multiple_of(4) {
                rhi.write_buffer(buffer, 0, bytes);
            } else {
                let mut padded = bytes.to_vec();
                padded.resize(bytes.len().next_multiple_of(4), 0);
                rhi.write_buffer(buffer, 0, &padded);
            }
        }
    }

    /// Applies pending texture updates (draining `list.textures`) and uploads `list.meshes`
    /// for a `size` target. Returns the draws for [`UiRenderer::record`], or `None` when
    /// there's nothing to draw (then the graph adds no Ui pass).
    pub(crate) fn prepare(
        &mut self,
        rhi: &Rhi,
        (width, height): (u32, u32),
        list: &mut UiDrawList,
    ) -> Option<UiDraws> {
        let _span = info_span!("Render::UiPrepare", meshes = list.meshes.len()).entered();
        self.apply_textures(rhi, &mut list.textures);
        if list.meshes.is_empty() || self.pipeline.is_none() || width == 0 || height == 0 {
            return None;
        }

        self.vertex_staging.clear();
        self.index_staging.clear();
        let mut draws = Vec::with_capacity(list.meshes.len());
        for mesh in &list.meshes {
            if mesh.indices.is_empty() {
                continue;
            }
            let [x, y, w, h] = mesh.clip;
            let x0 = x.min(width);
            let y0 = y.min(height);
            let x1 = x.saturating_add(w).min(width);
            let y1 = y.saturating_add(h).min(height);
            if x1 <= x0 || y1 <= y0 {
                continue;
            }
            let base_vertex = self.vertex_staging.len() as i32;
            let first_index = self.index_staging.len() as u32;
            self.vertex_staging.extend_from_slice(&mesh.vertices);
            self.index_staging.extend_from_slice(&mesh.indices);
            draws.push(UiDraw {
                scissor: [x0, y0, x1 - x0, y1 - y0],
                texture: mesh.texture,
                indices: first_index..first_index + mesh.indices.len() as u32,
                base_vertex,
            });
        }
        if draws.is_empty() {
            return None;
        }

        rhi.write_buffer(
            &self.screen_buffer,
            0,
            bytemuck::bytes_of(&ScreenUniform {
                size: [width as f32, height as f32],
                srgb_target: if self.color_format.is_srgb() {
                    1.0
                } else {
                    0.0
                },
                _pad: 0.0,
            }),
        );
        Self::upload(
            rhi,
            &mut self.vertex_buffer,
            "UiVertices",
            BufferUsage::VERTEX,
            bytemuck::cast_slice(&self.vertex_staging),
        );
        Self::upload(
            rhi,
            &mut self.index_buffer,
            "UiIndices",
            BufferUsage::INDEX,
            bytemuck::cast_slice(&self.index_staging),
        );
        Some(UiDraws(draws))
    }

    /// Draws what [`UiRenderer::prepare`] uploaded, over whatever the target already holds.
    pub(crate) fn record(&self, pass: &mut RenderPass<'_>, draws: &UiDraws) {
        let (Some(pipeline), Some(vb), Some(ib)) =
            (&self.pipeline, &self.vertex_buffer, &self.index_buffer)
        else {
            return;
        };
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &self.screen_group, &[]);
        pass.set_vertex_buffer(0, vb);
        pass.set_index_buffer(ib);
        for draw in &draws.0 {
            let [x, y, w, h] = draw.scissor;
            let group = match draw.texture {
                Some(id) => match self.textures.get(&id) {
                    Some(t) => &t.group,
                    None => continue,
                },
                None => &self.white,
            };
            pass.set_scissor_rect(x, y, w, h);
            pass.set_bind_group(1, group, &[]);
            pass.draw_indexed(draw.indices.clone(), draw.base_vertex, 0..1);
        }
    }
}

#[derive(Debug)]
struct UiDraw {
    scissor: [u32; 4],
    texture: Option<UiTextureId>,
    indices: std::ops::Range<u32>,
    base_vertex: i32,
}

/// One frame's uploaded UI draws.
#[derive(Debug)]
pub(crate) struct UiDraws(Vec<UiDraw>);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertex_is_20_bytes() {
        assert_eq!(size_of::<UiVertex>(), 20);
        assert_eq!(UiVertex::LAYOUT.stride, 20);
    }
}
