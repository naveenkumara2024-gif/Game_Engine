//! Editor state: the `Editor` resource, editor-only components, the view camera and picking.

use ge_ecs::prelude::*;
use ge_render::{Camera, Camera3d, Light, LightKind, MeshKind, MeshRenderer};

use crate::commands::History;
use crate::egui_glue::InputTranslator;

/// Belongs to the editor, not the scene: hidden from the outliner, never picked, never saved
/// or stashed for Play.
#[derive(Component, Copy, Clone, Debug, Default)]
pub struct EditorOnly;

/// Per-object viewport display options (Object tab ▸ Viewport Display). Editor-only data.
#[derive(Component, Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct ViewportDisplay {
    pub show_name: bool,
    pub show_axes: bool,
    pub show_bounds: bool,
}

/// The editor's orbit camera (Blender-style navigation around a focus point).
#[derive(Component, Copy, Clone, Debug, PartialEq)]
pub struct EditorCamera {
    pub focus: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
}

impl Default for EditorCamera {
    fn default() -> Self {
        Self {
            focus: Vec3::new(0.0, 1.0, 0.0),
            yaw: 0.6,
            pitch: 0.45,
            distance: 24.0,
        }
    }
}

impl EditorCamera {
    pub fn eye(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        self.focus + Vec3::new(sy * cp, sp, cy * cp) * self.distance
    }

    pub fn transform(&self) -> Transform {
        Transform::from_translation(self.eye()).looking_at(self.focus, Vec3::Y)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    Move,
    Rotate,
    Scale,
}

impl Tool {
    pub fn hint(self) -> &'static str {
        match self {
            Tool::Select => {
                "Click to select · drag to orbit · Space+drag to pan · wheel to zoom · Shift+A add"
            }
            Tool::Move => {
                "Drag an arrow to move · drag empty space to orbit · Space+drag to pan · wheel zoom"
            }
            Tool::Rotate => {
                "Drag a ring to rotate · drag empty space to orbit · Space+drag to pan · Ctrl snaps"
            }
            Tool::Scale => {
                "Drag a handle to scale · drag empty space to orbit · Space+drag to pan · Ctrl snaps"
            }
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Tab {
    Render,
    World,
    Object,
    Data,
    Material,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct OverlayToggles {
    pub grid: bool,
    pub icons: bool,
    pub selection_bounds: bool,
    pub stats: bool,
}

impl Default for OverlayToggles {
    fn default() -> Self {
        Self {
            grid: true,
            icons: true,
            selection_bounds: true,
            stats: true,
        }
    }
}

/// Everything about the editor UI that isn't scene data.
#[derive(Debug)]
pub struct UiState {
    pub tool: Tool,
    pub local_orientation: bool,
    pub snap: bool,
    pub show_properties: bool,
    pub show_outliner: bool,
    pub tab: Tab,
    pub show_advanced: bool,
    pub overlays: OverlayToggles,
    pub ui_scale: f32,
    pub search: Option<String>,
    pub search_selected: usize,
    /// Screen position where the Shift+A menu opened.
    pub add_menu_at: Option<egui::Pos2>,
    pub help_open: bool,
    pub prefs_open: bool,
    pub history_open: bool,
    pub about_open: bool,
    /// A setting id the search jumped to: its row scrolls into view and flashes.
    pub focus_setting: Option<(&'static str, f64)>,
    pub rename: Option<(Entity, String)>,
    /// Transient message (Undo: Move, Play started…).
    pub toast: Option<(String, f64)>,
    /// Euler angles shown in the Object tab, kept per entity so editing one axis never
    /// re-derives (and flips) the others.
    pub euler_cache: Option<(Entity, Quat, Vec3)>,
    /// The viewport drag in progress (orbit / pan / zoom), if any.
    pub nav_drag: Option<crate::viewport::NavDrag>,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            tool: Tool::Move,
            local_orientation: false,
            snap: false,
            show_properties: true,
            show_outliner: true,
            tab: Tab::Render,
            show_advanced: false,
            overlays: OverlayToggles::default(),
            ui_scale: 1.0,
            search: None,
            search_selected: 0,
            add_menu_at: None,
            help_open: false,
            prefs_open: false,
            history_open: false,
            about_open: false,
            focus_setting: None,
            rename: None,
            toast: None,
            euler_cache: None,
            nav_drag: None,
        }
    }
}

/// The editor: egui, UI state, selection and history. One per app.
#[derive(Resource)]
pub struct Editor {
    pub ctx: egui::Context,
    pub input: InputTranslator,
    pub ui: UiState,
    pub selected: Option<Entity>,
    pub history: History,
    pub camera: Entity,
    pub gizmo: transform_gizmo_egui::Gizmo,
    pub pie: crate::play::PieState,
    pub applied_scale: f32,
    /// `ctx.input().time` of the current frame.
    pub now: f64,
    /// Smoothed CPU cost of the editor UI system (ms), shown in the stats pill.
    pub ui_ms: f32,
}

impl std::fmt::Debug for Editor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Editor")
            .field("selected", &self.selected)
            .field("history", &self.history)
            .finish_non_exhaustive()
    }
}

impl Editor {
    pub fn toast(&mut self, message: impl Into<String>) {
        self.ui.toast = Some((message.into(), self.now));
    }
}

/// What the viewport is looking through this frame, in egui points.
#[derive(Copy, Clone, Debug)]
pub struct ViewInfo {
    pub camera: Camera,
    pub screen: egui::Rect,
    pub clip_from_world: Mat4,
}

impl ViewInfo {
    pub fn new(camera: Camera, screen: egui::Rect) -> Self {
        let aspect = screen.width().max(1.0) / screen.height().max(1.0);
        Self {
            camera,
            screen,
            clip_from_world: camera.clip_from_view(aspect) * camera.view_from_world(),
        }
    }

    /// World → screen point, `None` behind the camera.
    pub fn project(&self, p: Vec3) -> Option<egui::Pos2> {
        let clip = self.clip_from_world * p.extend(1.0);
        if clip.w <= 1e-4 {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        Some(egui::pos2(
            self.screen.min.x + (ndc.x * 0.5 + 0.5) * self.screen.width(),
            self.screen.min.y + (0.5 - ndc.y * 0.5) * self.screen.height(),
        ))
    }

    /// A world-space ray through a screen point: (origin on the near plane, unit direction).
    pub fn ray(&self, pos: egui::Pos2) -> (Vec3, Vec3) {
        let ndc = glam::Vec2::new(
            (pos.x - self.screen.min.x) / self.screen.width().max(1.0) * 2.0 - 1.0,
            1.0 - (pos.y - self.screen.min.y) / self.screen.height().max(1.0) * 2.0,
        );
        let inv = self.clip_from_world.inverse();
        // Reversed-Z: 1 is the near plane, 0.5 a finite point further along.
        let near = inv.project_point3(ndc.extend(1.0));
        let far = inv.project_point3(ndc.extend(0.5));
        (near, (far - near).normalize_or(Vec3::NEG_Z))
    }

    /// World units per screen point at distance `p` (keeps overlays a constant on-screen size).
    pub fn world_per_point(&self, p: Vec3) -> f32 {
        let d = (p - self.camera.position).length().max(1e-3);
        2.0 * d * (self.camera.fov_y * 0.5).tan() / self.screen.height().max(1.0)
    }
}

pub fn camera_from(global: &GlobalTransform, cam: &Camera3d) -> Camera {
    let position = global.translation();
    Camera {
        position,
        target: position + global.forward(),
        up: global.up(),
        fov_y: cam.fov_y,
        near: cam.near,
    }
}

/// Ray vs. an oriented box: `half` extents in the local space of `world_from_local`. Returns
/// the world-space distance along the ray. Flat boxes (planes) get a little thickness.
pub fn ray_obb(origin: Vec3, dir: Vec3, world_from_local: Mat4, half: Vec3) -> Option<f32> {
    let local_from_world = world_from_local.inverse();
    let o = local_from_world.transform_point3(origin);
    let d = local_from_world.transform_vector3(dir);
    let half = half.max(Vec3::splat(0.02));
    let mut t_min = f32::NEG_INFINITY;
    let mut t_max = f32::INFINITY;
    for axis in 0..3 {
        let (o, d, h) = (o[axis], d[axis], half[axis]);
        if d.abs() < 1e-8 {
            if o.abs() > h {
                return None;
            }
            continue;
        }
        let t1 = (-h - o) / d;
        let t2 = (h - o) / d;
        t_min = t_min.max(t1.min(t2));
        t_max = t_max.min(t1.max(t2));
    }
    if t_max < t_min.max(0.0) {
        return None;
    }
    let t_local = if t_min > 0.0 { t_min } else { t_max };
    let hit = world_from_local.transform_point3(o + d * t_local);
    Some((hit - origin).length())
}

/// Picks the entity under `pos`: meshes by ray vs. their bounds, lights and cameras by their
/// on-screen icon. The nearest hit wins.
pub fn pick(world: &mut World, view: &ViewInfo, pos: egui::Pos2) -> Option<Entity> {
    let (origin, dir) = view.ray(pos);
    let mut best: Option<(f32, Entity)> = None;
    let mut consider = |t: f32, e: Entity| {
        if best.is_none_or(|(bt, _)| t < bt) {
            best = Some((t, e));
        }
    };

    let mut meshes = world.query_filtered::<(
        Entity,
        &GlobalTransform,
        &MeshRenderer,
        Option<&ge_render::Visibility>,
    ), Without<EditorOnly>>();
    for (e, global, mesh, vis) in meshes.iter(world) {
        if vis.is_some_and(|v| !v.visible) {
            continue;
        }
        if let Some(t) = ray_obb(origin, dir, global.to_mat4(), mesh_half_extents(&mesh.mesh)) {
            consider(t, e);
        }
    }

    // Icons are ~14 points wide on screen: test the cursor distance to their projection.
    let mut icons = world.query_filtered::<(Entity, &GlobalTransform, Has<Light>, Has<Camera3d>), (Without<EditorOnly>, Without<MeshRenderer>)>();
    for (e, global, is_light, is_camera) in icons.iter(world) {
        if !(is_light || is_camera) {
            continue;
        }
        let p = global.translation();
        if let Some(screen) = view.project(p)
            && screen.distance(pos) < 14.0
        {
            consider((p - origin).length(), e);
        }
    }
    best.map(|(_, e)| e)
}

pub fn mesh_half_extents(mesh: &MeshKind) -> Vec3 {
    mesh.half_extents()
}

/// A Blender-style unique name: `Cube`, `Cube.001`, `Cube.002` …
pub fn unique_name(world: &mut World, base: &str) -> String {
    let mut names = world.query::<&Name>();
    let taken: std::collections::HashSet<String> =
        names.iter(world).map(|n| n.as_str().to_owned()).collect();
    let stem = match base.rsplit_once('.') {
        Some((stem, digits)) if digits.len() == 3 && digits.chars().all(|c| c.is_ascii_digit()) => {
            stem
        }
        _ => base,
    };
    if !taken.contains(stem) {
        return stem.to_owned();
    }
    (1..)
        .map(|i| format!("{stem}.{i:03}"))
        .find(|n| !taken.contains(n))
        .unwrap_or_else(|| stem.to_owned())
}

/// The label and icon an entity gets in the outliner and properties header.
pub fn describe(world: &World, e: Entity) -> (String, &'static str) {
    use egui_phosphor::regular as icon;
    let name = world.get::<Name>(e).map(|n| n.as_str().to_owned());
    let (kind, glyph) = if let Some(light) = world.get::<Light>(e) {
        (
            light.kind.label(),
            match light.kind {
                LightKind::Sun => icon::SUN,
                LightKind::Spot => icon::FLASHLIGHT,
                LightKind::Area => icon::SQUARE,
                LightKind::Point => icon::LIGHTBULB,
            },
        )
    } else if world.get::<Camera3d>(e).is_some() {
        ("Camera", icon::VIDEO_CAMERA)
    } else if world.get::<MeshRenderer>(e).is_some() {
        ("Mesh", icon::CUBE)
    } else {
        ("Empty", icon::CROSSHAIR)
    };
    (
        name.unwrap_or_else(|| format!("{kind} {}", e.index())),
        glyph,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ray_hits_box_in_front_and_misses_beside() {
        let m = Mat4::from_translation(Vec3::new(0.0, 0.0, -5.0));
        let t = ray_obb(Vec3::ZERO, Vec3::NEG_Z, m, Vec3::splat(0.5)).unwrap();
        assert!((t - 4.5).abs() < 1e-4);
        assert!(ray_obb(Vec3::new(2.0, 0.0, 0.0), Vec3::NEG_Z, m, Vec3::splat(0.5)).is_none());
        assert!(
            ray_obb(Vec3::ZERO, Vec3::Z, m, Vec3::splat(0.5)).is_none(),
            "behind"
        );
    }

    #[test]
    fn ray_respects_rotation_and_scale() {
        // A box stretched 4× along X and rotated 90° about Y now spans Z.
        let m = Mat4::from_scale_rotation_translation(
            Vec3::new(4.0, 1.0, 1.0),
            Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            Vec3::new(3.0, 0.0, 0.0),
        );
        let hit = ray_obb(Vec3::new(3.0, 0.0, 10.0), Vec3::NEG_Z, m, Vec3::splat(0.5));
        assert!((hit.unwrap() - 8.0).abs() < 1e-3, "{hit:?}");
        let miss = ray_obb(Vec3::new(5.0, 0.0, 10.0), Vec3::NEG_Z, m, Vec3::splat(0.5));
        assert!(miss.is_none());
    }

    #[test]
    fn ray_from_inside_hits_the_far_side() {
        let t = ray_obb(Vec3::ZERO, Vec3::X, Mat4::IDENTITY, Vec3::splat(1.0)).unwrap();
        assert!((t - 1.0).abs() < 1e-5);
    }

    #[test]
    fn view_ray_round_trips_projection() {
        let camera = Camera {
            position: Vec3::new(0.0, 3.0, 8.0),
            target: Vec3::ZERO,
            ..Default::default()
        };
        let view = ViewInfo::new(
            camera,
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 900.0)),
        );
        let p = Vec3::new(1.0, 0.5, -2.0);
        let screen = view.project(p).unwrap();
        let (o, d) = view.ray(screen);
        let closest = o + d * (p - o).dot(d);
        assert!(closest.distance(p) < 1e-3, "{closest} vs {p}");
    }

    #[test]
    fn unique_names_follow_blender() {
        let mut world = World::new();
        assert_eq!(unique_name(&mut world, "Cube"), "Cube");
        world.spawn(Name::new("Cube"));
        assert_eq!(unique_name(&mut world, "Cube"), "Cube.001");
        world.spawn(Name::new("Cube.001"));
        assert_eq!(unique_name(&mut world, "Cube.001"), "Cube.002");
    }
}
