//! Operators (finding B3): every action the editor can perform, whether it comes from a menu,
//! a toolbar button, a shortcut or F3 search, goes through [`Op::run`], so everything is
//! undoable and searchable from one place.

use std::f32::consts::FRAC_PI_2;

use ge_ecs::clone_tree;
use ge_ecs::prelude::*;
use ge_engine::SimulationState;
use ge_render::{
    Camera3d, Light, LightKind, Material, MeshKind, MeshRenderer, Primitive, PrimitiveMesh,
    Visibility,
};

use crate::commands::{AddComponent, SetAlive, SetComponent};
use crate::play;
use crate::state::{Editor, EditorCamera, Tab, Tool, ViewportDisplay, unique_name};

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Op {
    AddMesh(Primitive),
    AddLight(LightKind),
    AddCamera,
    AddEmpty,
    Delete,
    Duplicate,
    HideSelected,
    UnhideAll,
    FrameSelected,
    Undo,
    Redo,
    SetTool(Tool),
    ShowTab(Tab),
    ToggleProperties,
    ToggleOutliner,
    PlayPause,
    Stop,
    Search,
    Shortcuts,
    ViewFront,
    ViewRight,
    ViewTop,
    ViewBack,
    ViewLeft,
    ViewBottom,
}

impl Op {
    pub fn label(self) -> String {
        match self {
            Op::AddMesh(p) => format!("Add › Mesh › {}", p.label()),
            Op::AddLight(k) => format!("Add › Light › {}", k.label()),
            Op::AddCamera => "Add › Camera".into(),
            Op::AddEmpty => "Add › Empty".into(),
            Op::Delete => "Delete".into(),
            Op::Duplicate => "Duplicate".into(),
            Op::HideSelected => "Hide Selected".into(),
            Op::UnhideAll => "Unhide All".into(),
            Op::FrameSelected => "Frame Selected".into(),
            Op::Undo => "Undo".into(),
            Op::Redo => "Redo".into(),
            Op::SetTool(Tool::Select) => "Tool: Select".into(),
            Op::SetTool(Tool::Move) => "Tool: Move".into(),
            Op::SetTool(Tool::Rotate) => "Tool: Rotate".into(),
            Op::SetTool(Tool::Scale) => "Tool: Scale".into(),
            Op::ShowTab(t) => format!("Properties: {t:?}"),
            Op::ToggleProperties => "Toggle Properties Panel".into(),
            Op::ToggleOutliner => "Toggle Outliner".into(),
            Op::PlayPause => "Play / Pause".into(),
            Op::Stop => "Stop Playing".into(),
            Op::Search => "Search".into(),
            Op::Shortcuts => "Keyboard Shortcuts".into(),
            Op::ViewFront => "View: Front".into(),
            Op::ViewRight => "View: Right".into(),
            Op::ViewTop => "View: Top".into(),
            Op::ViewBack => "View: Back".into(),
            Op::ViewLeft => "View: Left".into(),
            Op::ViewBottom => "View: Bottom".into(),
        }
    }

    pub fn shortcut(self) -> &'static str {
        match self {
            Op::AddMesh(_) | Op::AddLight(_) | Op::AddCamera | Op::AddEmpty => "Shift A",
            Op::Delete => "X / Del",
            Op::Duplicate => "Shift D",
            Op::HideSelected => "H",
            Op::UnhideAll => "Alt H",
            Op::FrameSelected => "F",
            Op::Undo => "Ctrl Z",
            Op::Redo => "Ctrl Shift Z",
            Op::SetTool(Tool::Select) => "Q",
            Op::SetTool(Tool::Move) => "G",
            Op::SetTool(Tool::Rotate) => "R",
            Op::SetTool(Tool::Scale) => "S",
            Op::ToggleProperties => "N",
            Op::PlayPause => "Alt P",
            Op::Stop => "Shift Esc",
            Op::Search => "F3",
            Op::Shortcuts => "F1",
            Op::ViewFront => "1",
            Op::ViewRight => "3",
            Op::ViewTop => "7",
            Op::ViewBack => "Ctrl 1",
            Op::ViewLeft => "Ctrl 3",
            Op::ViewBottom => "Ctrl 7",
            Op::ShowTab(_) | Op::ToggleOutliner => "",
        }
    }

    /// Whether the op makes sense right now (Blender's `poll`).
    pub fn enabled(self, world: &World, ed: &Editor) -> bool {
        let editing = *world.resource::<SimulationState>() != SimulationState::Playing;
        match self {
            Op::Delete | Op::Duplicate | Op::HideSelected | Op::FrameSelected => {
                editing && ed.selected.is_some()
            }
            Op::Undo => editing && ed.history.can_undo(),
            Op::Redo => editing && ed.history.can_redo(),
            Op::Stop => *world.resource::<SimulationState>() != SimulationState::Editing,
            Op::AddMesh(_) | Op::AddLight(_) | Op::AddCamera | Op::AddEmpty | Op::UnhideAll => {
                editing
            }
            _ => true,
        }
    }

    /// Every op listed in F3 search.
    pub fn searchable() -> Vec<Op> {
        let mut ops: Vec<Op> = Primitive::ALL_DEFAULTS
            .iter()
            .map(|p| Op::AddMesh(*p))
            .collect();
        ops.extend(LightKind::ALL.iter().map(|k| Op::AddLight(*k)));
        ops.extend([
            Op::AddCamera,
            Op::AddEmpty,
            Op::Delete,
            Op::Duplicate,
            Op::HideSelected,
            Op::UnhideAll,
            Op::FrameSelected,
            Op::Undo,
            Op::Redo,
            Op::SetTool(Tool::Select),
            Op::SetTool(Tool::Move),
            Op::SetTool(Tool::Rotate),
            Op::SetTool(Tool::Scale),
            Op::ShowTab(Tab::Render),
            Op::ShowTab(Tab::World),
            Op::ShowTab(Tab::Object),
            Op::ShowTab(Tab::Data),
            Op::ShowTab(Tab::Material),
            Op::ToggleProperties,
            Op::ToggleOutliner,
            Op::PlayPause,
            Op::Stop,
            Op::Shortcuts,
            Op::ViewFront,
            Op::ViewRight,
            Op::ViewTop,
            Op::ViewBack,
            Op::ViewLeft,
            Op::ViewBottom,
        ]);
        ops
    }

    pub fn run(self, world: &mut World, ed: &mut Editor) {
        if !self.enabled(world, ed) {
            return;
        }
        match self {
            Op::AddMesh(shape) => add_mesh(world, ed, shape),
            Op::AddLight(kind) => add_light(world, ed, kind),
            Op::AddCamera => add_camera(world, ed),
            Op::AddEmpty => add_empty(world, ed),
            Op::Delete => delete_selected(world, ed),
            Op::Duplicate => duplicate_selected(world, ed),
            Op::HideSelected => {
                if let Some(e) = ed.selected {
                    set_visible(world, ed, e, false);
                }
            }
            Op::UnhideAll => unhide_all(world, ed),
            Op::FrameSelected => frame_selected(world, ed),
            Op::Undo => {
                if let Some(label) = ed.history.undo(world) {
                    ed.toast(format!("Undo: {label}"));
                }
                drop_stale_selection(world, ed);
            }
            Op::Redo => {
                if let Some(label) = ed.history.redo(world) {
                    ed.toast(format!("Redo: {label}"));
                }
                drop_stale_selection(world, ed);
            }
            Op::SetTool(tool) => ed.ui.tool = tool,
            Op::ShowTab(tab) => {
                ed.ui.tab = tab;
                ed.ui.show_properties = true;
            }
            Op::ToggleProperties => ed.ui.show_properties = !ed.ui.show_properties,
            Op::ToggleOutliner => ed.ui.show_outliner = !ed.ui.show_outliner,
            Op::PlayPause => {
                if play::state(world) == SimulationState::Playing {
                    play::pause(world, ed);
                } else {
                    play::play(world, ed);
                }
            }
            Op::Stop => play::stop(world, ed),
            Op::Search => {
                ed.ui.search = Some(String::new());
                ed.ui.search_selected = 0;
            }
            Op::Shortcuts => ed.ui.help_open = !ed.ui.help_open,
            Op::ViewFront => set_view(world, ed, 0.0, 0.0),
            Op::ViewBack => set_view(world, ed, std::f32::consts::PI, 0.0),
            Op::ViewRight => set_view(world, ed, FRAC_PI_2, 0.0),
            Op::ViewLeft => set_view(world, ed, -FRAC_PI_2, 0.0),
            Op::ViewTop => set_view(world, ed, 0.0, FRAC_PI_2 - 1e-3),
            Op::ViewBottom => set_view(world, ed, 0.0, -FRAC_PI_2 + 1e-3),
        }
    }
}

fn drop_stale_selection(world: &World, ed: &mut Editor) {
    if let Some(e) = ed.selected
        && (world.get_entity(e).is_err() || world.get::<Disabled>(e).is_some())
    {
        ed.selected = None;
    }
}

/// Where new objects appear: on the ground under the editor camera's focus point (we have no
/// 3D cursor yet; Blender adds at its 3D cursor).
fn spawn_point(world: &World, ed: &Editor) -> Vec3 {
    let focus = world
        .get::<EditorCamera>(ed.camera)
        .map_or(Vec3::ZERO, |c| c.focus);
    Vec3::new(focus.x, 0.0, focus.z)
}

fn finish_spawn(world: &mut World, ed: &mut Editor, entity: Entity, label: String) {
    ed.history.record(
        world,
        Box::new(SetAlive {
            entity,
            alive_when_applied: true,
            label: label.clone(),
        }),
        None,
    );
    ed.selected = Some(entity);
    ed.ui.tab = if world.get::<Light>(entity).is_some() {
        Tab::Data
    } else {
        Tab::Object
    };
    ed.toast(label);
}

pub fn add_mesh(world: &mut World, ed: &mut Editor, shape: Primitive) {
    let lift = shape.half_extents().y;
    let at = spawn_point(world, ed) + Vec3::Y * lift;
    let name = unique_name(world, shape.label().replace(' ', "").as_str());
    let e = world
        .spawn((
            Name::new(name),
            Transform::from_translation(at),
            MeshRenderer::new(MeshKind::Primitive(PrimitiveMesh::new(shape)), [1.0; 3]),
            Material::default(),
            Visibility::default(),
            ViewportDisplay::default(),
        ))
        .id();
    finish_spawn(world, ed, e, format!("Add {}", shape.label()));
}

pub fn add_light(world: &mut World, ed: &mut Editor, kind: LightKind) {
    let ground = spawn_point(world, ed);
    let transform = match kind {
        // Blender's default sun leans in from the side; spots and areas point straight down.
        LightKind::Sun => Transform::from_translation(ground + Vec3::new(0.0, 8.0, 0.0))
            .with_rotation(Quat::from_euler(EulerRot::YXZ, 0.6, -0.9, 0.0)),
        LightKind::Spot | LightKind::Area => Transform::from_translation(ground + Vec3::Y * 4.0)
            .with_rotation(Quat::from_rotation_x(-FRAC_PI_2)),
        LightKind::Point => Transform::from_translation(ground + Vec3::Y * 3.0),
    };
    let name = unique_name(world, kind.label());
    let e = world
        .spawn((
            Name::new(name),
            transform,
            Light::new(kind),
            Visibility::default(),
            ViewportDisplay::default(),
        ))
        .id();
    finish_spawn(world, ed, e, format!("Add {} Light", kind.label()));
}

pub fn add_camera(world: &mut World, ed: &mut Editor) {
    // New cameras start at the current view (Blender's "align to view").
    let transform = world
        .get::<EditorCamera>(ed.camera)
        .map(EditorCamera::transform)
        .unwrap_or_default();
    let name = unique_name(world, "Camera");
    let e = world
        .spawn((
            Name::new(name),
            transform,
            Camera3d::default(),
            Visibility::default(),
            ViewportDisplay::default(),
        ))
        .id();
    finish_spawn(world, ed, e, "Add Camera".into());
}

pub fn add_empty(world: &mut World, ed: &mut Editor) {
    let at = spawn_point(world, ed);
    let name = unique_name(world, "Empty");
    let e = world
        .spawn((
            Name::new(name),
            Transform::from_translation(at),
            Visibility::default(),
            ViewportDisplay {
                show_axes: true,
                ..Default::default()
            },
        ))
        .id();
    finish_spawn(world, ed, e, "Add Empty".into());
}

fn delete_selected(world: &mut World, ed: &mut Editor) {
    let Some(e) = ed.selected.take() else { return };
    let (name, _) = crate::state::describe(world, e);
    ed.history.execute(
        world,
        Box::new(SetAlive {
            entity: e,
            alive_when_applied: false,
            label: format!("Delete {name}"),
        }),
        None,
    );
    ed.toast(format!("Deleted {name} · Ctrl+Z to undo"));
}

fn duplicate_selected(world: &mut World, ed: &mut Editor) {
    let Some(source) = ed.selected else { return };
    let copy = clone_tree(world, source);
    let base = world
        .get::<Name>(source)
        .map_or_else(|| "Object".to_owned(), |n| n.as_str().to_owned());
    let name = unique_name(world, &base);
    world.entity_mut(copy).insert(Name::new(name.clone()));
    finish_spawn(world, ed, copy, format!("Duplicate {name}"));
}

pub fn set_visible(world: &mut World, ed: &mut Editor, e: Entity, visible: bool) {
    let label = if visible { "Unhide" } else { "Hide" };
    match world.get::<Visibility>(e).copied() {
        Some(before) => ed.history.execute(
            world,
            Box::new(SetComponent {
                entity: e,
                before,
                after: Visibility { visible },
                label,
            }),
            None,
        ),
        None => ed.history.execute(
            world,
            Box::new(AddComponent {
                entity: e,
                value: Visibility { visible },
                label,
            }),
            None,
        ),
    }
}

fn unhide_all(world: &mut World, ed: &mut Editor) {
    let mut q = world.query::<(Entity, &Visibility)>();
    let hidden: Vec<Entity> = q
        .iter(world)
        .filter(|(_, v)| !v.visible)
        .map(|(e, _)| e)
        .collect();
    for e in hidden {
        set_visible(world, ed, e, true);
    }
}

fn frame_selected(world: &mut World, ed: &mut Editor) {
    let Some(e) = ed.selected else { return };
    let Some(global) = world.get::<GlobalTransform>(e).copied() else {
        return;
    };
    let radius = world
        .get::<MeshRenderer>(e)
        .map(|m| (global.0.matrix3 * glam::Vec3A::from(m.mesh.half_extents())).length())
        .unwrap_or(1.0)
        .max(0.5);
    if let Some(mut cam) = world.get_mut::<EditorCamera>(ed.camera) {
        cam.focus = global.translation();
        cam.distance = (radius * 3.0).clamp(1.0, 500.0);
    }
}

fn set_view(world: &mut World, ed: &mut Editor, yaw: f32, pitch: f32) {
    if let Some(mut cam) = world.get_mut::<EditorCamera>(ed.camera) {
        cam.yaw = yaw;
        cam.pitch = pitch;
    }
}

/// Keyboard shortcuts (Blender-style defaults). Skipped while a text field has focus.
pub fn shortcuts(ctx: &egui::Context, world: &World, ed: &Editor) -> Vec<Op> {
    use egui::{Key, Modifiers};
    if ctx.egui_wants_keyboard_input() {
        return Vec::new();
    }
    let playing = *world.resource::<SimulationState>() == SimulationState::Playing;
    let mut ops = Vec::new();
    ctx.input_mut(|i| {
        let mut take = |mods: Modifiers, key: Key, op: Op| {
            if i.consume_key(mods, key) {
                ops.push(op);
            }
        };
        // Play controls work in every mode; in Play mode the game owns the other keys.
        take(Modifiers::SHIFT, Key::Escape, Op::Stop);
        take(Modifiers::ALT, Key::P, Op::PlayPause);
        if playing {
            return;
        }
        take(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z, Op::Redo);
        take(Modifiers::COMMAND, Key::Z, Op::Undo);
        take(Modifiers::COMMAND, Key::Y, Op::Redo);
        take(Modifiers::SHIFT, Key::D, Op::Duplicate);
        take(Modifiers::ALT, Key::H, Op::UnhideAll);
        take(Modifiers::COMMAND, Key::Num1, Op::ViewBack);
        take(Modifiers::COMMAND, Key::Num3, Op::ViewLeft);
        take(Modifiers::COMMAND, Key::Num7, Op::ViewBottom);
        take(Modifiers::NONE, Key::Q, Op::SetTool(Tool::Select));
        take(Modifiers::NONE, Key::G, Op::SetTool(Tool::Move));
        take(Modifiers::NONE, Key::R, Op::SetTool(Tool::Rotate));
        take(Modifiers::NONE, Key::S, Op::SetTool(Tool::Scale));
        take(Modifiers::NONE, Key::X, Op::Delete);
        take(Modifiers::NONE, Key::Delete, Op::Delete);
        take(Modifiers::NONE, Key::H, Op::HideSelected);
        take(Modifiers::NONE, Key::F, Op::FrameSelected);
        take(Modifiers::NONE, Key::Period, Op::FrameSelected);
        take(Modifiers::NONE, Key::N, Op::ToggleProperties);
        take(Modifiers::NONE, Key::F3, Op::Search);
        take(Modifiers::NONE, Key::F1, Op::Shortcuts);
        take(Modifiers::NONE, Key::Num1, Op::ViewFront);
        take(Modifiers::NONE, Key::Num3, Op::ViewRight);
        take(Modifiers::NONE, Key::Num7, Op::ViewTop);
    });
    let _ = ed;
    ops
}
