//! The M1 renderer: one opaque pass that draws indexed meshes with per-view and per-object
//! uniforms into a reversed-Z depth buffer. It talks to the GPU only through `ge_rhi` (law 12).
//!
//! The draw list is handed in directly for now. M5 replaces it with extracted render proxies,
//! a render graph and cached draw packets (lore §3.7, §6.7).

use bytemuck::{Pod, Zeroable};
use ge_rhi::{
    BindGroup, BindGroupEntry, BindGroupLayout, BindGroupLayoutEntry, BindingResource, BindingType,
    Buffer, BufferDesc, BufferUsage, ClearColor, CompareFunction, CullMode, DepthAttachment,
    DepthState, Frame, LoadOp, RenderPassDesc, RenderPipeline, RenderPipelineDesc, Rhi, RhiError,
    ShaderStages, Texture, TextureDesc, TextureFormat, TextureUsage,
};
use ge_shader::{ShaderError, ShaderLibrary};
use glam::{Mat4, Vec4};
use tracing::{error, info, info_span, warn};

use crate::Camera;
use crate::camera::aspect_ratio;
use crate::mesh::{GpuMesh, MeshData, Vertex};

ge_core::declare_cvar!(pub static R_DEBUG_NORMALS: bool = (
    "r.DebugNormals",
    false,
    "Shade meshes with their world-space normals (switches the mesh shader permutation)"
));

/// Objects per frame. The GPU scene (M5/M12) removes this cap.
pub const MAX_OBJECTS: usize = 1024;

/// Reversed-Z: clear to 0 (infinitely far), keep fragments with greater depth.
const DEPTH_FORMAT: TextureFormat = TextureFormat::Depth32Float;
const DEPTH_CLEAR: f32 = 0.0;
const DEPTH_COMPARE: CompareFunction = CompareFunction::Greater;

/// Matches `View` in `shaders/common/view.wesl`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct ViewUniform {
    clip_from_world: [[f32; 4]; 4],
    position_time: [f32; 4],
}

/// Matches `Object` in `shaders/mesh.wesl`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct ObjectUniform {
    world_from_local: [[f32; 4]; 4],
    normal_from_local: [[f32; 4]; 4],
    tint: [f32; 4],
}

/// The built-in meshes M1 can draw.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MeshKind {
    Cube,
    Plane,
}

#[derive(Copy, Clone, Debug)]
pub struct DrawItem {
    pub mesh: MeshKind,
    pub world_from_local: Mat4,
    /// Multiplies the vertex color (linear RGB; alpha unused for now).
    pub tint: Vec4,
}

#[derive(Copy, Clone, Debug)]
pub struct RenderView {
    pub camera: Camera,
    /// Seconds; exposed to shaders as `view.position_time.w`.
    pub time: f32,
    /// Linear RGB.
    pub clear_color: [f32; 3],
}

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error(transparent)]
    Shader(#[from] ShaderError),
    #[error(transparent)]
    Rhi(#[from] RhiError),
}

#[derive(Debug)]
pub struct Renderer {
    shaders: ShaderLibrary,
    color_format: TextureFormat,
    view_layout: BindGroupLayout,
    object_layout: BindGroupLayout,
    view_buffer: Buffer,
    view_group: BindGroup,
    object_buffer: Buffer,
    object_group: BindGroup,
    /// Byte distance between objects in `object_buffer` (dynamic-offset aligned).
    object_stride: u64,
    object_staging: Vec<u8>,
    pipeline: Option<RenderPipeline>,
    permutation_version: u32,
    depth: Option<Texture>,
    cube: GpuMesh,
    plane: GpuMesh,
    warned_overflow: bool,
}

impl Renderer {
    /// Builds GPU state for drawing into `color_format` targets. A shader that fails to
    /// compile is logged, not fatal: the frame is cleared until a hot reload fixes it.
    pub fn new(rhi: &Rhi, color_format: TextureFormat, shaders: ShaderLibrary) -> Self {
        let _span = info_span!("Renderer::new").entered();
        let view_size = size_of::<ViewUniform>() as u64;
        let object_size = size_of::<ObjectUniform>() as u64;
        let object_stride = object_size.next_multiple_of(rhi.uniform_offset_alignment());

        let view_layout = rhi.create_bind_group_layout(
            "View",
            &[BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::VertexFragment,
                ty: BindingType::UniformBuffer {
                    has_dynamic_offset: false,
                    min_binding_size: view_size,
                },
            }],
        );
        let object_layout = rhi.create_bind_group_layout(
            "Object",
            &[BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::VertexFragment,
                ty: BindingType::UniformBuffer {
                    has_dynamic_offset: true,
                    min_binding_size: object_size,
                },
            }],
        );

        let view_buffer = rhi.create_buffer(&BufferDesc {
            label: "View",
            size: view_size,
            usage: BufferUsage::UNIFORM | BufferUsage::COPY_DST,
        });
        let view_group = rhi.create_bind_group(
            "View",
            &view_layout,
            &[BindGroupEntry {
                binding: 0,
                resource: BindingResource::Buffer {
                    buffer: &view_buffer,
                    offset: 0,
                    size: None,
                },
            }],
        );
        let object_buffer = rhi.create_buffer(&BufferDesc {
            label: "Objects",
            size: object_stride * MAX_OBJECTS as u64,
            usage: BufferUsage::UNIFORM | BufferUsage::COPY_DST,
        });
        let object_group = rhi.create_bind_group(
            "Objects",
            &object_layout,
            &[BindGroupEntry {
                binding: 0,
                resource: BindingResource::Buffer {
                    buffer: &object_buffer,
                    offset: 0,
                    size: Some(object_size),
                },
            }],
        );

        let mut renderer = Self {
            shaders,
            color_format,
            view_layout,
            object_layout,
            view_buffer,
            view_group,
            object_buffer,
            object_group,
            object_stride,
            object_staging: Vec::new(),
            pipeline: None,
            permutation_version: R_DEBUG_NORMALS.version(),
            depth: None,
            cube: GpuMesh::upload(rhi, "Cube", &MeshData::cube(0.5, [1.0; 3])),
            plane: GpuMesh::upload(rhi, "Plane", &MeshData::plane(1.0, [1.0; 3])),
            warned_overflow: false,
        };
        renderer.rebuild_pipeline(rhi);
        renderer
    }

    /// True when the mesh pipeline compiled. False means frames are only cleared.
    pub fn has_pipeline(&self) -> bool {
        self.pipeline.is_some()
    }

    pub fn shaders_mut(&mut self) -> &mut ShaderLibrary {
        &mut self.shaders
    }

    /// Per-frame housekeeping: rebuilds pipelines after a shader edit or a permutation cvar
    /// change. On failure the previous pipeline keeps rendering.
    pub fn update(&mut self, rhi: &Rhi) {
        let edited = self.shaders.poll_changed();
        let permutation_changed = self.permutation_version != R_DEBUG_NORMALS.version();
        if edited || permutation_changed {
            self.permutation_version = R_DEBUG_NORMALS.version();
            if self.rebuild_pipeline(rhi) {
                info!(edited, permutation_changed, "mesh shader reloaded");
            }
        }
    }

    fn rebuild_pipeline(&mut self, rhi: &Rhi) -> bool {
        match self.build_pipeline(rhi) {
            Ok(p) => {
                self.pipeline = Some(p);
                true
            }
            Err(e) => {
                let keeping = if self.pipeline.is_some() {
                    "keeping the previous shader"
                } else {
                    "meshes will not draw until it compiles"
                };
                error!("mesh shader rejected ({keeping}):\n{e}");
                false
            }
        }
    }

    fn build_pipeline(&self, rhi: &Rhi) -> Result<RenderPipeline, RenderError> {
        let _span = info_span!("Renderer::BuildPipeline").entered();
        let compiled = self
            .shaders
            .compile("mesh", &[("debug_normals", R_DEBUG_NORMALS.get())])?;
        let module = rhi.create_shader_module("mesh", &compiled.wgsl)?;
        Ok(rhi.create_render_pipeline(&RenderPipelineDesc {
            label: "Mesh",
            shader: &module,
            vertex_entry: "vs_main",
            fragment_entry: "fs_main",
            vertex_layouts: &[Vertex::LAYOUT],
            bind_group_layouts: &[&self.view_layout, &self.object_layout],
            color_formats: &[self.color_format],
            depth: Some(DepthState {
                format: DEPTH_FORMAT,
                write_enabled: true,
                compare: DEPTH_COMPARE,
            }),
            cull_mode: CullMode::Back,
        })?)
    }

    /// Recreates the depth buffer when the target size changes (resize, first frame).
    fn depth_target(&mut self, rhi: &Rhi, size: (u32, u32)) -> &Texture {
        if self.depth.as_ref().is_none_or(|d| d.size() != size) {
            self.depth = Some(rhi.create_texture(&TextureDesc {
                label: "SceneDepth",
                size,
                format: DEPTH_FORMAT,
                usage: TextureUsage::RENDER_ATTACHMENT,
            }));
        }
        self.depth.as_ref().expect("created above")
    }

    /// Records the frame: uniforms upload, then the `Opaque` pass.
    pub fn render(&mut self, rhi: &Rhi, frame: &mut Frame, view: &RenderView, items: &[DrawItem]) {
        let _span = info_span!("Render::Scene", items = items.len()).entered();
        let size = frame.size();

        let camera = &view.camera;
        let view_uniform = ViewUniform {
            clip_from_world: camera
                .clip_from_world(aspect_ratio(size))
                .to_cols_array_2d(),
            position_time: camera.position.extend(view.time).to_array(),
        };
        rhi.write_buffer(&self.view_buffer, 0, bytemuck::bytes_of(&view_uniform));

        let items = if items.len() > MAX_OBJECTS {
            if !self.warned_overflow {
                warn!(
                    count = items.len(),
                    max = MAX_OBJECTS,
                    "too many draw items; extra skipped"
                );
                self.warned_overflow = true;
            }
            &items[..MAX_OBJECTS]
        } else {
            items
        };
        self.upload_objects(rhi, items);

        let depth = self.depth_target(rhi, size).clone();
        let [r, g, b] = view.clear_color;
        let mut pass = frame.render_pass(&RenderPassDesc {
            label: "Opaque",
            color: Some(LoadOp::Clear(ClearColor([r, g, b, 1.0]))),
            depth: Some(DepthAttachment {
                texture: &depth,
                load: LoadOp::Clear(DEPTH_CLEAR),
                store: false,
            }),
        });
        let Some(pipeline) = &self.pipeline else {
            return;
        };
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &self.view_group, &[]);
        for (i, item) in items.iter().enumerate() {
            let mesh = match item.mesh {
                MeshKind::Cube => &self.cube,
                MeshKind::Plane => &self.plane,
            };
            let offset = (i as u64 * self.object_stride) as u32;
            pass.set_bind_group(1, &self.object_group, &[offset]);
            pass.set_vertex_buffer(0, &mesh.vertex_buffer);
            pass.set_index_buffer(&mesh.index_buffer);
            pass.draw_indexed(0..mesh.index_count, 0, 0..1);
        }
    }

    /// Packs every object's uniforms at `object_stride` and uploads them in one write.
    fn upload_objects(&mut self, rhi: &Rhi, items: &[DrawItem]) {
        if items.is_empty() {
            return;
        }
        let stride = self.object_stride as usize;
        self.object_staging.clear();
        self.object_staging.resize(stride * items.len(), 0);
        for (i, item) in items.iter().enumerate() {
            let uniform = ObjectUniform {
                world_from_local: item.world_from_local.to_cols_array_2d(),
                normal_from_local: item
                    .world_from_local
                    .inverse()
                    .transpose()
                    .to_cols_array_2d(),
                tint: item.tint.to_array(),
            };
            let bytes = bytemuck::bytes_of(&uniform);
            self.object_staging[i * stride..i * stride + bytes.len()].copy_from_slice(bytes);
        }
        rhi.write_buffer(&self.object_buffer, 0, &self.object_staging);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform_sizes_match_shader_structs() {
        // WGSL: mat4x4<f32> = 64 bytes, vec4<f32> = 16 bytes, no padding needed.
        assert_eq!(size_of::<ViewUniform>(), 80);
        assert_eq!(size_of::<ObjectUniform>(), 144);
    }
}
