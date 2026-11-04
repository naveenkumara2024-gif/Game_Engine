//! The floating chrome around the viewport (plan 0004 "Layout"): main menu, tool bar, hint
//! line, play controls, view/undo cluster, stats, F3 search, the Shift+A menu and the help /
//! preferences / history windows.

use egui::{Align2, RichText};
use egui_phosphor::regular as icon;
use ge_ecs::prelude::*;
use ge_engine::{EngineStats, SimulationState};
use ge_render::lighting::{R_VIEWPORT_SHADING, ViewportShading};
use ge_render::settings;
use ge_render::{Camera3d, LightKind, MeshRenderer, Primitive, R_DEBUG_NORMALS};

use crate::actions::Op;
use crate::state::{Editor, Tab, Tool};
use crate::theme;

const TOP: f32 = theme::GUTTER;

fn icon_button(ui: &mut egui::Ui, glyph: &str, selected: bool, tooltip: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(glyph).size(17.0))
            .min_size(egui::vec2(34.0, 34.0))
            .selected(selected)
            .frame_when_inactive(selected),
    )
    .on_hover_text(tooltip)
}

/// The Add menu (shared by the tool bar button and Shift+A). Only categories that work are
/// listed; asset import is shown greyed with its milestone.
pub fn add_menu_contents(ui: &mut egui::Ui, ops: &mut Vec<Op>) {
    ui.set_min_width(170.0);
    ui.menu_button(format!("{}  Mesh", icon::CUBE), |ui| {
        for shape in Primitive::ALL_DEFAULTS {
            let glyph = match shape {
                Primitive::Plane { .. } => icon::SQUARE,
                Primitive::Cube { .. } => icon::CUBE,
                Primitive::Circle { .. } => icon::CIRCLE,
                Primitive::UvSphere { .. } => icon::GLOBE,
                Primitive::IcoSphere { .. } => icon::POLYGON,
                Primitive::Cylinder { .. } => icon::CYLINDER,
                Primitive::Cone { .. } => icon::TRIANGLE,
                Primitive::Torus { .. } => icon::CIRCLE_DASHED,
            };
            if ui.button(format!("{glyph}  {}", shape.label())).clicked() {
                ops.push(Op::AddMesh(shape));
                ui.close();
            }
        }
    });
    ui.menu_button(format!("{}  Light", icon::LIGHTBULB), |ui| {
        for kind in LightKind::ALL {
            let glyph = match kind {
                LightKind::Point => icon::LIGHTBULB,
                LightKind::Sun => icon::SUN,
                LightKind::Spot => icon::FLASHLIGHT,
                LightKind::Area => icon::SQUARE,
            };
            if ui.button(format!("{glyph}  {}", kind.label())).clicked() {
                ops.push(Op::AddLight(kind));
                ui.close();
            }
        }
    });
    if ui
        .button(format!("{}  Camera", icon::VIDEO_CAMERA))
        .clicked()
    {
        ops.push(Op::AddCamera);
        ui.close();
    }
    if ui.button(format!("{}  Empty", icon::CROSSHAIR)).clicked() {
        ops.push(Op::AddEmpty);
        ui.close();
    }
    ui.separator();
    ui.add_enabled(
        false,
        egui::Button::new(format!("{}  Import glTF…", icon::PACKAGE)),
    )
    .on_disabled_hover_text("Planned (M3): the asset system loads .gltf/.glb meshes.");
}

pub fn main_menu(ctx: &egui::Context, world: &mut World, ed: &mut Editor, ops: &mut Vec<Op>) {
    egui::Area::new(egui::Id::new("main_menu"))
        .anchor(Align2::LEFT_TOP, [theme::GUTTER, TOP])
        .show(ctx, |ui| {
            theme::panel_frame().inner_margin(4).show(ui, |ui| {
                ui.menu_button(RichText::new(icon::LIST).size(18.0), |ui| {
                    ui.set_min_width(200.0);
                    ui.menu_button("File", |ui| {
                        for label in ["New Scene", "Open…", "Save", "Save As…"] {
                            ui.add_enabled(false, egui::Button::new(label))
                                .on_disabled_hover_text("Planned (M6): RON scenes with GUIDs.");
                        }
                        ui.separator();
                        if ui.button("Quit").clicked() {
                            world
                                .resource_mut::<ge_engine::ExitRequest>()
                                .request(ge_engine::EXIT_WINDOW_CLOSED);
                        }
                    });
                    ui.menu_button("Edit", |ui| {
                        menu_op(ui, world, ed, Op::Undo, ops);
                        menu_op(ui, world, ed, Op::Redo, ops);
                        if ui.button("Undo History…").clicked() {
                            ed.ui.history_open = true;
                            ui.close();
                        }
                        ui.separator();
                        menu_op(ui, world, ed, Op::Duplicate, ops);
                        menu_op(ui, world, ed, Op::Delete, ops);
                        ui.separator();
                        if ui.button("Preferences…").clicked() {
                            ed.ui.prefs_open = true;
                            ui.close();
                        }
                    });
                    ui.menu_button("Add", |ui| add_menu_contents(ui, ops));
                    ui.menu_button("View", |ui| {
                        ui.checkbox(&mut ed.ui.show_properties, "Properties (N)");
                        ui.checkbox(&mut ed.ui.show_outliner, "Outliner");
                        ui.checkbox(&mut ed.ui.overlays.stats, "Statistics");
                        ui.separator();
                        for op in [Op::FrameSelected, Op::ViewFront, Op::ViewRight, Op::ViewTop] {
                            menu_op(ui, world, ed, op, ops);
                        }
                        ui.separator();
                        if ui.button("Reset Layout").clicked() {
                            ed.ui.show_properties = true;
                            ed.ui.show_outliner = true;
                            ui.ctx().memory_mut(|m| m.reset_areas());
                            ui.close();
                        }
                    });
                    ui.menu_button("Help", |ui| {
                        menu_op(ui, world, ed, Op::Shortcuts, ops);
                        menu_op(ui, world, ed, Op::Search, ops);
                        if ui.button("About").clicked() {
                            ed.ui.about_open = true;
                            ui.close();
                        }
                    });
                });
            });
        });
}

fn menu_op(ui: &mut egui::Ui, world: &World, ed: &Editor, op: Op, ops: &mut Vec<Op>) {
    let button = egui::Button::new(op.label()).shortcut_text(op.shortcut());
    if ui.add_enabled(op.enabled(world, ed), button).clicked() {
        ops.push(op);
        ui.close();
    }
}

pub fn toolbar(ctx: &egui::Context, world: &mut World, ed: &mut Editor, ops: &mut Vec<Op>) {
    let playing = *world.resource::<SimulationState>() != SimulationState::Editing;
    egui::Area::new(egui::Id::new("toolbar"))
        .anchor(Align2::CENTER_TOP, [0.0, TOP])
        .show(ctx, |ui| {
            theme::panel_frame().inner_margin(5).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 3.0;
                    for (tool, glyph, key, tip) in [
                        (Tool::Select, icon::CURSOR, "Q", "Select (Q): click objects"),
                        (Tool::Move, icon::ARROWS_OUT_CARDINAL, "G", "Move (G)"),
                        (Tool::Rotate, icon::ARROWS_CLOCKWISE, "R", "Rotate (R)"),
                        (Tool::Scale, icon::ARROWS_OUT, "S", "Scale (S)"),
                    ] {
                        let r = icon_button(ui, glyph, ed.ui.tool == tool, tip);
                        key_hint(ui, &r, key);
                        if r.clicked() {
                            ed.ui.tool = tool;
                        }
                    }
                    ui.separator();
                    ui.add_enabled_ui(!playing, |ui| {
                        ui.menu_button(
                            RichText::new(format!("{}  Add", icon::PLUS)).size(14.0),
                            |ui| add_menu_contents(ui, ops),
                        )
                        .response
                        .on_hover_text("Add mesh, light, camera or empty (Shift+A)");
                    });
                    ui.separator();
                    let orient = if ed.ui.local_orientation {
                        (icon::CUBE_TRANSPARENT, "Local")
                    } else {
                        (icon::GLOBE, "Global")
                    };
                    if ui
                        .add(
                            egui::Button::new(format!("{}  {}", orient.0, orient.1))
                                .frame_when_inactive(false),
                        )
                        .on_hover_text(
                            "Transform orientation: gizmo axes follow the world or the object",
                        )
                        .clicked()
                    {
                        ed.ui.local_orientation = !ed.ui.local_orientation;
                    }
                    icon_button(
                        ui,
                        icon::MAGNET,
                        ed.ui.snap,
                        "Snap (or hold Ctrl while dragging): 0.25 m, 15°, 0.1×",
                    )
                    .clicked()
                    .then(|| ed.ui.snap = !ed.ui.snap);
                    ui.separator();
                    shading_menu(ui);
                    ui.menu_button(RichText::new(icon::DOTS_THREE_VERTICAL).size(17.0), |ui| {
                        ui.label(RichText::new("Overlays").strong());
                        ui.checkbox(&mut ed.ui.overlays.grid, "Floor grid & axes");
                        ui.checkbox(&mut ed.ui.overlays.icons, "Light & camera icons");
                        ui.checkbox(&mut ed.ui.overlays.selection_bounds, "Selection outline");
                        ui.checkbox(&mut ed.ui.overlays.stats, "Statistics");
                    })
                    .response
                    .on_hover_text("Viewport overlays");
                });
            });
        });

    // Context hint under the tool bar (the reference layout's "Click and drag…").
    let hint = if playing {
        "Playing: the game has the keyboard · Shift+Esc stop · Alt+P pause"
    } else {
        ed.ui.tool.hint()
    };
    hint_pill(ctx, hint, TOP + 56.0);
}

/// The context hint: a translucent pill so it reads over any scene (the reference layout puts
/// it on a flat canvas; ours is a lit 3D view).
fn hint_pill(ctx: &egui::Context, text: &str, y: f32) {
    egui::Area::new(egui::Id::new("hint"))
        .anchor(Align2::CENTER_TOP, [0.0, y])
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(egui::Color32::from_black_alpha(150))
                .corner_radius(egui::CornerRadius::same(10))
                .inner_margin(egui::Margin::symmetric(10, 3))
                .show(ui, |ui| {
                    ui.label(RichText::new(text).color(theme::TEXT).small());
                });
        });
}

/// While playing only the hint line stays (the game owns the screen).
pub fn toolbar_hint_only(ctx: &egui::Context) {
    hint_pill(ctx, "Playing · Shift+Esc stop · Alt+P pause", TOP + 8.0);
}

fn key_hint(ui: &egui::Ui, r: &egui::Response, key: &str) {
    ui.painter().text(
        r.rect.right_bottom() - egui::vec2(4.0, 3.0),
        Align2::RIGHT_BOTTOM,
        key,
        egui::FontId::proportional(9.0),
        theme::TEXT_MUTED,
    );
}

fn shading_menu(ui: &mut egui::Ui) {
    let normals = R_DEBUG_NORMALS.get();
    let rendered = R_VIEWPORT_SHADING.get() == 1;
    let (glyph, label) = if normals {
        (icon::SPHERE, "Normals")
    } else if rendered {
        (icon::SUN_DIM, "Rendered")
    } else {
        (icon::CIRCLE_HALF, "Solid")
    };
    ui.menu_button(RichText::new(format!("{glyph}  {label}")).size(13.0), |ui| {
        ui.label(RichText::new("Viewport Shading").strong());
        let pick = |ui: &mut egui::Ui, text: &str, tip: &str, shading: ViewportShading, n: bool| {
            let on = normals == n && (n || rendered == (shading == ViewportShading::Rendered));
            if ui.selectable_label(on, text).on_hover_text(tip).clicked() {
                R_DEBUG_NORMALS.set(n);
                R_VIEWPORT_SHADING.set(i32::from(shading == ViewportShading::Rendered));
                ui.close();
            }
        };
        pick(ui, format!("{}  Solid", icon::CIRCLE_HALF).as_str(),
            "A fixed studio light, ignoring scene lights (Blender's Solid mode). Good for modelling.",
            ViewportShading::Solid, false);
        pick(ui, format!("{}  Rendered", icon::SUN_DIM).as_str(),
            "Scene lights + world, as the game renders (Blender's Material Preview / Rendered).",
            ViewportShading::Rendered, false);
        pick(ui, format!("{}  Normals", icon::SPHERE).as_str(),
            "Colors each surface by the direction it faces (debug view: r.DebugNormals).",
            ViewportShading::Solid, true);
    })
    .response
    .on_hover_text("Viewport shading");
}

pub fn play_controls(ctx: &egui::Context, world: &mut World, ed: &mut Editor, ops: &mut Vec<Op>) {
    let state = *world.resource::<SimulationState>();
    egui::Area::new(egui::Id::new("play"))
        .anchor(Align2::RIGHT_TOP, [-theme::GUTTER, TOP])
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                match state {
                    SimulationState::Editing => {
                        let play = egui::Button::new(
                            RichText::new(format!("{}  Play", icon::PLAY))
                                .color(theme::ACCENT_TEXT)
                                .size(14.0),
                        )
                        .fill(theme::ACCENT)
                        .min_size(egui::vec2(86.0, 36.0))
                        .corner_radius(egui::CornerRadius::same(theme::RADIUS));
                        if ui
                            .add(play)
                            .on_hover_text("Play the game in the editor (Alt+P)")
                            .clicked()
                        {
                            ops.push(Op::PlayPause);
                        }
                    }
                    SimulationState::Playing | SimulationState::Paused => {
                        theme::panel_frame().inner_margin(4).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let (glyph, tip) = if state == SimulationState::Playing {
                                    (icon::PAUSE, "Pause (Alt+P)")
                                } else {
                                    (icon::PLAY, "Resume (Alt+P)")
                                };
                                if icon_button(ui, glyph, false, tip).clicked() {
                                    ops.push(Op::PlayPause);
                                }
                                let stop = egui::Button::new(
                                    RichText::new(format!("{}  Stop", icon::STOP))
                                        .color(theme::ACCENT_TEXT),
                                )
                                .fill(theme::ACCENT)
                                .min_size(egui::vec2(70.0, 34.0));
                                if ui
                                    .add(stop)
                                    .on_hover_text("Stop and restore the scene (Shift+Esc)")
                                    .clicked()
                                {
                                    ops.push(Op::Stop);
                                }
                            });
                        });
                    }
                }
                if state != SimulationState::Playing {
                    theme::panel_frame().inner_margin(4).show(ui, |ui| {
                        if icon_button(ui, icon::SIDEBAR_SIMPLE, ed.ui.show_outliner, "Outliner")
                            .clicked()
                        {
                            ed.ui.show_outliner = !ed.ui.show_outliner;
                        }
                    });
                }
            });
        });
}

pub fn view_cluster(ctx: &egui::Context, world: &mut World, ed: &mut Editor, ops: &mut Vec<Op>) {
    egui::Area::new(egui::Id::new("view_cluster"))
        .anchor(Align2::LEFT_BOTTOM, [theme::GUTTER, -theme::GUTTER])
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                theme::panel_frame().inner_margin(4).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let cam = ed.camera;
                        let fov = world
                            .get::<Camera3d>(cam)
                            .map_or(60.0, |c| c.fov_y.to_degrees());
                        let mut new = fov;
                        if ui
                            .button(icon::MINUS)
                            .on_hover_text("Narrower view (telephoto)")
                            .clicked()
                        {
                            new -= 5.0;
                        }
                        ui.add(
                            egui::DragValue::new(&mut new)
                                .range(10.0..=120.0)
                                .suffix("°")
                                .speed(0.3),
                        )
                        .on_hover_text("Viewport field of view");
                        if ui.button(icon::PLUS).on_hover_text("Wider view").clicked() {
                            new += 5.0;
                        }
                        if new != fov
                            && let Some(mut c) = world.get_mut::<Camera3d>(cam)
                        {
                            c.fov_y = new.clamp(10.0, 120.0).to_radians();
                        }
                    });
                });
                theme::panel_frame().inner_margin(4).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(
                                Op::Undo.enabled(world, ed),
                                egui::Button::new(RichText::new(icon::ARROW_U_UP_LEFT).size(15.0)),
                            )
                            .on_hover_text("Undo (Ctrl+Z)")
                            .clicked()
                        {
                            ops.push(Op::Undo);
                        }
                        if ui
                            .add_enabled(
                                Op::Redo.enabled(world, ed),
                                egui::Button::new(RichText::new(icon::ARROW_U_UP_RIGHT).size(15.0)),
                            )
                            .on_hover_text("Redo (Ctrl+Shift+Z)")
                            .clicked()
                        {
                            ops.push(Op::Redo);
                        }
                    });
                });
            });
        });
}

pub fn stats_corner(ctx: &egui::Context, world: &mut World, ed: &mut Editor) {
    let stats = world.resource::<EngineStats>().clone();
    let objects = {
        let mut q = world.query::<&MeshRenderer>();
        q.iter(world).count()
    };
    egui::Area::new(egui::Id::new("stats"))
        .anchor(Align2::RIGHT_BOTTOM, [-theme::GUTTER, -theme::GUTTER])
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ed.ui.overlays.stats {
                    let gpu = stats
                        .gpu_ms
                        .map_or_else(|| "n/a".into(), |g| format!("{g:.2}"));
                    // Short adapter name: "NVIDIA GeForce RTX 3050 Laptop GPU" → "RTX 3050 Laptop GPU".
                    let adapter = stats
                        .adapter
                        .trim_start_matches("NVIDIA ")
                        .trim_start_matches("GeForce ")
                        .trim_start_matches("AMD ");
                    let text = format!(
                        "{}  {} · {} · CPU {:.2} ms · GPU {} ms · UI {:.2} ms · {:.0} fps · {} objects",
                        icon::GRAPHICS_CARD,
                        if adapter.is_empty() { "GPU" } else { adapter },
                        stats.backend,
                        stats.cpu_ms,
                        gpu,
                        ed.ui_ms,
                        stats.fps,
                        objects
                    );
                    egui::Frame::new()
                        .fill(egui::Color32::from_black_alpha(150))
                        .corner_radius(egui::CornerRadius::same(10))
                        .inner_margin(egui::Margin::symmetric(10, 4))
                        .show(ui, |ui| {
                            ui.label(RichText::new(text).small().color(theme::TEXT));
                        })
                        .response
                        .on_hover_text(
                            "CPU: whole frame time. GPU: measured with timestamp queries (on this \
                             laptop fps is capped by the display, so judge cost by GPU ms).",
                        );
                }
                theme::panel_frame().inner_margin(3).show(ui, |ui| {
                    if icon_button(ui, icon::INFO, ed.ui.overlays.stats, "Statistics overlay")
                        .clicked()
                    {
                        ed.ui.overlays.stats = !ed.ui.overlays.stats;
                    }
                });
                theme::panel_frame().inner_margin(3).show(ui, |ui| {
                    if icon_button(
                        ui,
                        icon::QUESTION,
                        ed.ui.help_open,
                        "Keyboard shortcuts (F1)",
                    )
                    .clicked()
                    {
                        ed.ui.help_open = !ed.ui.help_open;
                    }
                });
            });
        });
}

pub fn toast(ctx: &egui::Context, ed: &mut Editor) {
    let Some((text, at)) = &ed.ui.toast else {
        return;
    };
    let age = ed.now - at;
    if age > 2.5 {
        ed.ui.toast = None;
        return;
    }
    let alpha = (1.0 - ((age - 2.0) / 0.5).clamp(0.0, 1.0)) as f32;
    let text = text.clone();
    egui::Area::new(egui::Id::new("toast"))
        .anchor(Align2::CENTER_BOTTOM, [0.0, -theme::GUTTER - 64.0])
        .interactable(false)
        .show(ctx, |ui| {
            ui.set_opacity(alpha);
            theme::panel_frame().show(ui, |ui| {
                ui.label(RichText::new(text).color(theme::TEXT));
            });
        });
    ctx.request_repaint();
}

/// Shift+A: the Add menu at the mouse.
pub fn add_menu_popup(ctx: &egui::Context, ed: &mut Editor, ops: &mut Vec<Op>) {
    let Some(at) = ed.ui.add_menu_at else { return };
    let area = egui::Area::new(egui::Id::new("add_popup"))
        .fixed_pos(at)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            theme::panel_frame().show(ui, |ui| {
                ui.label(RichText::new("Add").strong());
                ui.separator();
                add_menu_contents(ui, ops);
            });
        });
    let clicked_outside = ctx.input(|i| i.pointer.any_pressed())
        && ctx
            .pointer_interact_pos()
            .is_some_and(|p| !area.response.rect.contains(p))
        && !ctx.any_popup_open();
    if !ops.is_empty() || clicked_outside || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        ed.ui.add_menu_at = None;
    }
}

/// F3: search every operator and every setting (Blender's menu search).
pub fn search(ctx: &egui::Context, world: &mut World, ed: &mut Editor, ops: &mut Vec<Op>) {
    let Some(query) = ed.ui.search.clone() else {
        return;
    };
    enum Hit {
        Op(Op),
        Setting(&'static settings::Setting),
    }
    let q = query.to_lowercase();
    let mut hits: Vec<(String, String, Hit)> = Vec::new();
    for op in Op::searchable() {
        if op.enabled(world, ed) && op.label().to_lowercase().contains(&q) {
            hits.push((op.label(), op.shortcut().to_owned(), Hit::Op(op)));
        }
    }
    for s in settings::all_settings() {
        if s.label.to_lowercase().contains(&q) || s.id.contains(&q) {
            let place = setting_place(s.id);
            let badge = match s.status {
                settings::Status::Live => "live".to_owned(),
                settings::Status::Planned(m, _) => m.to_owned(),
                settings::Status::NotApplicable(_) => "n/a".to_owned(),
            };
            hits.push((format!("{place} › {}", s.label), badge, Hit::Setting(s)));
        }
    }
    hits.truncate(14);
    ed.ui.search_selected = ed.ui.search_selected.min(hits.len().saturating_sub(1));

    let mut chosen = None;
    let mut close = false;
    egui::Area::new(egui::Id::new("search"))
        .anchor(Align2::CENTER_TOP, [0.0, 110.0])
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            theme::panel_frame().show(ui, |ui| {
                ui.set_width(460.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::MAGNIFYING_GLASS).color(theme::ACCENT));
                    let mut text = query.clone();
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut text)
                            .hint_text("Search actions and settings…")
                            .desired_width(f32::INFINITY),
                    );
                    r.request_focus();
                    if text != query {
                        ed.ui.search = Some(text);
                        ed.ui.search_selected = 0;
                    }
                });
                ui.separator();
                if hits.is_empty() {
                    ui.label(RichText::new("No matches").color(theme::TEXT_MUTED));
                }
                for (i, (label, badge, _)) in hits.iter().enumerate() {
                    let selected = i == ed.ui.search_selected;
                    let r = ui.horizontal(|ui| {
                        let r = ui.selectable_label(selected, label);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(badge).small().color(theme::TEXT_MUTED));
                        });
                        r
                    });
                    if r.inner.clicked() {
                        chosen = Some(i);
                    }
                }
            });
        });
    ctx.input(|i| {
        if i.key_pressed(egui::Key::ArrowDown) {
            ed.ui.search_selected = (ed.ui.search_selected + 1).min(hits.len().saturating_sub(1));
        }
        if i.key_pressed(egui::Key::ArrowUp) {
            ed.ui.search_selected = ed.ui.search_selected.saturating_sub(1);
        }
        if i.key_pressed(egui::Key::Enter) && !hits.is_empty() {
            chosen = Some(ed.ui.search_selected);
        }
        if i.key_pressed(egui::Key::Escape) {
            close = true;
        }
    });
    if let Some(i) = chosen {
        match &hits[i].2 {
            Hit::Op(op) => ops.push(*op),
            Hit::Setting(s) => {
                ed.ui.tab = setting_tab(s.id);
                ed.ui.show_properties = true;
                if !matches!(s.status, settings::Status::Live) && settings_is_advanced(s.id) {
                    ed.ui.show_advanced = true;
                }
                ed.ui.focus_setting = Some((s.id, ed.now));
            }
        }
        close = true;
    }
    if close {
        ed.ui.search = None;
    }
}

fn setting_tab(id: &str) -> Tab {
    if id.starts_with("world.") {
        Tab::World
    } else if id.starts_with("light.") {
        Tab::Data
    } else if id.starts_with("material.") {
        Tab::Material
    } else {
        Tab::Render
    }
}

fn setting_place(id: &str) -> &'static str {
    match setting_tab(id) {
        Tab::World => "World",
        Tab::Data => "Light",
        Tab::Material => "Material",
        _ => "Render",
    }
}

fn settings_is_advanced(id: &str) -> bool {
    settings::all_panels().any(|p| {
        p.advanced
            && (p.settings.iter().any(|s| s.id == id)
                || p.header_toggle.is_some_and(|t| t.id == id))
    }) || settings::find(id).is_some_and(|s| matches!(s.status, settings::Status::NotApplicable(_)))
}

pub fn windows(ctx: &egui::Context, world: &mut World, ed: &mut Editor, ops: &mut Vec<Op>) {
    let mut open = ed.ui.help_open;
    egui::Window::new(format!("{}  Keyboard Shortcuts", icon::KEYBOARD))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            egui::Grid::new("shortcuts")
                .num_columns(2)
                .spacing([24.0, 6.0])
                .striped(true)
                .show(ui, |ui| {
                    let rows: &[(&str, &str)] = &[
                        ("Shift A", "Add menu at the mouse"),
                        ("Q / G / R / S", "Select / Move / Rotate / Scale tool"),
                        ("Ctrl while dragging", "Snap"),
                        ("X, Delete", "Delete selected"),
                        ("Shift D", "Duplicate"),
                        ("H / Alt H", "Hide selected / unhide all"),
                        ("F, Numpad .", "Frame selected"),
                        ("1 / 3 / 7 (+Ctrl)", "Front / right / top view (opposite)"),
                        ("Left-drag empty space", "Orbit around the scene"),
                        ("Space + left-drag", "Pan"),
                        ("Wheel", "Zoom"),
                        (
                            "MMB / Shift+MMB / Ctrl+MMB",
                            "Orbit / pan / zoom (Blender keys)",
                        ),
                        ("Ctrl Z / Ctrl Shift Z", "Undo / redo"),
                        ("N", "Toggle properties panel"),
                        ("F3", "Search actions and settings"),
                        ("Alt P", "Play / pause"),
                        ("Shift Esc", "Stop playing"),
                        ("F1", "This sheet"),
                    ];
                    for (key, what) in rows {
                        ui.label(RichText::new(*key).monospace().color(theme::ACCENT));
                        ui.label(*what);
                        ui.end_row();
                    }
                });
        });
    ed.ui.help_open = open;

    let mut open = ed.ui.prefs_open;
    egui::Window::new(format!("{}  Preferences", icon::GEAR))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("UI scale");
                ui.add(egui::Slider::new(&mut ed.ui.ui_scale, 0.75..=2.0).step_by(0.05));
            });
            ui.label(
                RichText::new(
                    "Preferences last for this session; saving them comes with config files (M6).",
                )
                .small()
                .color(theme::TEXT_MUTED),
            );
        });
    ed.ui.prefs_open = open;

    let mut open = ed.ui.history_open;
    egui::Window::new(format!("{}  Undo History", icon::STACK))
        .open(&mut open)
        .collapsible(false)
        .default_width(260.0)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            let (labels, applied) = ed.history.labels();
            if labels.is_empty() {
                ui.label(RichText::new("Nothing to undo yet.").color(theme::TEXT_MUTED));
            }
            egui::ScrollArea::vertical()
                .max_height(320.0)
                .show(ui, |ui| {
                    ui.label(RichText::new("Original").color(theme::TEXT_MUTED));
                    for (i, label) in labels.iter().enumerate() {
                        let done = i < applied;
                        let text = RichText::new(label).color(if done {
                            theme::TEXT
                        } else {
                            theme::TEXT_MUTED
                        });
                        if ui.selectable_label(i + 1 == applied, text).clicked() {
                            // Step the history to just after this entry.
                            let target = i + 1;
                            for _ in target..applied {
                                ops.push(Op::Undo);
                            }
                            for _ in applied..target {
                                ops.push(Op::Redo);
                            }
                        }
                    }
                });
        });
    ed.ui.history_open = open;

    let mut open = ed.ui.about_open;
    egui::Window::new("About")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(RichText::new("Game_Engine Editor").heading());
            ui.label("An Unreal-style engine in Rust, with an editor modelled on Blender's workflow.");
            ui.label(
                RichText::new("The Render tab mirrors EEVEE 5.2's settings so you can learn them: green dots are live, amber milestones are coming.")
                    .color(theme::TEXT_MUTED),
            );
        });
    ed.ui.about_open = open;
    let _ = world;
}
