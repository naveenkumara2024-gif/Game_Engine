//! Gameplay-facing render components and the **extract** step (law 2, lore §6.7).
//!
//! Gameplay only ever touches components. Once per frame, in the `Extract` tick group,
//! [`extract_render_snapshot`] copies what the renderer needs into a plain-data
//! [`RenderSnapshot`]. That is the single sync point between game and render state; in M5 the
//! snapshot moves to the render thread unchanged.

use ge_asset::{AssetEventKind, AssetId, AssetServer, MaterialAsset};
use ge_ecs::prelude::*;

use crate::assets::AssetUpload;
use crate::lighting::{
    ColorManagement, Light, LightItem, MAX_LIGHTS, Material, R_DIRECT_LIGHT_INTENSITY, R_LOOK,
    R_VIEWPORT_SHADING, RenderMethod, RenderWorld, ViewportShading, WorldSettings, dielectric_f0,
};
use crate::post::BloomSettings;
use crate::renderer::DrawAlpha;
use crate::ui::UiDrawList;
use crate::{Camera, DrawItem, MeshKind, Overlays};

/// Draws a mesh at the entity's `GlobalTransform`.
#[derive(Component, Copy, Clone, Debug, PartialEq)]
#[require(Transform)]
pub struct MeshRenderer {
    pub mesh: MeshKind,
    /// Linear RGB multiplier.
    pub tint: Vec4,
    /// A material asset (glTF). An inline [`Material`] component on the same entity wins.
    pub material: Option<AssetId<MaterialAsset>>,
    /// Object ▸ Visibility ▸ Shadow: whether this object casts shadows.
    pub cast_shadow: bool,
}

impl MeshRenderer {
    pub fn new(mesh: MeshKind, tint: [f32; 3]) -> Self {
        Self {
            mesh,
            tint: Vec3::from(tint).extend(1.0),
            material: None,
            cast_shadow: true,
        }
    }
}

/// Hides an entity's mesh or light without despawning it (Blender's "Show in Viewport").
/// Entities without the component are visible.
#[derive(Component, Copy, Clone, Debug, PartialEq, Eq)]
pub struct Visibility {
    pub visible: bool,
}

impl Default for Visibility {
    fn default() -> Self {
        Self { visible: true }
    }
}

/// A perspective camera looking down the entity's local -Z. The camera with the highest
/// `priority` is used, unless [`CameraOverride`] names one.
#[derive(Component, Copy, Clone, Debug, PartialEq)]
#[require(Transform)]
pub struct Camera3d {
    /// Vertical field of view in radians.
    pub fov_y: f32,
    pub near: f32,
    pub priority: i32,
}

impl Default for Camera3d {
    fn default() -> Self {
        let d = Camera::default();
        Self {
            fov_y: d.fov_y,
            near: d.near,
            priority: 0,
        }
    }
}

/// Forces the view camera (the editor's viewport camera while editing).
#[derive(Resource, Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct CameraOverride(pub Option<Entity>);

/// Overlays requested for the next frame (the editor turns them on; games leave them off).
#[derive(Resource, Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct ViewportOverlays(pub Overlays);

/// UI to draw on top of the scene. A UI producer (the editor) replaces `meshes` each frame and
/// **appends** to `textures`; extract moves both into the snapshot.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct UiOutput(pub UiDrawList);

/// Plain-data copy of everything the renderer needs for one frame.
#[derive(Resource, Clone, Debug, Default)]
pub struct RenderSnapshot {
    pub camera: Option<Camera>,
    pub time: f32,
    pub items: Vec<DrawItem>,
    pub lights: Vec<LightItem>,
    /// `None` when the app has no `WorldSettings` (the renderer uses `r.ClearColor`).
    pub world: Option<RenderWorld>,
    pub color: ColorManagement,
    pub look: i32,
    pub bloom: BloomSettings,
    pub shading: ViewportShading,
    pub direct_light: f32,
    pub overlays: Overlays,
    /// Texture updates queue up here until a frame is actually rendered (see [`UiDrawList`]).
    pub ui: UiDrawList,
    /// Asset changes for the GPU; like UI textures, they queue until a frame renders.
    pub asset_uploads: Vec<AssetUpload>,
}

#[allow(clippy::too_many_arguments)]
pub fn extract_render_snapshot(
    mut snapshot: ResMut<RenderSnapshot>,
    time: Res<Time>,
    world: Option<Res<WorldSettings>>,
    camera_override: Option<Res<CameraOverride>>,
    overlays: Option<Res<ViewportOverlays>>,
    ui: Option<ResMut<UiOutput>>,
    assets: Option<ResMut<AssetServer>>,
    cameras: Query<(Entity, &GlobalTransform, &Camera3d, Option<&Visibility>)>,
    meshes: Query<(
        &GlobalTransform,
        &MeshRenderer,
        Option<&Material>,
        Option<&Visibility>,
    )>,
    lights: Query<(&GlobalTransform, &Light, Option<&Visibility>)>,
) {
    let snapshot = &mut *snapshot;
    let visible = |v: Option<&Visibility>| v.is_none_or(|v| v.visible);
    snapshot.time = time.elapsed_secs();

    // Asset changes since the last extract → one-time uploads (law 2: no per-frame copies).
    let assets = assets.map(|mut server| {
        for event in server.drain_render_changes() {
            let upload = match event.kind {
                AssetEventKind::Removed => Some(AssetUpload::Free(event.id)),
                AssetEventKind::Loaded | AssetEventKind::Modified => {
                    if let Some(id) = event.id.typed() {
                        server.get(id).map(|m| AssetUpload::Mesh(id, m.clone()))
                    } else if let Some(id) = event.id.typed() {
                        server.get(id).map(|t| AssetUpload::Texture(id, t.clone()))
                    } else if let Some(id) = event.id.typed() {
                        server.get(id).map(|m| AssetUpload::Material(id, m.clone()))
                    } else {
                        None
                    }
                }
            };
            snapshot.asset_uploads.extend(upload);
        }
        server
    });

    let forced = camera_override
        .and_then(|o| o.0)
        .and_then(|e| cameras.get(e).ok());
    let chosen = forced.or_else(|| {
        cameras
            .iter()
            .filter(|(.., v)| visible(*v))
            .max_by_key(|(_, _, cam, _)| cam.priority)
    });
    snapshot.camera = chosen.map(|(_, global, cam, _)| {
        let position = global.translation();
        Camera {
            position,
            target: position + global.forward(),
            up: global.up(),
            fov_y: cam.fov_y,
            near: cam.near,
        }
    });

    snapshot.items.clear();
    snapshot
        .items
        .extend(
            meshes
                .iter()
                .filter(|(.., v)| visible(*v))
                .map(|(global, mesh, material, _)| {
                    let mut item = DrawItem::new(mesh.mesh, global.to_mat4(), mesh.tint);
                    item.cast_shadow = mesh.cast_shadow;
                    // Material asset first (factors + textures); an inline Material overrides.
                    if material.is_none()
                        && let (Some(id), Some(server)) = (mesh.material, assets.as_deref())
                        && let Some(m) = server.get(id)
                    {
                        let [r, g, b, _] = m.base_color;
                        item.tint = (mesh.tint.truncate() * Vec3::new(r, g, b)).extend(mesh.tint.w);
                        item.metallic = m.metallic;
                        item.roughness = m.roughness;
                        item.emission = Vec3::from(m.emissive);
                        item.double_sided = m.double_sided;
                        item.material_asset = Some(id);
                        item.tint.w = mesh.tint.w * m.base_color[3];
                        item.normal_strength = m.normal_scale;
                        item.alpha = match m.alpha_mode {
                            ge_asset::AlphaMode::Opaque => DrawAlpha::Opaque,
                            ge_asset::AlphaMode::Mask => DrawAlpha::Mask(m.alpha_cutoff),
                            ge_asset::AlphaMode::Blend => DrawAlpha::Blend,
                        };
                    }
                    if let Some(m) = material {
                        // Inline factors win; the asset's textures (if any) still apply.
                        item.material_asset = mesh.material;
                        item.tint = (mesh.tint.truncate() * m.base_color).extend(mesh.tint.w);
                        item.metallic = m.metallic;
                        item.roughness = m.roughness;
                        item.emission = m.emission_color * m.emission_strength.max(0.0);
                        item.double_sided = !m.backface_culling;
                        item.tint.w = mesh.tint.w * m.alpha.clamp(0.0, 1.0);
                        item.f0 = dielectric_f0(m.ior, m.specular_ior_level);
                        item.shadow_double_sided = !m.backface_culling_shadow;
                        item.transparent_shadows = m.transparent_shadows;
                        // Blender's "Dithered" needs TAA (M12); a 0.5 cutout stands in for it.
                        item.alpha = match m.render_method {
                            RenderMethod::Blended if m.alpha < 1.0 => DrawAlpha::Blend,
                            RenderMethod::Dithered if m.alpha < 1.0 => DrawAlpha::Mask(0.5),
                            _ => item.alpha,
                        };
                    }
                    item
                }),
        );

    snapshot.lights.clear();
    snapshot.lights.extend(
        lights
            .iter()
            .filter(|(.., v)| visible(*v))
            .take(MAX_LIGHTS)
            .map(|(global, light, _)| LightItem::from_light(light, global)),
    );

    snapshot.world = world.map(|w| w.to_render());
    snapshot.color = ColorManagement::from_cvars();
    snapshot.look = R_LOOK.get();
    snapshot.bloom = BloomSettings::from_cvars();
    snapshot.shading = match R_VIEWPORT_SHADING.get() {
        1 => ViewportShading::Rendered,
        _ => ViewportShading::Solid,
    };
    snapshot.direct_light = R_DIRECT_LIGHT_INTENSITY.get().max(0.0);
    snapshot.overlays = overlays.map(|o| o.0).unwrap_or_default();
    if let Some(mut ui) = ui {
        let ui = &mut ui.0;
        snapshot.ui.meshes = std::mem::take(&mut ui.meshes);
        snapshot.ui.textures.append(&mut ui.textures);
    } else {
        snapshot.ui.meshes.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lighting::LightKind;
    use ge_ecs::propagate_transforms;
    use ge_ecs::schedule::Schedule;

    fn run(world: &mut World) {
        let mut schedule = Schedule::default();
        schedule.add_systems((propagate_transforms, extract_render_snapshot).chain());
        schedule.run(world);
    }

    fn world() -> World {
        let mut world = World::new();
        world.init_resource::<RenderSnapshot>();
        world.init_resource::<Time>();
        world
    }

    #[test]
    fn extract_copies_cameras_and_meshes() {
        let mut world = world();
        world.spawn((
            Transform::from_xyz(0.0, 2.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
            Camera3d::default(),
        ));
        let parent = world
            .spawn((
                Transform::from_xyz(1.0, 0.0, 0.0),
                MeshRenderer::new(MeshKind::Cube, [1.0; 3]),
            ))
            .id();
        world.spawn((
            Transform::from_xyz(0.0, 1.0, 0.0),
            MeshRenderer::new(MeshKind::Plane, [0.5; 3]),
            ChildOf(parent),
        ));
        run(&mut world);

        let snap = world.resource::<RenderSnapshot>();
        let cam = snap.camera.expect("camera extracted");
        assert!(
            (cam.target - cam.position)
                .normalize()
                .abs_diff_eq((Vec3::ZERO - Vec3::new(0.0, 2.0, 5.0)).normalize(), 1e-5)
        );
        assert_eq!(snap.items.len(), 2);
        let child = snap
            .items
            .iter()
            .find(|i| i.mesh == MeshKind::Plane)
            .unwrap();
        assert_eq!(
            child.world_from_local.w_axis.truncate(),
            Vec3::new(1.0, 1.0, 0.0),
            "child inherits the parent's transform"
        );
    }

    #[test]
    fn hidden_entities_materials_lights_and_camera_choice() {
        let mut world = world();
        let low = world
            .spawn((Transform::from_xyz(0.0, 0.0, 1.0), Camera3d::default()))
            .id();
        world.spawn((
            Transform::from_xyz(0.0, 0.0, 9.0),
            Camera3d {
                priority: 5,
                ..Default::default()
            },
        ));
        world.spawn((
            MeshRenderer::new(MeshKind::Cube, [0.5; 3]),
            Material {
                base_color: Vec3::new(1.0, 0.0, 0.0),
                metallic: 1.0,
                ..Default::default()
            },
        ));
        world.spawn((
            MeshRenderer::new(MeshKind::Cube, [1.0; 3]),
            Visibility { visible: false },
        ));
        world.spawn((
            Transform::from_xyz(0.0, 3.0, 0.0),
            Light::new(LightKind::Spot),
        ));
        world.spawn((Light::new(LightKind::Sun), Visibility { visible: false }));
        run(&mut world);

        let snap = world.resource::<RenderSnapshot>();
        assert_eq!(
            snap.camera.unwrap().position.z,
            9.0,
            "highest priority wins"
        );
        assert_eq!(snap.items.len(), 1, "hidden mesh skipped");
        let item = snap.items[0];
        assert_eq!(item.tint.truncate(), Vec3::new(0.5, 0.0, 0.0));
        assert_eq!(item.metallic, 1.0);
        assert!(item.double_sided, "Blender default: no backface culling");
        assert_eq!(snap.lights.len(), 1, "hidden light skipped");
        assert_eq!(snap.lights[0].position, Vec3::new(0.0, 3.0, 0.0));

        world.insert_resource(CameraOverride(Some(low)));
        run(&mut world);
        let snap = world.resource::<RenderSnapshot>();
        assert_eq!(snap.camera.unwrap().position.z, 1.0, "override wins");
    }

    #[test]
    fn ui_texture_updates_accumulate_until_drained() {
        use crate::ui::{UiTextureId, UiTextureUpdate};
        let mut world = world();
        world.init_resource::<UiOutput>();
        for i in 0..2 {
            world
                .resource_mut::<UiOutput>()
                .0
                .textures
                .push(UiTextureUpdate::Free(UiTextureId(i)));
            run(&mut world);
        }
        assert_eq!(world.resource::<RenderSnapshot>().ui.textures.len(), 2);
        assert!(world.resource::<UiOutput>().0.textures.is_empty());
    }
}
