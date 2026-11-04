//! The forward renderer: one opaque pass that draws indexed meshes with per-view and per-object
//! uniforms into a reversed-Z depth buffer, an optional ground-grid overlay, then the UI pass.
//! It talks to the GPU only through `ge_rhi` (law 12).
//!
//! The draw list is handed in directly for now. M5 replaces it with extracted render proxies,
//! a render graph and cached draw packets (lore §3.7, §6.7).

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use ge_rhi::{
    BindGroup, BindGroupEntry, BindGroupLayout, BindGroupLayoutEntry, BindingResource, BindingType,
    BlendMode, Buffer, BufferDesc, BufferUsage, ClearColor, CompareFunction, CullMode,
    DepthAttachment, DepthState, Frame, LoadOp, RenderPassDesc, RenderPipeline, RenderPipelineDesc,
    Rhi, RhiError, ShaderStages, Texture, TextureDesc, TextureFormat, TextureUsage,
};
use ge_shader::{ShaderError, ShaderLibrary};
use glam::{Mat4, Vec3, Vec4};
use tracing::{error, info, info_span, warn};

use ge_asset::{AssetId, MaterialAsset, Mesh as AssetMesh};

use crate::Camera;
use crate::assets::{AssetUpload, GpuAssets};
use crate::camera::aspect_ratio;
use crate::lighting::{ColorManagement, LightItem, LightUniform, MAX_LIGHTS, ViewportShading};
use crate::mesh::{GpuMesh, MeshData, Vertex};
use crate::primitives::PrimitiveMesh;
use crate::ui::{UiDrawList, UiRenderer};

ge_core::declare_cvar!(pub static R_DEBUG_NORMALS: bool = (
    "r.DebugNormals",
    false,
    "Shade meshes with their world-space normals (switches the mesh shader permutation)"
));

/// Objects per frame. The GPU scene (M5/M12) removes this cap.
pub const MAX_OBJECTS: usize = 1024;

/// Generated primitive meshes unused for this many frames are freed (slider scrubbing in the
/// editor creates one per value).
const PRIMITIVE_CACHE_FRAMES: u64 = 120;

/// Reversed-Z: clear to 0 (infinitely far), keep fragments with greater depth.
const DEPTH_FORMAT: TextureFormat = TextureFormat::Depth32Float;
const DEPTH_CLEAR: f32 = 0.0;
const DEPTH_COMPARE: CompareFunction = CompareFunction::Greater;

/// Matches `View` in `shaders/common/view.wesl`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct ViewUniform {
    clip_from_world: [[f32; 4]; 4],
    world_from_clip: [[f32; 4]; 4],
    position_time: [f32; 4],
    color_management: [f32; 4],
    white_balance: [f32; 4],
    ambient: [f32; 4],
}

/// Matches `Lights` in `shaders/common/lighting.wesl`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct LightsUniform {
    items: [LightUniform; MAX_LIGHTS],
}

/// Matches `Object` in `shaders/mesh.wesl`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct ObjectUniform {
    world_from_local: [[f32; 4]; 4],
    normal_from_local: [[f32; 4]; 4],
    tint: [f32; 4],
    material: [f32; 4],
    emission: [f32; 4],
}

/// The meshes the renderer can draw until assets (M3) bring `Handle<Mesh>`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MeshKind {
    /// Unit cube (half-extent 0.5), flat shaded.
    Cube,
    /// Unit square in XZ (half-size 1) facing +Y.
    Plane,
    /// An editor primitive, generated and cached on first use.
    Primitive(PrimitiveMesh),
    /// A mesh asset (glTF primitive), uploaded when the asset loads.
    Asset(AssetId<AssetMesh>),
}

impl MeshKind {
    /// Half-extents of the local bounding box, for picking and selection outlines.
    pub fn half_extents(&self) -> Vec3 {
        match self {
            MeshKind::Cube => Vec3::splat(0.5),
            MeshKind::Plane => Vec3::new(1.0, 0.0, 1.0),
            MeshKind::Primitive(p) => p.shape.half_extents(),
            // Asset bounds live in the asset; callers that need them ask the AssetServer.
            MeshKind::Asset(_) => Vec3::splat(0.5),
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct DrawItem {
    pub mesh: MeshKind,
    pub world_from_local: Mat4,
    /// Base color: multiplies the vertex color (linear RGB; alpha unused for now).
    pub tint: Vec4,
    pub metallic: f32,
    pub roughness: f32,
    /// Emission color × strength (linear RGB, added after lighting).
    pub emission: Vec3,
    /// Draw back faces too (Blender's "Backface Culling" off).
    pub double_sided: bool,
    /// Textures come from this material asset (factors are already folded into the fields
    /// above by extract). `None` = white textures.
    pub material_asset: Option<AssetId<MaterialAsset>>,
}

impl DrawItem {
    /// A single-sided dielectric (metallic 0, roughness 0.5) with no emission.
    pub fn new(mesh: MeshKind, world_from_local: Mat4, tint: Vec4) -> Self {
        Self {
            mesh,
            world_from_local,
            tint,
            metallic: 0.0,
            roughness: 0.5,
            emission: Vec3::ZERO,
            double_sided: false,
            material_asset: None,
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct RenderView {
    pub camera: Camera,
    /// Seconds; exposed to shaders as `view.position_time.w`.
    pub time: f32,
    /// Linear RGB.
    pub clear_color: [f32; 3],
}

/// Viewport overlays drawn with the scene (the editor turns them on).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Overlays {
    pub grid: bool,
}

/// Everything one frame draws. [`FrameInputs::basic`] is the M1/M2 path: studio light, no
/// color management, no overlays or UI.
#[derive(Debug)]
pub struct FrameInputs<'a> {
    pub view: &'a RenderView,
    pub items: &'a [DrawItem],
    pub lights: &'a [LightItem],
    /// World ambient radiance (Rendered shading).
    pub ambient: Vec3,
    pub color: ColorManagement,
    pub shading: ViewportShading,
    /// Render ▸ Light Paths ▸ Intensity ▸ Direct Light.
    pub direct_light: f32,
    pub overlays: Overlays,
    pub ui: Option<&'a mut UiDrawList>,
    /// Asset changes to apply before drawing (drained).
    pub asset_uploads: Option<&'a mut Vec<AssetUpload>>,
}

impl<'a> FrameInputs<'a> {
    pub fn basic(view: &'a RenderView, items: &'a [DrawItem]) -> Self {
        Self {
            view,
            items,
            lights: &[],
            ambient: Vec3::ZERO,
            color: ColorManagement::default(),
            shading: ViewportShading::Solid,
            direct_light: 1.0,
            overlays: Overlays::default(),
            ui: None,
            asset_uploads: None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error(transparent)]
    Shader(#[from] ShaderError),
    #[error(transparent)]
    Rhi(#[from] RhiError),
}

#[derive(Debug)]
struct MeshPipelines {
    single_sided: RenderPipeline,
    double_sided: RenderPipeline,
}

#[derive(Debug)]
struct CachedPrimitive {
    mesh: GpuMesh,
    last_used: u64,
}

#[derive(Debug)]
pub struct Renderer {
    shaders: ShaderLibrary,
    color_format: TextureFormat,
    view_layout: BindGroupLayout,
    object_layout: BindGroupLayout,
    view_buffer: Buffer,
    lights_buffer: Buffer,
    view_group: BindGroup,
    object_buffer: Buffer,
    object_group: BindGroup,
    /// Byte distance between objects in `object_buffer` (dynamic-offset aligned).
    object_stride: u64,
    object_staging: Vec<u8>,
    pipelines: Option<MeshPipelines>,
    grid: Option<RenderPipeline>,
    permutation_version: u32,
    depth: Option<Texture>,
    cube: GpuMesh,
    plane: GpuMesh,
    primitives: HashMap<PrimitiveMesh, CachedPrimitive>,
    assets: GpuAssets,
    ui: UiRenderer,
    frame_index: u64,
    warned_overflow: bool,
}

impl Renderer {
    /// Builds GPU state for drawing into `color_format` targets. A shader that fails to
    /// compile is logged, not fatal: the frame is cleared until a hot reload fixes it.
    pub fn new(rhi: &Rhi, color_format: TextureFormat, shaders: ShaderLibrary) -> Self {
        let _span = info_span!("Renderer::new").entered();
        let view_size = size_of::<ViewUniform>() as u64;
        let lights_size = size_of::<LightsUniform>() as u64;
        let object_size = size_of::<ObjectUniform>() as u64;
        let object_stride = object_size.next_multiple_of(rhi.uniform_offset_alignment());

        let view_layout = rhi.create_bind_group_layout(
            "View",
            &[
                BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::VertexFragment,
                    ty: BindingType::UniformBuffer {
                        has_dynamic_offset: false,
                        min_binding_size: view_size,
                    },
                },
                BindGroupLayoutEntry {
                    binding: 1,
                    visibility: ShaderStages::Fragment,
                    ty: BindingType::UniformBuffer {
                        has_dynamic_offset: false,
                        min_binding_size: lights_size,
                    },
                },
            ],
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
        let lights_buffer = rhi.create_buffer(&BufferDesc {
            label: "Lights",
            size: lights_size,
            usage: BufferUsage::UNIFORM | BufferUsage::COPY_DST,
        });
        let view_group = rhi.create_bind_group(
            "View",
            &view_layout,
            &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::Buffer {
                        buffer: &view_buffer,
                        offset: 0,
                        size: None,
                    },
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Buffer {
                        buffer: &lights_buffer,
                        offset: 0,
                        size: None,
                    },
                },
            ],
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

        let ui = UiRenderer::new(rhi, color_format, &shaders);
        let assets = GpuAssets::new(rhi);
        let mut renderer = Self {
            shaders,
            color_format,
            view_layout,
            object_layout,
            view_buffer,
            lights_buffer,
            view_group,
            object_buffer,
            object_group,
            object_stride,
            object_staging: Vec::new(),
            pipelines: None,
            grid: None,
            permutation_version: R_DEBUG_NORMALS.version(),
            depth: None,
            cube: GpuMesh::upload(rhi, "Cube", &MeshData::cube(0.5, [1.0; 3])),
            plane: GpuMesh::upload(rhi, "Plane", &MeshData::plane(1.0, [1.0; 3])),
            primitives: HashMap::new(),
            assets,
            ui,
            frame_index: 0,
            warned_overflow: false,
        };
        renderer.rebuild_pipelines(rhi);
        renderer
    }

    /// True when the mesh pipeline compiled. False means frames are only cleared.
    pub fn has_pipeline(&self) -> bool {
        self.pipelines.is_some()
    }

    /// Names of pipelines whose shaders failed to build (empty when everything compiled).
    pub fn missing_pipelines(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        if self.pipelines.is_none() {
            missing.push("mesh");
        }
        if self.grid.is_none() {
            missing.push("grid");
        }
        if !self.ui.has_pipeline() {
            missing.push("ui");
        }
        missing
    }

    /// GPU copies held: `(meshes, textures, materials)`.
    pub fn gpu_asset_counts(&self) -> (usize, usize, usize) {
        self.assets.counts()
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
            if self.rebuild_pipelines(rhi) {
                info!(edited, permutation_changed, "scene shaders reloaded");
            }
        }
    }

    fn rebuild_pipelines(&mut self, rhi: &Rhi) -> bool {
        self.ui.rebuild_pipeline(rhi, &self.shaders);
        match self.build_grid(rhi) {
            Ok(p) => self.grid = Some(p),
            Err(e) => error!("grid shader rejected:\n{e}"),
        }
        match self.build_mesh_pipelines(rhi) {
            Ok(p) => {
                self.pipelines = Some(p);
                true
            }
            Err(e) => {
                let keeping = if self.pipelines.is_some() {
                    "keeping the previous shader"
                } else {
                    "meshes will not draw until it compiles"
                };
                error!("mesh shader rejected ({keeping}):\n{e}");
                false
            }
        }
    }

    fn build_mesh_pipelines(&self, rhi: &Rhi) -> Result<MeshPipelines, RenderError> {
        let _span = info_span!("Renderer::BuildPipeline").entered();
        let compiled = self
            .shaders
            .compile("mesh", &[("debug_normals", R_DEBUG_NORMALS.get())])?;
        let module = rhi.create_shader_module("mesh", &compiled.wgsl)?;
        let make = |label, cull_mode| {
            rhi.create_render_pipeline(&RenderPipelineDesc {
                label,
                shader: &module,
                vertex_entry: "vs_main",
                fragment_entry: "fs_main",
                vertex_layouts: &[Vertex::LAYOUT],
                bind_group_layouts: &[&self.view_layout, &self.object_layout, &self.assets.layout],
                color_formats: &[self.color_format],
                depth: Some(DepthState {
                    format: DEPTH_FORMAT,
                    write_enabled: true,
                    compare: DEPTH_COMPARE,
                }),
                cull_mode,
                blend: None,
            })
        };
        Ok(MeshPipelines {
            single_sided: make("Mesh", CullMode::Back)?,
            double_sided: make("MeshDoubleSided", CullMode::None)?,
        })
    }

    fn build_grid(&self, rhi: &Rhi) -> Result<RenderPipeline, RenderError> {
        let compiled = self.shaders.compile("grid", &[])?;
        let module = rhi.create_shader_module("grid", &compiled.wgsl)?;
        Ok(rhi.create_render_pipeline(&RenderPipelineDesc {
            label: "Grid",
            shader: &module,
            vertex_entry: "vs_main",
            fragment_entry: "fs_main",
            vertex_layouts: &[],
            bind_group_layouts: &[&self.view_layout],
            color_formats: &[self.color_format],
            depth: Some(DepthState {
                format: DEPTH_FORMAT,
                write_enabled: false,
                compare: CompareFunction::GreaterEqual,
            }),
            cull_mode: CullMode::None,
            blend: Some(BlendMode::Alpha),
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

    /// The M1/M2 entry point: studio-lit scene, no overlays or UI.
    pub fn render(&mut self, rhi: &Rhi, frame: &mut Frame, view: &RenderView, items: &[DrawItem]) {
        self.render_frame(rhi, frame, FrameInputs::basic(view, items));
    }

    /// Records the frame: uniforms upload, the `Opaque` pass (meshes, then the grid overlay),
    /// then the `Ui` pass.
    pub fn render_frame(&mut self, rhi: &Rhi, frame: &mut Frame, inputs: FrameInputs<'_>) {
        let _span = info_span!("Render::Scene", items = inputs.items.len()).entered();
        self.frame_index += 1;
        if let Some(uploads) = inputs.asset_uploads {
            self.assets.apply(rhi, uploads);
        }
        let size = frame.size();
        let view = inputs.view;

        let camera = &view.camera;
        let clip_from_world = camera.clip_from_world(aspect_ratio(size));
        let lights = &inputs.lights[..inputs.lights.len().min(MAX_LIGHTS)];
        let cm = &inputs.color;
        let view_uniform = ViewUniform {
            clip_from_world: clip_from_world.to_cols_array_2d(),
            world_from_clip: clip_from_world.inverse().to_cols_array_2d(),
            position_time: camera.position.extend(view.time).to_array(),
            color_management: [
                cm.exposure_ev.exp2(),
                cm.gamma,
                cm.view as u8 as f32,
                match inputs.shading {
                    ViewportShading::Solid => 0.0,
                    ViewportShading::Rendered => 1.0,
                },
            ],
            white_balance: [
                cm.white_balance[0],
                cm.white_balance[1],
                cm.white_balance[2],
                inputs.direct_light,
            ],
            ambient: inputs.ambient.extend(lights.len() as f32).to_array(),
        };
        rhi.write_buffer(&self.view_buffer, 0, bytemuck::bytes_of(&view_uniform));
        let mut lights_uniform = LightsUniform::zeroed();
        for (slot, light) in lights_uniform.items.iter_mut().zip(lights) {
            *slot = LightUniform::pack(light);
        }
        rhi.write_buffer(&self.lights_buffer, 0, bytemuck::bytes_of(&lights_uniform));

        let items = if inputs.items.len() > MAX_OBJECTS {
            if !self.warned_overflow {
                warn!(
                    count = inputs.items.len(),
                    max = MAX_OBJECTS,
                    "too many draw items; extra skipped"
                );
                self.warned_overflow = true;
            }
            &inputs.items[..MAX_OBJECTS]
        } else {
            inputs.items
        };
        self.upload_objects(rhi, items);
        self.prepare_primitives(rhi, items);

        let depth = self.depth_target(rhi, size).clone();
        let [r, g, b] = view.clear_color;
        {
            let mut pass = frame.render_pass(&RenderPassDesc {
                label: "Opaque",
                color: Some(LoadOp::Clear(ClearColor([r, g, b, 1.0]))),
                depth: Some(DepthAttachment {
                    texture: &depth,
                    load: LoadOp::Clear(DEPTH_CLEAR),
                    store: false,
                }),
            });
            if let Some(pipelines) = &self.pipelines {
                pass.set_bind_group(0, &self.view_group, &[]);
                let mut current_double: Option<bool> = None;
                let mut current_material: Option<Option<AssetId<MaterialAsset>>> = None;
                for (i, item) in items.iter().enumerate() {
                    let mesh = match &item.mesh {
                        MeshKind::Cube => &self.cube,
                        MeshKind::Plane => &self.plane,
                        MeshKind::Primitive(p) => match self.primitives.get(p) {
                            Some(c) => &c.mesh,
                            None => continue,
                        },
                        // Not uploaded yet (still loading): skip this frame.
                        MeshKind::Asset(id) => match self.assets.mesh(*id) {
                            Some(m) => m,
                            None => continue,
                        },
                    };
                    if current_material != Some(item.material_asset) {
                        pass.set_bind_group(
                            2,
                            self.assets.material_group(item.material_asset),
                            &[],
                        );
                        current_material = Some(item.material_asset);
                    }
                    if current_double != Some(item.double_sided) {
                        pass.set_pipeline(if item.double_sided {
                            &pipelines.double_sided
                        } else {
                            &pipelines.single_sided
                        });
                        current_double = Some(item.double_sided);
                    }
                    let offset = (i as u64 * self.object_stride) as u32;
                    pass.set_bind_group(1, &self.object_group, &[offset]);
                    pass.set_vertex_buffer(0, &mesh.vertex_buffer);
                    pass.set_index_buffer(&mesh.index_buffer);
                    pass.draw_indexed(0..mesh.index_count, 0, 0..1);
                }
            }
            if inputs.overlays.grid
                && let Some(grid) = &self.grid
            {
                pass.set_pipeline(grid);
                pass.set_bind_group(0, &self.view_group, &[]);
                pass.draw(0..3, 0..1);
            }
        }

        if let Some(ui) = inputs.ui {
            self.ui.render(rhi, frame, ui);
        }
    }

    /// Generates any primitive meshes this frame needs and frees ones that went unused.
    fn prepare_primitives(&mut self, rhi: &Rhi, items: &[DrawItem]) {
        let frame = self.frame_index;
        for item in items {
            if let MeshKind::Primitive(p) = &item.mesh {
                self.primitives
                    .entry(*p)
                    .and_modify(|c| c.last_used = frame)
                    .or_insert_with(|| {
                        let _span = info_span!("Render::BuildPrimitive").entered();
                        CachedPrimitive {
                            mesh: GpuMesh::upload(rhi, p.shape.label(), &p.shape.build(p.smooth)),
                            last_used: frame,
                        }
                    });
            }
        }
        self.primitives
            .retain(|_, c| frame - c.last_used <= PRIMITIVE_CACHE_FRAMES);
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
                material: [item.metallic, item.roughness, 0.0, 0.0],
                emission: item.emission.extend(0.0).to_array(),
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
        assert_eq!(size_of::<ViewUniform>(), 192);
        assert_eq!(size_of::<ObjectUniform>(), 176);
        assert_eq!(size_of::<LightsUniform>(), 64 * MAX_LIGHTS);
    }
}
