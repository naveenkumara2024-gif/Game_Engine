//! The editor (plan 0004, lore §3.11, §6.10): an in-process plugin that floats a Blender-style
//! UI over the running engine. egui is the UI library, wired by hand to our platform events and
//! UI pass (`egui_glue`), so no other crate touches it (xtask facade rule).
//!
//! ```ignore
//! App::new().add_plugins(DefaultPlugins).add_plugins(EditorPlugin).add_plugins(MyGame).run()
//! ```
//!
//! The game keeps running its own `Startup`; the editor starts in Edit mode (gameplay tick
//! groups paused) and Play / Pause / Stop run the game in place (`play`).

#![forbid(unsafe_code)]

mod actions;
mod chrome;
mod commands;
mod egui_glue;
mod outliner;
mod play;
mod properties;
mod state;
mod theme;
mod viewport;
mod widgets;

use ge_ecs::prelude::*;
use ge_engine::input::PlatformEvents;
use ge_engine::{App, Plugin, SimulationState, TickGroup, WindowInfo};
use ge_render::lighting::R_VIEWPORT_SHADING;
use ge_render::{Camera3d, CameraOverride, Overlays, UiOutput, ViewportOverlays, WorldSettings};
use tracing::info_span;

pub use actions::Op;
pub use state::{Editor, EditorCamera, EditorOnly};

/// Adds the editor. Depends on the default plugins (input, transforms, rendering).
#[derive(Debug, Default)]
pub struct EditorPlugin;

impl Plugin for EditorPlugin {
    fn name(&self) -> &'static str {
        "ge.editor"
    }

    fn dependencies(&self) -> &'static [&'static str] {
        &["ge.input", "ge.transform", "ge.render"]
    }

    fn build(&self, app: &mut App) {
        let world = app.world_mut();
        let ec = EditorCamera::default();
        let camera = world
            .spawn((
                EditorOnly,
                Name::new("Editor Camera"),
                ec,
                ec.transform(),
                Camera3d {
                    fov_y: 50f32.to_radians(),
                    ..Default::default()
                },
            ))
            .id();
        world.insert_resource(new_editor(camera));
        world.insert_resource(SimulationState::Editing);
        world.insert_resource(CameraOverride(Some(camera)));
        world.init_resource::<UiOutput>();
        world.init_resource::<ViewportOverlays>();
        if !world.contains_resource::<WorldSettings>() {
            world.init_resource::<WorldSettings>();
        }
        // Start in Rendered shading so lights and materials show immediately, with Blender's
        // default view transform (AgX). The standalone game keeps the engine defaults.
        R_VIEWPORT_SHADING.set(1);
        ge_render::lighting::R_VIEW_TRANSFORM
            .set(ge_render::lighting::ViewTransform::AgX.as_cvar());
        app.add_systems(TickGroup::Ui, editor_ui);
    }
}

pub(crate) fn new_editor(camera: Entity) -> Editor {
    let ctx = egui::Context::default();
    theme::install(&ctx, 1.0);
    Editor {
        ctx,
        input: Default::default(),
        ui: Default::default(),
        selected: None,
        history: Default::default(),
        camera,
        gizmo: Default::default(),
        pie: Default::default(),
        applied_scale: 1.0,
        now: 0.0,
        ui_ms: 0.0,
    }
}

/// The whole editor UI, once per frame (exclusive: panels edit the world directly).
fn editor_ui(world: &mut World) {
    let _span = info_span!("Editor::Ui").entered();
    let started = std::time::Instant::now();
    world.resource_scope(|world, mut ed: Mut<Editor>| {
        let ed = &mut *ed;
        let cost = ed.ui_ms;
        play::intercept_game_exit(world, ed);

        let window = *world.resource::<WindowInfo>();
        let (time, dt) = {
            let t = world.resource::<Time>();
            (f64::from(t.elapsed_secs()), t.delta_secs())
        };
        ed.now = time;
        if (ed.applied_scale - ed.ui.ui_scale).abs() > 1e-3 {
            theme::apply_style(&ed.ctx, ed.ui.ui_scale);
            ed.applied_scale = ed.ui.ui_scale;
        }
        let events = std::mem::take(&mut world.resource_mut::<PlatformEvents>().0);
        let raw = ed
            .input
            .raw_input(&events, window.size, window.scale_factor, time, dt);
        world.resource_mut::<PlatformEvents>().0 = events;

        let ctx = ed.ctx.clone();
        let mut ops: Vec<Op> = Vec::new();
        let output = ctx.run_ui(raw, |root| frame(root, world, ed, &mut ops));
        for op in ops {
            op.run(world, ed);
        }
        ed.history.end_frame();
        if let Some((_, at)) = ed.ui.focus_setting
            && time - at > 1.5
        {
            ed.ui.focus_setting = None;
        }

        let state = *world.resource::<SimulationState>();
        world.resource_mut::<ViewportOverlays>().0 = Overlays {
            grid: ed.ui.overlays.grid && state != SimulationState::Playing,
        };
        let scale = output.pixels_per_point;
        let mut ui_out = world.resource_mut::<UiOutput>();
        egui_glue::write_draw_list(&ctx, output, scale, &mut ui_out.0);
        // Exponential moving average so the readout is stable.
        let ms = started.elapsed().as_secs_f32() * 1000.0;
        ed.ui_ms = if cost == 0.0 {
            ms
        } else {
            cost * 0.95 + ms * 0.05
        };
    });
}

/// One UI frame: viewport first (background layer), then the floating chrome on top.
fn frame(root: &mut egui::Ui, world: &mut World, ed: &mut Editor, ops: &mut Vec<Op>) {
    let ctx = root.ctx().clone();
    let screen = ctx.content_rect();
    let state = *world.resource::<SimulationState>();
    let playing = state == SimulationState::Playing;

    // Viewport: navigation, picking, gizmo, overlays.
    let response = root.interact(
        screen,
        egui::Id::new("viewport"),
        egui::Sense::click_and_drag(),
    );
    if state == SimulationState::Editing {
        viewport::navigate(root, &response, world, ed);
    }
    if !playing && let Some(view) = viewport::view_info(world, ed, screen) {
        viewport::overlays(root.painter(), world, ed, &view);
        viewport::gizmo(root, world, ed, &view);
        viewport::click_select(&response, world, ed, &view);
    }
    if state == SimulationState::Editing {
        let right = if ed.ui.show_outliner { 290.0 } else { 0.0 };
        let center = egui::pos2(
            screen.max.x - theme::GUTTER - 52.0 - right,
            screen.min.y + 140.0,
        );
        if let Some(op) = viewport::nav_gizmo(root, center, world, ed) {
            ops.push(op);
        }
    }

    // Shortcuts (after the viewport so text fields that took focus this frame win).
    ops.extend(actions::shortcuts(&ctx, world, ed));
    if !playing
        && !ctx.egui_wants_keyboard_input()
        && ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::A))
    {
        ed.ui.add_menu_at = ctx.pointer_hover_pos().or(Some(screen.center()));
    }

    // Chrome.
    chrome::play_controls(&ctx, world, ed, ops);
    if !playing {
        chrome::main_menu(&ctx, world, ed, ops);
        chrome::toolbar(&ctx, world, ed, ops);
        chrome::view_cluster(&ctx, world, ed, ops);
        if ed.ui.show_properties {
            egui::Area::new(egui::Id::new("properties"))
                .anchor(egui::Align2::LEFT_TOP, [theme::GUTTER, 72.0])
                .show(&ctx, |ui| {
                    theme::panel_frame().show(ui, |ui| {
                        // Leave room for the bottom-left view cluster.
                        ui.set_max_height((screen.height() - 72.0 - 84.0).max(120.0));
                        properties::draw(ui, world, ed);
                    });
                });
        }
        if ed.ui.show_outliner {
            egui::Area::new(egui::Id::new("outliner"))
                .anchor(egui::Align2::RIGHT_TOP, [-theme::GUTTER, 72.0])
                .show(&ctx, |ui| {
                    theme::panel_frame().show(ui, |ui| {
                        ui.set_width(250.0);
                        ui.set_max_height((screen.height() * 0.6).max(200.0));
                        outliner::draw(ui, world, ed);
                    });
                });
        }
        chrome::add_menu_popup(&ctx, world, ed, ops);
        chrome::search(&ctx, world, ed, ops);
        chrome::windows(&ctx, world, ed, ops);
    } else {
        chrome::toolbar_hint_only(&ctx);
    }
    chrome::stats_corner(&ctx, world, ed);
    chrome::toast(&ctx, ed);
}
