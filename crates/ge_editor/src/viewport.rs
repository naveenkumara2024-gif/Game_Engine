//! The 3D viewport behind the floating panels: editor-camera navigation, picking, the
//! transform gizmo and painter overlays (selection bounds, light / camera icons, spot cones,
//! the navigation gizmo). Drawn on egui's background layer, so every floating panel sits on top
//! and gets input first.

use egui::{Color32, Pos2, Stroke};
use egui_phosphor::regular as icon;
use ge_ecs::prelude::*;
use ge_engine::SimulationState;
use ge_render::{Camera3d, Light, LightKind, MeshRenderer, Visibility};
use transform_gizmo_egui::prelude::*;

use crate::state::{
    Editor, EditorCamera, EditorOnly, Tool, ViewInfo, ViewportDisplay, camera_from, pick,
};
use crate::theme;

/// The camera the viewport currently shows: the editor camera while editing, the game's
/// highest-priority camera while playing or paused (what extract picks).
pub fn view_info(world: &mut World, ed: &Editor, screen: egui::Rect) -> Option<ViewInfo> {
    let editing = *world.resource::<SimulationState>() == SimulationState::Editing;
    let camera = if editing {
        let cam = world.get::<Camera3d>(ed.camera)?;
        let ec = world.get::<EditorCamera>(ed.camera)?;
        let t = ec.transform();
        camera_from(&GlobalTransform(t.to_affine()), cam)
    } else {
        let mut q = world.query_filtered::<(&GlobalTransform, &Camera3d), Without<EditorOnly>>();
        let (g, c) = q.iter(world).max_by_key(|(_, c)| c.priority)?;
        camera_from(g, c)
    };
    Some(ViewInfo::new(camera, screen))
}

/// What a mouse drag in the viewport is doing. Decided when the button goes down and kept
/// until it is released, so a drag never switches meaning half-way.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NavDrag {
    Orbit,
    Pan,
    Zoom,
}

/// Mouse navigation:
/// - **Left-drag** on empty space orbits around the focus point; **Space + left-drag** pans
///   (the reference canvas' hand tool); the **wheel** zooms. A left-drag that starts on the
///   gizmo moves the object instead, and a plain click still selects.
/// - Blender's bindings work too: MMB orbit, Shift+MMB pan, Ctrl+MMB zoom.
pub fn navigate(ui: &egui::Ui, response: &egui::Response, world: &mut World, ed: &mut Editor) {
    let Some(mut cam) = world.get::<EditorCamera>(ed.camera).copied() else {
        return;
    };
    let typing = ui.ctx().egui_wants_keyboard_input();
    let (delta, mods, primary_pressed, middle_pressed, any_down, space, scroll) = ui.input(|i| {
        (
            i.pointer.delta(),
            i.modifiers,
            i.pointer.button_pressed(egui::PointerButton::Primary),
            i.pointer.button_pressed(egui::PointerButton::Middle),
            i.pointer.primary_down() || i.pointer.middle_down(),
            !typing && i.key_down(egui::Key::Space),
            i.smooth_scroll_delta.y,
        )
    });

    // A new press over the viewport (not over a panel) decides the drag kind.
    let over_viewport = response.hovered() || response.is_pointer_button_down_on();
    if over_viewport && (primary_pressed || middle_pressed) {
        ed.ui.nav_drag = if primary_pressed && ed.gizmo.is_focused() && !space {
            None // the gizmo owns this drag
        } else if space || mods.shift {
            Some(NavDrag::Pan)
        } else if mods.ctrl && middle_pressed {
            Some(NavDrag::Zoom)
        } else {
            Some(NavDrag::Orbit)
        };
    }
    if !any_down {
        ed.ui.nav_drag = None;
    }

    if let Some(kind) = ed.ui.nav_drag
        && delta != egui::Vec2::ZERO
    {
        match kind {
            NavDrag::Pan => {
                // Grab-the-scene panning: the point under the cursor follows the cursor.
                let t = cam.transform();
                let right = t.rotation * Vec3::X;
                let up = t.rotation * Vec3::Y;
                let scale = cam.distance * 0.0018;
                cam.focus += (-right * delta.x + up * delta.y) * scale;
            }
            NavDrag::Zoom => {
                cam.distance = (cam.distance * (1.0 + delta.y * 0.01)).clamp(0.2, 2000.0);
            }
            NavDrag::Orbit => {
                cam.yaw -= delta.x * 0.006;
                cam.pitch = (cam.pitch + delta.y * 0.006).clamp(-1.55, 1.55);
            }
        }
    }
    if response.hovered() && scroll != 0.0 {
        let lines = scroll / 40.0;
        cam.distance = (cam.distance * 0.88f32.powf(lines)).clamp(0.2, 2000.0);
    }
    world.entity_mut(ed.camera).insert((cam, cam.transform()));
}

/// Click-to-select (ignored when the click was a navigation drag or hit the gizmo).
pub fn click_select(
    response: &egui::Response,
    world: &mut World,
    ed: &mut Editor,
    view: &ViewInfo,
) {
    if !response.clicked() || ed.gizmo.is_focused() {
        return;
    }
    let alt = response.ctx.input(|i| i.modifiers.alt);
    if alt {
        return;
    }
    if let Some(pos) = response.interact_pointer_pos() {
        ed.selected = pick(world, view, pos);
        if ed.selected.is_none() {
            ed.ui.rename = None;
        }
    }
}

/// The transform gizmo on the selection. Writes the new local transform with merged undo.
pub fn gizmo(ui: &egui::Ui, world: &mut World, ed: &mut Editor, view: &ViewInfo) {
    let Some(e) = ed.selected else { return };
    let modes = match ed.ui.tool {
        Tool::Select => return,
        Tool::Move => GizmoMode::all_translate(),
        Tool::Rotate => GizmoMode::all_rotate(),
        Tool::Scale => GizmoMode::all_scale(),
    };
    let (Some(global), Some(local)) = (
        world.get::<GlobalTransform>(e).copied(),
        world.get::<Transform>(e).copied(),
    ) else {
        return;
    };

    // transform-gizmo unprojects with OpenGL-style depth (−1..1), so hand it a conventional
    // finite projection with the same field of view; screen positions match ours exactly.
    let aspect = view.screen.width() / view.screen.height().max(1.0);
    let proj = glam::camera::rh::proj::opengl::perspective(
        view.camera.fov_y,
        aspect,
        view.camera.near,
        10_000.0,
    )
    .as_dmat4();
    let view_m = view.camera.view_from_world().as_dmat4();
    let snapping = ed.ui.snap || ui.input(|i| i.modifiers.ctrl);
    ed.gizmo.update_config(GizmoConfig {
        view_matrix: row_matrix(view_m),
        projection_matrix: row_matrix(proj),
        viewport: Rect::from_min_max(
            Pos2::new(view.screen.min.x, view.screen.min.y),
            Pos2::new(view.screen.max.x, view.screen.max.y),
        ),
        modes,
        orientation: if ed.ui.local_orientation {
            GizmoOrientation::Local
        } else {
            GizmoOrientation::Global
        },
        snapping,
        snap_angle: 15f32.to_radians(),
        snap_distance: 0.25,
        snap_scale: 0.1,
        ..Default::default()
    });

    let (scale, rotation, translation) = global.0.to_scale_rotation_translation();
    let v3 = |v: Vec3| mint::Vector3 {
        x: f64::from(v.x),
        y: f64::from(v.y),
        z: f64::from(v.z),
    };
    let target = transform_gizmo_egui::math::Transform::from_scale_rotation_translation(
        v3(scale),
        mint::Quaternion {
            v: v3(Vec3::new(rotation.x, rotation.y, rotation.z)),
            s: f64::from(rotation.w),
        },
        v3(translation),
    );
    let Some((_, results)) = ed.gizmo.interact(ui, &[target]) else {
        return;
    };
    let Some(result) = results.first() else {
        return;
    };
    let s: [f64; 3] = result.scale.into();
    let r: [f64; 4] = [
        result.rotation.v.x,
        result.rotation.v.y,
        result.rotation.v.z,
        result.rotation.s,
    ];
    let t: [f64; 3] = result.translation.into();
    let world_new = Affine3A::from_scale_rotation_translation(
        Vec3::new(s[0] as f32, s[1] as f32, s[2] as f32),
        Quat::from_xyzw(r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32).normalize(),
        Vec3::new(t[0] as f32, t[1] as f32, t[2] as f32),
    );
    // Back to the parent's space.
    let parent_global = world
        .get::<ChildOf>(e)
        .and_then(|c| world.get::<GlobalTransform>(c.parent()))
        .map_or(Affine3A::IDENTITY, |g| g.0);
    let local_affine = parent_global.inverse() * world_new;
    let (ls, lr, lt) = local_affine.to_scale_rotation_translation();
    let mut after = local;
    after.translation = lt;
    after.rotation = lr.normalize();
    after.scale = ls;
    if after != local {
        world.entity_mut(e).insert(after);
        let label = match ed.ui.tool {
            Tool::Rotate => "Rotate",
            Tool::Scale => "Scale",
            _ => "Move",
        };
        let merge = egui::Id::new("gizmo").value();
        ed.history.record(
            world,
            Box::new(crate::commands::SetComponent {
                entity: e,
                before: local,
                after,
                label,
            }),
            Some(merge),
        );
        ed.history.keep_open(merge);
    } else if ed.gizmo.is_focused() {
        ed.history.keep_open(egui::Id::new("gizmo").value());
    }
}

fn row_matrix(m: glam::DMat4) -> mint::RowMatrix4<f64> {
    let t = m.transpose().to_cols_array_2d();
    mint::RowMatrix4 {
        x: t[0].into(),
        y: t[1].into(),
        z: t[2].into(),
        w: t[3].into(),
    }
}

/// Painter overlays: selection bounds, light / camera / empty icons, spot cones, names.
pub fn overlays(painter: &egui::Painter, world: &mut World, ed: &Editor, view: &ViewInfo) {
    let toggles = ed.ui.overlays;
    let selected = ed.selected;

    // Selection bounds (and per-object "Show Bounds").
    let mut meshes = world.query_filtered::<(
        Entity,
        &GlobalTransform,
        &MeshRenderer,
        Option<&ViewportDisplay>,
        Option<&Visibility>,
    ), Without<EditorOnly>>();
    // A selected model instance outlines every part of it.
    let selected_scene = selected.filter(|e| world.get::<ge_render::SceneRoot>(*e).is_some());
    let world_ref: &World = world;
    let in_selected_scene = |mut e: Entity| {
        let Some(root) = selected_scene else {
            return false;
        };
        loop {
            if e == root {
                return true;
            }
            match world_ref.get::<ChildOf>(e) {
                Some(p) => e = p.parent(),
                None => return false,
            }
        }
    };
    for (e, global, mesh, display, vis) in meshes.iter(world_ref) {
        if vis.is_some_and(|v| !v.visible) {
            continue;
        }
        let is_selected = selected == Some(e) || in_selected_scene(e);
        let show =
            (is_selected && toggles.selection_bounds) || display.is_some_and(|d| d.show_bounds);
        if show {
            let color = if is_selected {
                theme::SELECTION
            } else {
                theme::TEXT_MUTED
            };
            let (center, half) = crate::state::mesh_bounds(world_ref, &mesh.mesh);
            draw_box(
                painter,
                view,
                global.to_mat4() * Mat4::from_translation(center),
                half,
                color,
            );
        }
    }

    // Icons for lights, cameras and empties; axes and names on request.
    let mut objects = world.query_filtered::<(
        Entity,
        &GlobalTransform,
        Option<&Light>,
        Option<&Camera3d>,
        Has<MeshRenderer>,
        Option<&ViewportDisplay>,
        Option<&Name>,
    ), Without<EditorOnly>>();
    for (e, global, light, camera, has_mesh, display, name) in objects.iter(world) {
        let p = global.translation();
        let Some(screen) = view.project(p) else {
            continue;
        };
        if !view.screen.expand(40.0).contains(screen) {
            continue;
        }
        let is_selected = selected == Some(e);
        let tint = |base: Color32| if is_selected { theme::SELECTION } else { base };

        if toggles.icons {
            if let Some(light) = light {
                let glyph = match light.kind {
                    LightKind::Sun => icon::SUN,
                    LightKind::Spot => icon::FLASHLIGHT,
                    LightKind::Area => icon::SQUARE,
                    LightKind::Point => icon::LIGHTBULB,
                };
                icon_at(painter, screen, glyph, tint(theme::LIGHT_ICON));
                let dir = global.forward();
                if light.kind == LightKind::Spot && (light.show_cone || is_selected) {
                    draw_cone(
                        painter,
                        view,
                        p,
                        dir,
                        light.spot_size,
                        tint(theme::LIGHT_ICON),
                    );
                } else if matches!(
                    light.kind,
                    LightKind::Sun | LightKind::Area | LightKind::Spot
                ) {
                    let len = view.world_per_point(p) * 60.0;
                    line3(
                        painter,
                        view,
                        p,
                        p + dir * len,
                        tint(theme::LIGHT_ICON),
                        1.0,
                    );
                }
            } else if camera.is_some() {
                icon_at(
                    painter,
                    screen,
                    icon::VIDEO_CAMERA,
                    tint(theme::CAMERA_ICON),
                );
                let len = view.world_per_point(p) * 40.0;
                line3(
                    painter,
                    view,
                    p,
                    p + global.forward() * len,
                    tint(theme::CAMERA_ICON),
                    1.0,
                );
            } else if !has_mesh && display.is_none_or(|d| !d.show_axes) && is_selected {
                icon_at(painter, screen, icon::CROSSHAIR, theme::SELECTION);
            }
        }
        if display.is_some_and(|d| d.show_axes)
            || (is_selected && !has_mesh && light.is_none() && camera.is_none())
        {
            let len = view.world_per_point(p) * 50.0;
            for (axis, color) in [
                (Vec3::X, theme::AXIS_X),
                (Vec3::Y, theme::AXIS_Y),
                (Vec3::Z, theme::AXIS_Z),
            ] {
                let d = global.0.transform_vector3(axis).normalize_or_zero();
                line3(painter, view, p, p + d * len, color, 1.5);
            }
        }
        if (display.is_some_and(|d| d.show_name) || (is_selected && !has_mesh))
            && let Some(name) = name
        {
            {
                painter.text(
                    screen + egui::vec2(12.0, -12.0),
                    egui::Align2::LEFT_BOTTOM,
                    name.as_str(),
                    egui::FontId::proportional(12.0),
                    if is_selected {
                        theme::SELECTION
                    } else {
                        theme::TEXT
                    },
                );
            }
        }
    }
}

fn icon_at(painter: &egui::Painter, at: Pos2, glyph: &str, color: Color32) {
    painter.circle_filled(at, 11.0, Color32::from_black_alpha(140));
    painter.text(
        at,
        egui::Align2::CENTER_CENTER,
        glyph,
        egui::FontId::proportional(14.0),
        color,
    );
}

fn line3(painter: &egui::Painter, view: &ViewInfo, a: Vec3, b: Vec3, color: Color32, width: f32) {
    if let (Some(a), Some(b)) = (view.project(a), view.project(b)) {
        painter.line_segment([a, b], Stroke::new(width, color));
    }
}

fn draw_box(painter: &egui::Painter, view: &ViewInfo, m: Mat4, half: Vec3, color: Color32) {
    let corner = |i: usize| {
        let s = Vec3::new(
            if i & 1 == 0 { -1.0 } else { 1.0 },
            if i & 2 == 0 { -1.0 } else { 1.0 },
            if i & 4 == 0 { -1.0 } else { 1.0 },
        );
        m.transform_point3(s * half)
    };
    for (a, b) in [
        (0, 1),
        (2, 3),
        (4, 5),
        (6, 7),
        (0, 2),
        (1, 3),
        (4, 6),
        (5, 7),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ] {
        line3(painter, view, corner(a), corner(b), color, 1.5);
    }
}

fn draw_cone(
    painter: &egui::Painter,
    view: &ViewInfo,
    apex: Vec3,
    dir: Vec3,
    angle: f32,
    color: Color32,
) {
    let len = 3.0;
    let radius = (angle * 0.5).tan() * len;
    let side = dir.any_orthonormal_vector();
    let up = dir.cross(side);
    let center = apex + dir * len;
    let mut previous = None;
    for i in 0..=24 {
        let a = i as f32 / 24.0 * std::f32::consts::TAU;
        let p = center + (side * a.cos() + up * a.sin()) * radius;
        if let Some(prev) = previous {
            line3(painter, view, prev, p, color, 1.0);
        }
        if i % 6 == 0 {
            line3(painter, view, apex, p, color, 1.0);
        }
        previous = Some(p);
    }
}

/// Blender's navigation gizmo: axis dots you can click to look along that axis.
pub fn nav_gizmo(
    ui: &mut egui::Ui,
    center: Pos2,
    world: &mut World,
    ed: &Editor,
) -> Option<crate::actions::Op> {
    use crate::actions::Op;
    let cam = world.get::<EditorCamera>(ed.camera).copied()?;
    let rot = cam.transform().rotation.inverse();
    let radius = 34.0;
    let painter = ui.painter();
    painter.circle_filled(center, radius + 8.0, Color32::from_white_alpha(10));
    let mut axes = [
        (Vec3::X, theme::AXIS_X, "X", Op::ViewRight),
        (Vec3::Y, theme::AXIS_Y, "Y", Op::ViewTop),
        (Vec3::Z, theme::AXIS_Z, "Z", Op::ViewFront),
        (Vec3::NEG_X, theme::AXIS_X, "", Op::ViewLeft),
        (Vec3::NEG_Y, theme::AXIS_Y, "", Op::ViewBottom),
        (Vec3::NEG_Z, theme::AXIS_Z, "", Op::ViewBack),
    ]
    .map(|(axis, color, label, op)| {
        let v = rot * axis;
        (v, color, label, op)
    });
    // Back to front.
    axes.sort_by(|a, b| a.0.z.total_cmp(&b.0.z));
    let mut clicked = None;
    for (v, color, label, op) in axes {
        let p = center + egui::vec2(v.x, -v.y) * radius;
        if !label.is_empty() {
            painter.line_segment([center, p], Stroke::new(2.0, color.gamma_multiply(0.8)));
        }
        let r = if label.is_empty() { 6.0 } else { 9.0 };
        let fill = if label.is_empty() {
            color.gamma_multiply(0.45)
        } else {
            color
        };
        painter.circle_filled(p, r, fill);
        if !label.is_empty() {
            painter.text(
                p,
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::proportional(11.0),
                theme::ACCENT_TEXT,
            );
        }
        let hit = ui.interact(
            egui::Rect::from_center_size(p, egui::vec2(r * 2.0, r * 2.0)),
            ui.id().with(("nav", label, op.label())),
            egui::Sense::click(),
        );
        if hit.clicked() {
            clicked = Some(op);
        }
    }
    clicked
}
