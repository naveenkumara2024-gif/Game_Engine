//! The forward+ renderer (plan 0006, lore §6.7). One frame:
//!
//! ```text
//! Shadows      sun cascades + spot/point/area atlas            (Rendered shading only)
//! DepthPrepass opaque + masked, reversed-Z
//! LightCulling compute: lights → 16×9×24 clusters
//! Opaque       forward+ PBR → SceneColor (HDR), then sky, sorted transparents, grid
//! Bloom        dual-filter chain
//! Tonemap      color management → display target
//! Ui
//! ```
//!
//! It talks to the GPU only through `ge_rhi` (law 12). The pass order is fixed for now; the
//! render graph (M5) will own it, with transient aliasing and pass culling.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use ge_asset::{AssetId, MaterialAsset, Mesh as AssetMesh};
use ge_rhi::{
    BindGroup, BindGroupEntry, BindGroupLayout, BindGroupLayoutEntry, BindingResource, BindingType,
    BlendMode, Buffer, BufferDesc, BufferUsage, ClearColor, ColorTarget, CompareFunction, CullMode,
    DepthBias, DepthState, DepthTarget, Frame, LoadOp, RenderPass, RenderPipeline,
    RenderPipelineDesc, Rhi, RhiError, ShaderModule, ShaderStages, TargetPassDesc, TextureFormat,
};
use ge_shader::{ShaderError, ShaderLibrary};
use glam::{Mat4, Vec3, Vec4};
use tracing::{error, info, info_span, warn};

use crate::Camera;
use crate::assets::{AssetUpload, GpuAssets};
use crate::camera::aspect_ratio;
use crate::frame::{
    CLUSTERS, FrameResources, GroupInputs, R_CLUSTERS_FAR, ViewUniform, slice_params,
};
use crate::ibl::{EnvKey, Environment};
use crate::lighting::{
    ColorManagement, LightItem, LightKind, LightUniform, MAX_LIGHTS, RenderWorld, ViewportShading,
};
use crate::mesh::{GpuMesh, MeshData, Vertex};
use crate::post::{BloomSettings, DEPTH_FORMAT, Post, SCENE_COLOR_FORMAT};
use crate::primitives::PrimitiveMesh;
use crate::shadows::{self, ShadowMaps, ShadowPlan};
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
const DEPTH_CLEAR: f32 = 0.0;

/// Matches `Object` in `shaders/common/object.wesl`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct ObjectUniform {
    world_from_local: [[f32; 4]; 4],
    normal_from_local: [[f32; 4]; 4],
    tint: [f32; 4],
    material: [f32; 4],
    emission: [f32; 4],
}

/// The meshes the renderer can draw.
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

/// How a draw handles alpha (Blender's Render Method + glTF alpha modes).
#[derive(Copy, Clone, Debug, PartialEq, Default)]
pub enum DrawAlpha {
    #[default]
    Opaque,
    /// Cutout: fragments below the cutoff are discarded (also in the prepass and shadows).
    Mask(f32),
    /// Alpha-blended, sorted back to front, after opaque geometry.
    Blend,
}

#[derive(Copy, Clone, Debug)]
pub struct DrawItem {
    pub mesh: MeshKind,
    pub world_from_local: Mat4,
    /// Base color (rgb) × vertex color, and alpha (a).
    pub tint: Vec4,
    pub metallic: f32,
    pub roughness: f32,
    /// Emission color × strength (linear RGB, added after lighting).
    pub emission: Vec3,
    /// Draw back faces too (Blender's "Backface Culling" off).
    pub double_sided: bool,
    /// Textures come from this material asset (factors are already folded into the fields
    /// above by extract). `None` = white / flat textures.
    pub material_asset: Option<AssetId<MaterialAsset>>,
    pub alpha: DrawAlpha,
    /// Normal map strength (0 = ignore the normal map).
    pub normal_strength: f32,
    /// Dielectric reflectance at normal incidence (from IOR and Specular IOR Level).
    pub f0: f32,
    pub cast_shadow: bool,
    /// Back faces cast shadows too.
    pub shadow_double_sided: bool,
    /// Blended objects cast shadows as an alpha cutout ("Transparent Shadows").
    pub transparent_shadows: bool,
}

impl DrawItem {
    /// An opaque, single-sided dielectric (metallic 0, roughness 0.5, F0 0.04) that casts
    /// shadows.
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
            alpha: DrawAlpha::Opaque,
            normal_strength: 1.0,
            f0: 0.04,
            cast_shadow: true,
            shadow_double_sided: false,
            transparent_shadows: false,
        }
    }

    fn is_blended(&self) -> bool {
        self.alpha == DrawAlpha::Blend
    }

    fn cutoff(&self) -> f32 {
        match self.alpha {
            DrawAlpha::Mask(c) => c.clamp(1e-3, 1.0),
            _ => 0.0,
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct RenderView {
    pub camera: Camera,
    /// Seconds; exposed to shaders as `view.position_time.w`.
    pub time: f32,
    /// Background radiance (linear RGB) when there's no environment map.
    pub clear_color: [f32; 3],
}

/// Viewport overlays drawn with the scene (the editor turns them on).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Overlays {
    pub grid: bool,
}

/// Everything one frame draws. [`FrameInputs::basic`] is the M1/M2 path: studio light,
/// Standard view transform, no world, overlays, bloom or UI.
#[derive(Debug)]
pub struct FrameInputs<'a> {
    pub view: &'a RenderView,
    pub items: &'a [DrawItem],
    pub lights: &'a [LightItem],
    /// The world (environment, IBL). `None` = flat `ambient` and `view.clear_color`.
    pub world: Option<RenderWorld>,
    /// Flat ambient radiance used when there is no world.
    pub ambient: Vec3,
    pub color: ColorManagement,
    /// Render ▸ Color Management ▸ Look (0 = None).
    pub look: i32,
    pub shading: ViewportShading,
    /// Render ▸ Light Paths ▸ Intensity ▸ Direct Light.
    pub direct_light: f32,
    pub bloom: BloomSettings,
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
            world: None,
            ambient: Vec3::ZERO,
            color: ColorManagement::default(),
            look: 0,
            shading: ViewportShading::Solid,
            direct_light: 1.0,
            bloom: BloomSettings::OFF,
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

/// `[single-sided, double-sided]`.
type Sided = [RenderPipeline; 2];

#[derive(Debug)]
struct ScenePipelines {
    prepass: Sided,
    prepass_masked: Sided,
    opaque: Sided,
    transparent: Sided,
    shadow: Sided,
    shadow_masked: Sided,
    sky: RenderPipeline,
}

#[derive(Debug)]
struct CachedPrimitive {
    mesh: GpuMesh,
    last_used: u64,
}

#[derive(Debug)]
pub struct Renderer {
    shaders: ShaderLibrary,
    frame: FrameResources,
    object_layout: BindGroupLayout,
    object_buffer: Buffer,
    object_group: BindGroup,
    /// Byte distance between objects in `object_buffer` (dynamic-offset aligned).
    object_stride: u64,
    object_staging: Vec<u8>,
    pipelines: Option<ScenePipelines>,
    grid: Option<RenderPipeline>,
    permutation_version: u32,
    cube: GpuMesh,
    plane: GpuMesh,
    primitives: HashMap<PrimitiveMesh, CachedPrimitive>,
    assets: GpuAssets,
    ui: UiRenderer,
    post: Post,
    shadows: ShadowMaps,
    shadow_pool: i32,
    shadow_resolution_version: u32,
    env: Environment,
    group_dirty: bool,
    frame_index: u64,
    warned_overflow: bool,
}

impl Renderer {
    /// Builds GPU state for drawing into `color_format` targets. A shader that fails to
    /// compile is logged, not fatal: the frame is cleared until a hot reload fixes it.
    pub fn new(rhi: &Rhi, color_format: TextureFormat, shaders: ShaderLibrary) -> Self {
        let _span = info_span!("Renderer::new").entered();
        let object_size = size_of::<ObjectUniform>() as u64;
        let object_stride = object_size.next_multiple_of(rhi.uniform_offset_alignment());
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
        let limits = rhi.limits();
        info!(?limits, "renderer device limits");

        let frame = FrameResources::new(rhi);
        let ui = UiRenderer::new(rhi, color_format, &shaders);
        let assets = GpuAssets::new(rhi);
        let post = Post::new(rhi, color_format, size_of::<ViewUniform>() as u64);
        let shadow_pool = shadows::R_SHADOW_POOL.get();
        let shadow_maps = ShadowMaps::new(
            rhi,
            shadows::csm_resolution(),
            shadows::atlas_resolution_for_pool(shadow_pool),
        );
        let env = Environment::new(rhi);
        let mut renderer = Self {
            shaders,
            frame,
            object_layout,
            object_buffer,
            object_group,
            object_stride,
            object_staging: Vec::new(),
            pipelines: None,
            grid: None,
            permutation_version: R_DEBUG_NORMALS.version(),
            cube: GpuMesh::upload(rhi, "Cube", &{
                let mut m = MeshData::cube(0.5, [1.0; 3]);
                m.generate_tangents();
                m
            }),
            plane: GpuMesh::upload(rhi, "Plane", &{
                let mut m = MeshData::plane(1.0, [1.0; 3]);
                m.generate_tangents();
                m
            }),
            primitives: HashMap::new(),
            assets,
            ui,
            post,
            shadows: shadow_maps,
            shadow_pool,
            shadow_resolution_version: shadows::R_SHADOW_RESOLUTION.version(),
            env,
            group_dirty: true,
            frame_index: 0,
            warned_overflow: false,
        };
        renderer.rebuild_pipelines(rhi);
        renderer
    }

    /// True when the mesh pipelines compiled. False means frames are only cleared.
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
        if !self.frame.has_cull() {
            missing.push("cluster_cull");
        }
        if !self.post.has_pipelines() {
            missing.push("post");
        }
        if !self.env.has_pipelines() {
            missing.push("env");
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
    /// change, and shadow maps after a resolution change. On failure the previous pipeline
    /// keeps rendering.
    pub fn update(&mut self, rhi: &Rhi) {
        let edited = self.shaders.poll_changed();
        let permutation_changed = self.permutation_version != R_DEBUG_NORMALS.version();
        if edited || permutation_changed {
            self.permutation_version = R_DEBUG_NORMALS.version();
            if self.rebuild_pipelines(rhi) {
                info!(edited, permutation_changed, "scene shaders reloaded");
            }
        }
        let pool = shadows::R_SHADOW_POOL.get();
        let res_version = shadows::R_SHADOW_RESOLUTION.version();
        if pool != self.shadow_pool || res_version != self.shadow_resolution_version {
            self.shadow_pool = pool;
            self.shadow_resolution_version = res_version;
            self.shadows = ShadowMaps::new(
                rhi,
                shadows::csm_resolution(),
                shadows::atlas_resolution_for_pool(pool),
            );
            self.pipelines = None;
            self.rebuild_pipelines(rhi);
            self.group_dirty = true;
            info!(
                csm = self.shadows.csm_resolution,
                atlas = self.shadows.atlas_resolution,
                "shadow maps resized"
            );
        }
    }

    fn rebuild_pipelines(&mut self, rhi: &Rhi) -> bool {
        self.ui.rebuild_pipeline(rhi, &self.shaders);
        self.frame.rebuild_pipelines(rhi, &self.shaders);
        self.post.rebuild_pipelines(rhi, &self.shaders);
        self.env.rebuild_pipelines(rhi, &self.shaders);
        match self.build_grid(rhi) {
            Ok(p) => self.grid = Some(p),
            Err(e) => error!("grid shader rejected:\n{e}"),
        }
        match self.build_scene_pipelines(rhi) {
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

    fn module(
        &self,
        rhi: &Rhi,
        name: &str,
        features: &[(&str, bool)],
    ) -> Result<ShaderModule, RenderError> {
        let compiled = self.shaders.compile(name, features)?;
        Ok(rhi.create_shader_module(name, &compiled.wgsl)?)
    }

    fn build_scene_pipelines(&self, rhi: &Rhi) -> Result<ScenePipelines, RenderError> {
        let _span = info_span!("Renderer::BuildPipeline").entered();
        let mesh = self.module(rhi, "mesh", &[("debug_normals", R_DEBUG_NORMALS.get())])?;
        let shadow = self.module(rhi, "shadow_depth", &[])?;
        let sky = self.module(rhi, "sky", &[])?;
        let scene_layouts = [&self.frame.layout, &self.object_layout, &self.assets.layout];
        let shadow_layouts = [
            &self.shadows.view_layout,
            &self.object_layout,
            &self.assets.layout,
        ];
        let cull = |double: bool| {
            if double {
                CullMode::None
            } else {
                CullMode::Back
            }
        };

        struct Kind<'a> {
            label: &'a str,
            module: &'a ShaderModule,
            vs: &'a str,
            fs: Option<&'a str>,
            colors: &'a [TextureFormat],
            depth_write: bool,
            compare: CompareFunction,
            blend: Option<BlendMode>,
            bias: Option<DepthBias>,
            shadow: bool,
        }
        let make = |k: &Kind<'_>, double: bool| {
            rhi.create_render_pipeline(&RenderPipelineDesc {
                label: k.label,
                shader: k.module,
                vertex_entry: k.vs,
                fragment_entry: k.fs,
                vertex_layouts: &[Vertex::LAYOUT],
                bind_group_layouts: if k.shadow {
                    &shadow_layouts
                } else {
                    &scene_layouts
                },
                color_formats: k.colors,
                depth: Some(DepthState {
                    format: DEPTH_FORMAT,
                    write_enabled: k.depth_write,
                    compare: k.compare,
                }),
                cull_mode: cull(double),
                blend: k.blend,
                depth_bias: k.bias,
            })
        };
        let sided =
            |k: Kind<'_>| -> Result<Sided, RhiError> { Ok([make(&k, false)?, make(&k, true)?]) };
        let color = [SCENE_COLOR_FORMAT];
        // Reversed-Z: push shadow depths *away* from the light (smaller values).
        let shadow_bias = Some(DepthBias {
            constant: -2,
            slope_scale: -2.5,
            clamp: 0.0,
        });
        Ok(ScenePipelines {
            prepass: sided(Kind {
                label: "DepthPrepass",
                module: &mesh,
                vs: "vs_depth",
                fs: None,
                colors: &[],
                depth_write: true,
                compare: CompareFunction::Greater,
                blend: None,
                bias: None,
                shadow: false,
            })?,
            prepass_masked: sided(Kind {
                label: "DepthPrepassMasked",
                module: &mesh,
                vs: "vs_depth",
                fs: Some("fs_alpha_test"),
                colors: &[],
                depth_write: true,
                compare: CompareFunction::Greater,
                blend: None,
                bias: None,
                shadow: false,
            })?,
            opaque: sided(Kind {
                label: "Opaque",
                module: &mesh,
                vs: "vs_main",
                fs: Some("fs_main"),
                colors: &color,
                depth_write: false,
                compare: CompareFunction::GreaterEqual,
                blend: None,
                bias: None,
                shadow: false,
            })?,
            transparent: sided(Kind {
                label: "Transparent",
                module: &mesh,
                vs: "vs_main",
                fs: Some("fs_main"),
                colors: &color,
                depth_write: false,
                compare: CompareFunction::Greater,
                blend: Some(BlendMode::Alpha),
                bias: None,
                shadow: false,
            })?,
            shadow: sided(Kind {
                label: "Shadow",
                module: &shadow,
                vs: "vs_shadow",
                fs: None,
                colors: &[],
                depth_write: true,
                compare: CompareFunction::Greater,
                blend: None,
                bias: shadow_bias,
                shadow: true,
            })?,
            shadow_masked: sided(Kind {
                label: "ShadowMasked",
                module: &shadow,
                vs: "vs_shadow",
                fs: Some("fs_shadow_masked"),
                colors: &[],
                depth_write: true,
                compare: CompareFunction::Greater,
                blend: None,
                bias: shadow_bias,
                shadow: true,
            })?,
            sky: rhi.create_render_pipeline(&RenderPipelineDesc {
                label: "Sky",
                shader: &sky,
                vertex_entry: "vs_main",
                fragment_entry: Some("fs_main"),
                vertex_layouts: &[],
                bind_group_layouts: &[&self.frame.layout],
                color_formats: &color,
                depth: Some(DepthState {
                    format: DEPTH_FORMAT,
                    write_enabled: false,
                    compare: CompareFunction::GreaterEqual,
                }),
                cull_mode: CullMode::None,
                blend: None,
                depth_bias: None,
            })?,
        })
    }

    fn build_grid(&self, rhi: &Rhi) -> Result<RenderPipeline, RenderError> {
        let module = self.module(rhi, "grid", &[])?;
        Ok(rhi.create_render_pipeline(&RenderPipelineDesc {
            label: "Grid",
            shader: &module,
            vertex_entry: "vs_main",
            fragment_entry: Some("fs_main"),
            vertex_layouts: &[],
            bind_group_layouts: &[&self.frame.layout],
            color_formats: &[SCENE_COLOR_FORMAT],
            depth: Some(DepthState {
                format: DEPTH_FORMAT,
                write_enabled: false,
                compare: CompareFunction::GreaterEqual,
            }),
            cull_mode: CullMode::None,
            blend: Some(BlendMode::Alpha),
            depth_bias: None,
        })?)
    }

    /// The M1/M2 entry point: studio-lit scene, no overlays or UI.
    pub fn render(&mut self, rhi: &Rhi, frame: &mut Frame, view: &RenderView, items: &[DrawItem]) {
        self.render_frame(rhi, frame, FrameInputs::basic(view, items));
    }

    /// Records one frame (see the module docs for the pass order).
    pub fn render_frame(&mut self, rhi: &Rhi, frame: &mut Frame, inputs: FrameInputs<'_>) {
        let _span = info_span!("Render::Scene", items = inputs.items.len()).entered();
        self.frame_index += 1;
        if let Some(uploads) = inputs.asset_uploads {
            self.assets.apply(rhi, uploads);
        }
        let size = frame.size();
        if size.0 == 0 || size.1 == 0 {
            return;
        }
        self.post.ensure_targets(rhi, size, &self.frame.view_buffer);

        let view = inputs.view;
        let camera = &view.camera;
        let aspect = aspect_ratio(size);
        let clip_from_view = camera.clip_from_view(aspect);
        let view_from_world = camera.view_from_world();
        let clip_from_world = clip_from_view * view_from_world;
        let rendered = inputs.shading == ViewportShading::Rendered;

        // Lights: suns first (they skip culling), capped.
        let mut lights: Vec<LightItem> = Vec::with_capacity(inputs.lights.len());
        if rendered {
            lights.extend(inputs.lights.iter().filter(|l| l.kind == LightKind::Sun));
            lights.extend(inputs.lights.iter().filter(|l| l.kind != LightKind::Sun));
            lights.truncate(MAX_LIGHTS);
        }
        let sun_count = lights.iter().filter(|l| l.kind == LightKind::Sun).count() as u32;

        let shadows_on = rendered && shadows::R_SHADOWS.get() && self.pipelines.is_some();
        let plan = if shadows_on {
            shadows::plan(
                &lights,
                camera,
                aspect,
                self.shadows.csm_resolution,
                self.shadows.atlas_resolution,
            )
        } else {
            ShadowPlan {
                slots: vec![None; lights.len()],
                ..Default::default()
            }
        };
        let light_uniforms: Vec<LightUniform> = lights
            .iter()
            .zip(&plan.slots)
            .map(|(l, slot)| LightUniform::pack(l, *slot))
            .collect();

        // Environment (rebuilt only when the world changes).
        let ibl_on = rendered && inputs.world.is_some() && self.env.has_pipelines();
        if let (true, Some(world)) = (ibl_on, inputs.world) {
            let sun = lights
                .iter()
                .find(|l| l.kind == LightKind::Sun)
                .map_or(Vec3::new(0.3, 0.8, 0.5).normalize(), |l| -l.direction);
            let hdri_texture = world.hdri.and_then(|id| self.assets.texture(id));
            let key = EnvKey {
                surface: world.surface,
                color: world.color.to_array(),
                strength: world.strength,
                sun: sun.to_array(),
                hdri: world
                    .hdri
                    .filter(|_| hdri_texture.is_some())
                    .map(|id| (id, self.assets.texture_version(id))),
                rotation: world.hdri_rotation,
                resolution: world.probe_resolution,
            };
            let hdri_texture = hdri_texture.cloned();
            if self.env.update(rhi, frame, key, hdri_texture.as_ref()) {
                self.group_dirty = true;
            }
        }
        if self.group_dirty || self.frame.group.is_none() {
            self.frame.build_group(
                rhi,
                &GroupInputs {
                    cascade_maps: &self.shadows.csm,
                    shadow_atlas: &self.shadows.atlas,
                    shadow_sampler: &self.shadows.sampler,
                    shadow_data: &self.shadows.data,
                    env_cube: &self.env.specular,
                    env_sampler: &self.env.sampler,
                    brdf_lut: &self.env.brdf_lut,
                    env_sh: &self.env.sh,
                },
            );
            self.group_dirty = false;
        }

        let near = camera.near;
        let far = R_CLUSTERS_FAR.get().max(near * 10.0);
        let (scale, bias) = slice_params(near, far);
        let cm = &inputs.color;
        let flat_ambient = if rendered && !ibl_on {
            inputs.world.map_or(inputs.ambient, |w| w.radiance())
        } else {
            inputs.ambient
        };
        let view_uniform = ViewUniform {
            clip_from_world: clip_from_world.to_cols_array_2d(),
            world_from_clip: clip_from_world.inverse().to_cols_array_2d(),
            view_from_world: view_from_world.to_cols_array_2d(),
            view_from_clip: clip_from_view.inverse().to_cols_array_2d(),
            position_time: camera.position.extend(view.time).to_array(),
            color_management: [
                cm.exposure_ev.exp2(),
                cm.gamma,
                cm.view.as_cvar() as f32,
                if rendered { 1.0 } else { 0.0 },
            ],
            white_balance: [
                cm.white_balance[0],
                cm.white_balance[1],
                cm.white_balance[2],
                inputs.direct_light,
            ],
            ambient: flat_ambient.extend(1.0).to_array(),
            screen: [size.0 as f32, size.1 as f32, near, far],
            cluster_dims: [CLUSTERS[0], CLUSTERS[1], CLUSTERS[2], sun_count],
            counts: [
                lights.len().min(crate::lighting::MAX_LIGHTS) as u32,
                self.env.mip_count,
                u32::from(ibl_on && self.env.ready),
                u32::from(shadows_on && !plan.views.is_empty())
                    | (u32::from(crate::frame::R_CLUSTERS_BRUTE_FORCE.get()) << 1),
            ],
            params: [
                scale,
                bias,
                inputs.look.clamp(0, 4) as f32,
                inputs.bloom.intensity,
            ],
            background: Vec3::from(view.clear_color).extend(1.0).to_array(),
        };
        self.frame.upload(rhi, &view_uniform, &light_uniforms);

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

        if let Some(p) = &self.pipelines
            && shadows_on
            && !plan.views.is_empty()
        {
            self.shadow_passes(rhi, frame, p, &plan, items);
        }

        let Some(targets) = &self.post.targets else {
            return;
        };
        // Depth prepass: opaque + masked.
        {
            let mut pass = frame.pass_to(&TargetPassDesc {
                label: "DepthPrepass",
                color: &[],
                depth: Some(DepthTarget {
                    view: &targets.depth_view,
                    load: LoadOp::Clear(DEPTH_CLEAR),
                    store: true,
                }),
            });
            if let (Some(p), Some(group)) = (&self.pipelines, &self.frame.group) {
                pass.set_bind_group(0, group, &[]);
                self.draw_items(&mut pass, items, |item| {
                    (!item.is_blended()).then(|| {
                        let masked = matches!(item.alpha, DrawAlpha::Mask(_));
                        let set = if masked {
                            &p.prepass_masked
                        } else {
                            &p.prepass
                        };
                        &set[usize::from(item.double_sided)]
                    })
                });
            }
        }

        self.frame.cull(frame);

        // Main pass: opaque, sky, transparent (back to front), grid.
        {
            let [r, g, b] = view.clear_color;
            let mut pass = frame.pass_to(&TargetPassDesc {
                label: "Opaque",
                color: &[ColorTarget {
                    view: &targets.color_view,
                    load: LoadOp::Clear(ClearColor([r, g, b, 1.0])),
                }],
                depth: Some(DepthTarget {
                    view: &targets.depth_view,
                    load: LoadOp::Load,
                    store: true,
                }),
            });
            if let (Some(p), Some(group)) = (&self.pipelines, &self.frame.group) {
                pass.set_bind_group(0, group, &[]);
                self.draw_items(&mut pass, items, |item| {
                    (!item.is_blended()).then(|| &p.opaque[usize::from(item.double_sided)])
                });
                pass.set_pipeline(&p.sky);
                pass.set_bind_group(0, group, &[]);
                pass.draw(0..3, 0..1);

                let mut blended: Vec<(usize, f32)> = items
                    .iter()
                    .enumerate()
                    .filter(|(_, i)| i.is_blended())
                    .map(|(n, i)| {
                        (
                            n,
                            i.world_from_local
                                .w_axis
                                .truncate()
                                .distance_squared(camera.position),
                        )
                    })
                    .collect();
                if !blended.is_empty() {
                    blended.sort_by(|a, b| b.1.total_cmp(&a.1));
                    pass.set_bind_group(0, group, &[]);
                    let order: Vec<usize> = blended.into_iter().map(|(n, _)| n).collect();
                    self.draw_indexed_subset(&mut pass, items, &order, |item| {
                        Some(&p.transparent[usize::from(item.double_sided)])
                    });
                }
            }
            if inputs.overlays.grid
                && let (Some(grid), Some(group)) = (&self.grid, &self.frame.group)
            {
                pass.set_pipeline(grid);
                pass.set_bind_group(0, group, &[]);
                pass.draw(0..3, 0..1);
            }
        }

        self.post.bloom(rhi, frame, inputs.bloom);
        let backbuffer = frame.backbuffer();
        self.post.tonemap(frame, &backbuffer);

        if let Some(ui) = inputs.ui {
            self.ui.render(rhi, frame, ui);
        }
    }

    fn shadow_passes(
        &self,
        rhi: &Rhi,
        frame: &mut Frame,
        p: &ScenePipelines,
        plan: &ShadowPlan,
        items: &[DrawItem],
    ) {
        let offsets = self.shadows.upload(rhi, plan);
        let pick = |item: &DrawItem| -> Option<&RenderPipeline> {
            if !item.cast_shadow || (item.is_blended() && !item.transparent_shadows) {
                return None;
            }
            let masked = matches!(item.alpha, DrawAlpha::Mask(_)) || item.is_blended();
            let set = if masked { &p.shadow_masked } else { &p.shadow };
            Some(&set[usize::from(item.double_sided || item.shadow_double_sided)])
        };
        for (view, &offset) in plan.views.iter().zip(&offsets) {
            if let Some(layer) = view.cascade {
                let mut pass = frame.pass_to(&TargetPassDesc {
                    label: "Shadows.Cascade",
                    color: &[],
                    depth: Some(DepthTarget {
                        view: &self.shadows.csm_layers[layer],
                        load: LoadOp::Clear(DEPTH_CLEAR),
                        store: true,
                    }),
                });
                pass.set_bind_group(0, &self.shadows.view_group, &[offset]);
                self.draw_items(&mut pass, items, pick);
            }
        }
        let locals: Vec<_> = plan
            .views
            .iter()
            .zip(&offsets)
            .filter(|(v, _)| v.cascade.is_none())
            .collect();
        if !locals.is_empty() {
            let mut pass = frame.pass_to(&TargetPassDesc {
                label: "Shadows.Atlas",
                color: &[],
                depth: Some(DepthTarget {
                    view: &self.shadows.atlas_view,
                    load: LoadOp::Clear(DEPTH_CLEAR),
                    store: true,
                }),
            });
            for (view, &offset) in locals {
                let [x, y, s] = view.tile;
                pass.set_viewport(x as f32, y as f32, s as f32, s as f32);
                pass.set_scissor_rect(x, y, s, s);
                pass.set_bind_group(0, &self.shadows.view_group, &[offset]);
                self.draw_items(&mut pass, items, pick);
            }
        }
    }

    fn mesh_for(&self, kind: &MeshKind) -> Option<&GpuMesh> {
        match kind {
            MeshKind::Cube => Some(&self.cube),
            MeshKind::Plane => Some(&self.plane),
            MeshKind::Primitive(p) => self.primitives.get(p).map(|c| &c.mesh),
            // Not uploaded yet (still loading): skipped this frame.
            MeshKind::Asset(id) => self.assets.mesh(*id),
        }
    }

    /// Draws every item `pick` returns a pipeline for, in list order.
    fn draw_items<'p>(
        &'p self,
        pass: &mut RenderPass<'_>,
        items: &[DrawItem],
        pick: impl Fn(&DrawItem) -> Option<&'p RenderPipeline>,
    ) {
        let order: Vec<usize> = (0..items.len()).collect();
        self.draw_indexed_subset(pass, items, &order, pick);
    }

    fn draw_indexed_subset<'p>(
        &'p self,
        pass: &mut RenderPass<'_>,
        items: &[DrawItem],
        order: &[usize],
        pick: impl Fn(&DrawItem) -> Option<&'p RenderPipeline>,
    ) {
        let mut current_pipeline: Option<*const RenderPipeline> = None;
        let mut current_material: Option<Option<AssetId<MaterialAsset>>> = None;
        for &i in order {
            let item = &items[i];
            let Some(pipeline) = pick(item) else { continue };
            let Some(mesh) = self.mesh_for(&item.mesh) else {
                continue;
            };
            if current_pipeline != Some(pipeline as *const _) {
                pass.set_pipeline(pipeline);
                current_pipeline = Some(pipeline as *const _);
            }
            if current_material != Some(item.material_asset) {
                pass.set_bind_group(2, self.assets.material_group(item.material_asset), &[]);
                current_material = Some(item.material_asset);
            }
            let offset = (i as u64 * self.object_stride) as u32;
            pass.set_bind_group(1, &self.object_group, &[offset]);
            pass.set_vertex_buffer(0, &mesh.vertex_buffer);
            pass.set_index_buffer(&mesh.index_buffer);
            pass.draw_indexed(0..mesh.index_count, 0, 0..1);
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
                material: [
                    item.metallic,
                    item.roughness,
                    item.normal_strength,
                    item.cutoff(),
                ],
                emission: item.emission.extend(item.f0).to_array(),
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
        assert_eq!(size_of::<ObjectUniform>(), 176);
    }
}
