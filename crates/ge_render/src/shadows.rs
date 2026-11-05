//! Shadows (plan 0006, decision 3): CPU-side planning (cascade splits, stabilized cascade
//! frusta, atlas tile assignment) and the GPU resources the shadow passes render into and the
//! lighting shader samples. Reversed-Z throughout (lit ⇔ `ref ≥ stored`).

use bytemuck::{Pod, Zeroable};
use ge_rhi::{
    AddressMode, BindGroup, BindGroupEntry, BindGroupLayout, BindGroupLayoutEntry, BindingResource,
    BindingType, Buffer, BufferDesc, BufferUsage, CompareFunction, FilterMode, Rhi, Sampler,
    SamplerDesc, ShaderStages, Texture, TextureDescEx, TextureDim, TextureFormat, TextureUsage,
    TextureView, ViewRange,
};
use glam::camera::rh::{proj, view};
use glam::{Mat4, Vec3, Vec4};

use crate::Camera;
use crate::lighting::{LightItem, LightKind};

ge_core::declare_cvar!(pub static R_SHADOWS: bool = (
    "r.Shadows",
    true,
    "Render shadows (Render ▸ Sampling ▸ Shadows)"
));
ge_core::declare_cvar!(pub static R_SHADOW_RESOLUTION: f32 = (
    "r.Shadow.Resolution",
    1.0,
    "Scale of every shadow map's resolution (Render ▸ Sampling ▸ Shadows ▸ Resolution)",
    ge_core::CVarFlags::SCALABILITY,
));
ge_core::declare_cvar!(pub static R_SHADOW_POOL: i32 = (
    "r.Shadow.Pool",
    64,
    "Shadow atlas memory in MB for spot/point/area lights (Render ▸ Performance ▸ Shadow Pool)",
    ge_core::CVarFlags::SCALABILITY,
));
ge_core::declare_cvar!(pub static R_SHADOW_CSM_CASCADES: i32 = (
    "r.Shadow.CSM.Cascades",
    4,
    "Sun shadow cascades (1-4)",
    ge_core::CVarFlags::SCALABILITY,
));
ge_core::declare_cvar!(pub static R_SHADOW_CSM_DISTANCE: f32 = (
    "r.Shadow.CSM.Distance",
    80.0,
    "How far from the camera the sun casts shadows (m)"
));
ge_core::declare_cvar!(pub static R_SHADOW_CSM_LAMBDA: f32 = (
    "r.Shadow.CSM.Lambda",
    0.75,
    "Cascade split blend: 0 = uniform, 1 = logarithmic"
));
ge_core::declare_cvar!(pub static R_SHADOW_MAX_LOCAL: i32 = (
    "r.Shadow.MaxLocal",
    8,
    "Shadowed spot/point/area lights per frame (nearest first)",
    ge_core::CVarFlags::SCALABILITY,
));

pub(crate) const MAX_CASCADES: usize = 4;
const CSM_BASE_RESOLUTION: u32 = 2048;
/// Atlas tiles per side (a point light takes six).
const ATLAS_GRID: u32 = 8;
pub(crate) const MAX_LOCAL_VIEWS: usize = (ATLAS_GRID * ATLAS_GRID) as usize;
/// How far behind the camera frustum casters are still captured (m).
const CASTER_EXTENSION: f32 = 100.0;

/// Matches `Cascades` in `shaders/common/shadows.wesl`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, Pod, Zeroable)]
pub(crate) struct CascadesUniform {
    clip_from_world: [[[f32; 4]; 4]; MAX_CASCADES],
    splits: [f32; 4],
    texel_world: [f32; 4],
    params: [f32; 4],
}

/// Matches `LocalShadow` in `shaders/common/shadows.wesl`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, Pod, Zeroable)]
pub(crate) struct LocalShadowUniform {
    clip_from_world: [[f32; 4]; 4],
    rect: [f32; 4],
    params: [f32; 4],
}

/// One shadow map view to render: a cascade layer or an atlas tile.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct ShadowView {
    pub clip_from_world: Mat4,
    /// `None` = cascade layer `n`, `Some` = atlas tile `[x, y, size]` in pixels.
    pub cascade: Option<usize>,
    pub tile: [u32; 3],
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ShadowPlan {
    pub views: Vec<ShadowView>,
    /// Per light (same order as the frame's light list): its shadow slot.
    pub slots: Vec<Option<u32>>,
    pub cascades: CascadesUniform,
    pub locals: Vec<LocalShadowUniform>,
}

/// Practical split scheme (Zhang et al.): blend of uniform and logarithmic splits.
pub(crate) fn cascade_splits(near: f32, far: f32, count: usize, lambda: f32) -> Vec<f32> {
    (1..=count)
        .map(|i| {
            let t = i as f32 / count as f32;
            let log = near * (far / near).powf(t);
            let uniform = near + (far - near) * t;
            lambda * log + (1.0 - lambda) * uniform
        })
        .collect()
}

/// Reversed-Z orthographic projection: depth 1 at `near`, 0 at `far`.
fn ortho_reversed(r: f32, near: f32, far: f32) -> Mat4 {
    let flip = Mat4::from_cols(
        Vec4::X,
        Vec4::Y,
        Vec4::new(0.0, 0.0, -1.0, 0.0),
        Vec4::new(0.0, 0.0, 1.0, 1.0),
    );
    flip * proj::directx::orthographic(-r, r, -r, r, near, far)
}

/// A stable cascade: bounding sphere of the camera sub-frustum `[d0, d1]`, light-space
/// orthographic projection, center snapped to whole shadow texels so the map doesn't swim as
/// the camera moves. Returns `(clip_from_world, texel world size)`.
pub(crate) fn cascade_matrix(
    camera: &Camera,
    aspect: f32,
    d0: f32,
    d1: f32,
    light_dir: Vec3,
    resolution: u32,
) -> (Mat4, f32) {
    let world_from_view = camera.view_from_world().inverse();
    let tan_y = (camera.fov_y * 0.5).tan();
    let tan_x = tan_y * aspect;
    let mut corners = [Vec3::ZERO; 8];
    for (i, c) in corners.iter_mut().enumerate() {
        let d = if i < 4 { d0 } else { d1 };
        let sx = if i & 1 == 0 { -1.0 } else { 1.0 };
        let sy = if i & 2 == 0 { -1.0 } else { 1.0 };
        *c = world_from_view.transform_point3(Vec3::new(sx * tan_x * d, sy * tan_y * d, -d));
    }
    let center = corners.iter().copied().sum::<Vec3>() / 8.0;
    let radius = corners
        .iter()
        .map(|c| c.distance(center))
        .fold(0.0, f32::max);
    // Quantize the radius so the projection size doesn't flicker with tiny camera changes.
    let radius = (radius * 16.0).ceil() / 16.0;

    let dir = light_dir.normalize_or(Vec3::NEG_Y);
    let up = if dir.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
    let eye = center - dir * (radius + CASTER_EXTENSION);
    let light_view = view::look_at_mat4(eye, center, up);
    let projection = ortho_reversed(radius, 0.0, 2.0 * radius + CASTER_EXTENSION);
    let mut clip_from_world = projection * light_view;

    // Snap the world origin's projection to the texel grid.
    let texel = 2.0 / resolution as f32;
    let origin = clip_from_world * Vec4::new(0.0, 0.0, 0.0, 1.0);
    let snapped_x = (origin.x / texel).round() * texel;
    let snapped_y = (origin.y / texel).round() * texel;
    let offset = Mat4::from_translation(Vec3::new(snapped_x - origin.x, snapped_y - origin.y, 0.0));
    clip_from_world = offset * clip_from_world;
    (clip_from_world, 2.0 * radius / resolution as f32)
}

/// Reversed-Z infinite perspective for local shadow views.
fn local_projection(fov: f32) -> Mat4 {
    proj::directx::perspective_infinite_reverse(fov, 1.0, 0.05)
}

fn look(eye: Vec3, dir: Vec3) -> Mat4 {
    let up = if dir.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
    view::look_to_mat4(eye, dir, up)
}

/// Cube face directions in the order the shader picks them (+X −X +Y −Y +Z −Z).
pub(crate) const CUBE_FACES: [Vec3; 6] = [
    Vec3::X,
    Vec3::NEG_X,
    Vec3::Y,
    Vec3::NEG_Y,
    Vec3::Z,
    Vec3::NEG_Z,
];

/// Plans this frame's shadow views and each light's slot.
pub(crate) fn plan(
    lights: &[LightItem],
    camera: &Camera,
    aspect: f32,
    csm_resolution: u32,
    atlas_resolution: u32,
) -> ShadowPlan {
    let mut plan = ShadowPlan {
        slots: vec![None; lights.len()],
        ..Default::default()
    };

    // Sun: the first shadow-casting sun gets the cascades.
    if let Some((i, sun)) = lights
        .iter()
        .enumerate()
        .find(|(_, l)| l.kind == LightKind::Sun && l.cast_shadow)
    {
        let count = (R_SHADOW_CSM_CASCADES.get().clamp(1, MAX_CASCADES as i32)) as usize;
        let far = R_SHADOW_CSM_DISTANCE.get().max(camera.near * 2.0);
        let splits = cascade_splits(
            camera.near,
            far,
            count,
            R_SHADOW_CSM_LAMBDA.get().clamp(0.0, 1.0),
        );
        let mut d0 = camera.near;
        for (c, &d1) in splits.iter().enumerate() {
            let (m, texel) = cascade_matrix(camera, aspect, d0, d1, sun.direction, csm_resolution);
            plan.cascades.clip_from_world[c] = m.to_cols_array_2d();
            plan.cascades.splits[c] = d1;
            plan.cascades.texel_world[c] = texel;
            plan.views.push(ShadowView {
                clip_from_world: m,
                cascade: Some(c),
                tile: [0; 3],
            });
            d0 = d1;
        }
        plan.cascades.params = [
            count as f32,
            1.0 / csm_resolution as f32,
            0.1,
            sun.shadow_filter * 1.5 + sun.sun_angular_radius * 300.0,
        ];
        plan.slots[i] = Some(0);
    }

    // Local lights: nearest shadow casters first, as long as atlas tiles last.
    let mut locals: Vec<usize> = (0..lights.len())
        .filter(|&i| lights[i].kind != LightKind::Sun && lights[i].cast_shadow)
        .collect();
    locals.sort_by(|&a, &b| {
        let da = (lights[a].position - camera.position).length() - lights[a].range;
        let db = (lights[b].position - camera.position).length() - lights[b].range;
        da.total_cmp(&db)
    });
    locals.truncate(R_SHADOW_MAX_LOCAL.get().max(0) as usize);
    let tile = (atlas_resolution / ATLAS_GRID).max(16);
    let mut next = 0u32;
    for i in locals {
        let light = &lights[i];
        let faces: Vec<(Vec3, f32)> = match light.kind {
            LightKind::Point => CUBE_FACES
                .iter()
                .map(|&d| (d, 95f32.to_radians()))
                .collect(),
            LightKind::Spot => vec![(
                light.direction,
                (light.spot_size + 0.2).min(170f32.to_radians()),
            )],
            _ => vec![(light.direction, 160f32.to_radians())],
        };
        if next as usize + faces.len() > MAX_LOCAL_VIEWS {
            break;
        }
        plan.slots[i] = Some(next);
        for (dir, fov) in faces {
            let m = local_projection(fov) * look(light.position, dir);
            let (tx, ty) = (next % ATLAS_GRID, next / ATLAS_GRID);
            let px = [tx * tile, ty * tile, tile];
            let inv = 1.0 / atlas_resolution as f32;
            plan.locals.push(LocalShadowUniform {
                clip_from_world: m.to_cols_array_2d(),
                rect: [
                    px[0] as f32 * inv,
                    px[1] as f32 * inv,
                    tile as f32 * inv,
                    tile as f32 * inv,
                ],
                params: [
                    2.0 * (fov * 0.5).tan() / tile as f32,
                    light.shadow_filter * 1.5 + light.radius * 6.0,
                    inv,
                    0.0,
                ],
            });
            plan.views.push(ShadowView {
                clip_from_world: m,
                cascade: None,
                tile: px,
            });
            next += 1;
        }
    }
    plan
}

/// GPU side: the maps, the comparison sampler and the per-view uniforms.
pub(crate) struct ShadowMaps {
    pub csm: Texture,
    pub csm_layers: Vec<TextureView>,
    pub csm_resolution: u32,
    pub atlas: Texture,
    pub atlas_view: TextureView,
    pub atlas_resolution: u32,
    pub sampler: Sampler,
    pub data: Buffer,
    pub view_layout: BindGroupLayout,
    pub view_buffer: Buffer,
    pub view_group: BindGroup,
    pub view_stride: u64,
}

impl std::fmt::Debug for ShadowMaps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShadowMaps")
            .field("csm", &self.csm_resolution)
            .field("atlas", &self.atlas_resolution)
            .finish_non_exhaustive()
    }
}

/// Atlas side length from the Shadow Pool size (Depth32: 4 bytes per texel).
pub(crate) fn atlas_resolution_for_pool(mb: i32) -> u32 {
    let texels = (mb.clamp(4, 1024) as u64 * 1024 * 1024) / 4;
    let side = (texels as f64).sqrt() as u32;
    // Round down to a power of two, at least 1024.
    (1u32 << (31 - side.max(1).leading_zeros())).clamp(1024, 8192)
}

impl ShadowMaps {
    pub(crate) fn new(rhi: &Rhi, csm_resolution: u32, atlas_resolution: u32) -> Self {
        let depth = |label, size, layers, dim| {
            rhi.create_texture_ex(&TextureDescEx {
                label,
                size: (size, size),
                format: TextureFormat::Depth32Float,
                usage: TextureUsage::RENDER_ATTACHMENT | TextureUsage::TEXTURE_BINDING,
                mip_levels: 1,
                layers,
                dim,
            })
        };
        let csm = depth(
            "ShadowCascades",
            csm_resolution,
            MAX_CASCADES as u32,
            TextureDim::D2Array,
        );
        let csm_layers = (0..MAX_CASCADES as u32)
            .map(|l| csm.view(ViewRange::level(0, l)))
            .collect();
        let atlas = depth("ShadowAtlas", atlas_resolution, 1, TextureDim::D2);
        let atlas_view = atlas.full_view();
        let sampler = rhi.create_sampler(&SamplerDesc {
            label: "ShadowCompare",
            filter: FilterMode::Linear,
            address: AddressMode::ClampToEdge,
            mipmaps: false,
            anisotropy: 1,
            compare: Some(CompareFunction::GreaterEqual),
        });
        let data = rhi.create_buffer(&BufferDesc {
            label: "ShadowData",
            size: (size_of::<CascadesUniform>() + size_of::<LocalShadowUniform>() * MAX_LOCAL_VIEWS)
                as u64,
            usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
        });
        let view_stride = 64u64.next_multiple_of(rhi.uniform_offset_alignment());
        let view_layout = rhi.create_bind_group_layout(
            "ShadowView",
            &[BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::Vertex,
                ty: BindingType::UniformBuffer {
                    has_dynamic_offset: true,
                    min_binding_size: 64,
                },
            }],
        );
        let view_buffer = rhi.create_buffer(&BufferDesc {
            label: "ShadowViews",
            size: view_stride * (MAX_CASCADES + MAX_LOCAL_VIEWS) as u64,
            usage: BufferUsage::UNIFORM | BufferUsage::COPY_DST,
        });
        let view_group = rhi.create_bind_group(
            "ShadowViews",
            &view_layout,
            &[BindGroupEntry {
                binding: 0,
                resource: BindingResource::Buffer {
                    buffer: &view_buffer,
                    offset: 0,
                    size: Some(64),
                },
            }],
        );
        Self {
            csm,
            csm_layers,
            csm_resolution,
            atlas,
            atlas_view,
            atlas_resolution,
            sampler,
            data,
            view_layout,
            view_buffer,
            view_group,
            view_stride,
        }
    }

    /// Uploads the plan's matrices; returns each view's dynamic offset.
    pub(crate) fn upload(&self, rhi: &Rhi, plan: &ShadowPlan) -> Vec<u32> {
        let mut data = bytemuck::bytes_of(&plan.cascades).to_vec();
        data.extend_from_slice(bytemuck::cast_slice(&plan.locals));
        if plan.locals.is_empty() {
            data.extend_from_slice(bytemuck::bytes_of(&LocalShadowUniform::default()));
        }
        rhi.write_buffer(&self.data, 0, &data);
        let stride = self.view_stride as usize;
        let mut views = vec![0u8; stride * plan.views.len().max(1)];
        let mut offsets = Vec::with_capacity(plan.views.len());
        for (i, v) in plan.views.iter().enumerate() {
            let m = v.clip_from_world.to_cols_array();
            views[i * stride..i * stride + 64].copy_from_slice(bytemuck::cast_slice(&m));
            offsets.push((i * stride) as u32);
        }
        rhi.write_buffer(&self.view_buffer, 0, &views);
        offsets
    }
}

/// Resolution of the cascade maps after `r.Shadow.Resolution`.
pub(crate) fn csm_resolution() -> u32 {
    ((CSM_BASE_RESOLUTION as f32 * R_SHADOW_RESOLUTION.get().clamp(0.125, 2.0)) as u32)
        .next_power_of_two()
        .clamp(256, 4096)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_are_increasing_and_end_at_far() {
        let s = cascade_splits(0.1, 80.0, 4, 0.75);
        assert_eq!(s.len(), 4);
        assert!(s.windows(2).all(|w| w[1] > w[0]));
        assert!((s[3] - 80.0).abs() < 1e-3);
        // Logarithmic weighting packs more resolution near the camera than uniform splits.
        assert!(s[0] < 80.0 / 4.0);
    }

    #[test]
    fn cascade_contains_its_frustum_and_is_texel_stable() {
        let mut cam = Camera {
            position: Vec3::new(0.0, 5.0, 10.0),
            target: Vec3::ZERO,
            ..Default::default()
        };
        let sun = Vec3::new(-0.3, -1.0, -0.4).normalize();
        let (m, texel) = cascade_matrix(&cam, 16.0 / 9.0, 0.1, 10.0, sun, 2048);
        assert!(texel > 0.0);
        // A point inside the sub-frustum projects inside the map with valid reversed depth.
        let p = cam.position + (cam.target - cam.position).normalize() * 5.0;
        let c = m * p.extend(1.0);
        assert!(c.x.abs() <= 1.0 && c.y.abs() <= 1.0, "{c}");
        assert!((0.0..=1.0).contains(&c.z), "{c}");
        // Moving the camera by a fraction of a texel moves the projection by whole texels only.
        cam.position.x += texel * 0.3;
        cam.target.x += texel * 0.3;
        let (m2, _) = cascade_matrix(&cam, 16.0 / 9.0, 0.1, 10.0, sun, 2048);
        let o1 = m * Vec4::W;
        let o2 = m2 * Vec4::W;
        let step = 2.0 / 2048.0;
        let dx = (o2.x - o1.x) / step;
        assert!((dx - dx.round()).abs() < 1e-3, "origin moved {dx} texels");
    }

    #[test]
    fn closer_to_light_means_larger_depth() {
        let cam = Camera::default();
        let sun = Vec3::NEG_Y;
        let (m, _) = cascade_matrix(&cam, 1.0, 0.1, 20.0, sun, 1024);
        let high = (m * Vec4::new(0.0, 5.0, 0.0, 1.0)).z;
        let low = (m * Vec4::new(0.0, 0.0, 0.0, 1.0)).z;
        assert!(
            high > low,
            "reversed-Z: the higher point is closer to the sun"
        );
    }

    #[test]
    fn local_lights_get_slots_until_the_atlas_is_full() {
        let mut point = LightItem::from_light(
            &crate::Light::new(LightKind::Point),
            &ge_ecs::GlobalTransform::default(),
        );
        point.cast_shadow = true;
        let lights = vec![point; 12];
        let plan = plan(&lights, &Camera::default(), 1.0, 1024, 4096);
        let shadowed = plan.slots.iter().filter(|s| s.is_some()).count();
        assert_eq!(shadowed, 8, "r.Shadow.MaxLocal");
        assert_eq!(plan.locals.len(), 8 * 6, "six faces each");
        assert_eq!(plan.slots[0], Some(0));
    }

    #[test]
    fn pool_to_atlas_size() {
        assert_eq!(atlas_resolution_for_pool(64), 4096);
        assert_eq!(atlas_resolution_for_pool(16), 2048);
        assert_eq!(atlas_resolution_for_pool(1), 1024);
    }

    #[test]
    fn uniform_layouts_match_wgsl() {
        assert_eq!(size_of::<CascadesUniform>(), 304);
        assert_eq!(size_of::<LocalShadowUniform>(), 96);
    }
}
