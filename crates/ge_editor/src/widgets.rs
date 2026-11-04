//! Property-row widgets shared by the properties tabs: undoable component/resource/cvar edits
//! and schema-driven rows with Live / Planned badges and EEVEE tooltips.

use ge_ecs::prelude::*;
use ge_render::settings::{Kind, Panel, Setting, Status};

use crate::commands::{SetCVar, SetComponent, SetResource};
use crate::state::Editor;
use crate::theme;

/// Width of the label column in property rows.
pub const LABEL_WIDTH: f32 = 112.0;

/// Edits a copy of component `T` on `entity` with `edit`; if it changed, writes it back and
/// records an undo step that merges while the widget stays active.
pub fn edit_component<T, F>(
    ui: &mut egui::Ui,
    world: &mut World,
    ed: &mut Editor,
    entity: Entity,
    label: &'static str,
    edit: F,
) -> Option<egui::Response>
where
    T: Component + Clone + PartialEq,
    F: FnOnce(&mut egui::Ui, &mut T) -> egui::Response,
{
    let before = world.get::<T>(entity)?.clone();
    let mut value = before.clone();
    let response = edit(ui, &mut value);
    track(ed, &response);
    if value != before {
        world.entity_mut(entity).insert(value.clone());
        ed.history.record(
            world,
            Box::new(SetComponent {
                entity,
                before,
                after: value,
                label,
            }),
            Some(merge_id(&response)),
        );
    }
    Some(response)
}

pub fn edit_resource<T, F>(
    ui: &mut egui::Ui,
    world: &mut World,
    ed: &mut Editor,
    label: &'static str,
    edit: F,
) -> Option<egui::Response>
where
    T: Resource + Clone + PartialEq,
    F: FnOnce(&mut egui::Ui, &mut T) -> egui::Response,
{
    let before = world.get_resource::<T>()?.clone();
    let mut value = before.clone();
    let response = edit(ui, &mut value);
    track(ed, &response);
    if value != before {
        world.insert_resource(value.clone());
        ed.history.record(
            world,
            Box::new(SetResource {
                before,
                after: value,
                label,
            }),
            Some(merge_id(&response)),
        );
    }
    Some(response)
}

fn merge_id(response: &egui::Response) -> u64 {
    response.id.value()
}

/// Keeps the undo merge window open while the widget is being dragged or typed into.
fn track(ed: &mut Editor, response: &egui::Response) {
    if response.dragged() || response.has_focus() || response.is_pointer_button_down_on() {
        ed.history.keep_open(merge_id(response));
    }
}

/// A two-column property row: muted label on the left, widget on the right.
pub fn row<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.add_sized(
            [LABEL_WIDTH, ui.spacing().interact_size.y],
            egui::Label::new(egui::RichText::new(label).color(theme::TEXT_MUTED)).truncate(),
        );
        add(ui)
    })
    .inner
}

/// `on_hover` text explaining a setting, plus where it stands in this engine.
pub fn help_text(help: &str, status: Status) -> String {
    match status {
        Status::Live => format!("{help}\n\nLive: changes the renderer now."),
        Status::Planned(m, what) => {
            format!("{help}\n\nPlanned ({m}): arrives with {what}. Shown so you can learn it.")
        }
        Status::NotApplicable(why) => format!("{help}\n\nNot planned: {why}."),
    }
}

/// A painted dot (the default fonts have no "●").
pub fn live_dot(ui: &mut egui::Ui, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.5, color);
}

/// A small colored dot (Live = green, Planned = amber) after a row.
pub fn status_badge(ui: &mut egui::Ui, status: Status) {
    let (color, text) = match status {
        Status::Live => (theme::LIVE, String::new()),
        Status::Planned(m, _) => (theme::PLANNED, m.to_owned()),
        Status::NotApplicable(_) => (theme::TEXT_MUTED, "n/a".to_owned()),
    };
    if text.is_empty() {
        live_dot(ui, color);
    } else {
        ui.label(egui::RichText::new(text).color(color).small());
    }
}

/// One schema row. Live rows edit their cvar (undoable); others show Blender's default,
/// disabled, with a tooltip.
pub fn schema_setting(ui: &mut egui::Ui, world: &mut World, ed: &mut Editor, s: &Setting) {
    let focused = ed.ui.focus_setting.is_some_and(|(id, _)| id == s.id);
    let live = s.status == Status::Live;
    let cvar = s.cvar.and_then(ge_core::cvar::find);
    let current = cvar.map(|c| c.value_string());

    let inner = ui.horizontal(|ui| {
        let label =
            egui::RichText::new(s.label).color(if live { theme::TEXT } else { theme::TEXT_MUTED });
        ui.add_sized(
            [LABEL_WIDTH, ui.spacing().interact_size.y],
            egui::Label::new(label).truncate(),
        );
        let response = ui
            .add_enabled_ui(live, |ui| setting_widget(ui, s, current.as_deref()))
            .inner;
        status_badge(ui, s.status);
        response
    });
    let (response, new_value) = inner.inner;
    let row_rect = inner.response.rect;
    inner.response.on_hover_text(help_text(s.help, s.status));
    if let Some(r) = &response {
        track(ed, r);
    }
    if focused {
        ui.scroll_to_rect(row_rect, Some(egui::Align::Center));
        ui.painter().rect_stroke(
            row_rect.expand(2.0),
            egui::CornerRadius::same(4),
            egui::Stroke::new(1.5, theme::ACCENT),
            egui::StrokeKind::Outside,
        );
    }
    if let (Some(name), Some(before), Some(after), Some(r)) = (s.cvar, current, new_value, response)
    {
        let edit = Box::new(SetCVar {
            name,
            before,
            after,
            label: format!("Set {}", s.label),
        });
        ed.history.execute(world, edit, Some(r.id.value()));
    }
}

/// Draws the widget for a setting. Returns the response and, for live rows, the new cvar
/// text when the user changed it.
fn setting_widget(
    ui: &mut egui::Ui,
    s: &Setting,
    current: Option<&str>,
) -> (Option<egui::Response>, Option<String>) {
    match s.kind {
        Kind::Bool(default) => {
            let mut v = current.map_or(default, |c| c == "1" || c == "true");
            let before = v;
            let r = ui.checkbox(&mut v, "");
            let changed = (v != before).then(|| if v { "1" } else { "0" }.to_owned());
            (Some(r), changed)
        }
        Kind::Float {
            min,
            max,
            default,
            unit,
        } => {
            let mut v: f32 = current.and_then(|c| c.parse().ok()).unwrap_or(default);
            let before = v;
            let speed = ((max - min) / 400.0).clamp(0.001, 50.0);
            let r = ui.add(
                egui::DragValue::new(&mut v)
                    .range(min..=max)
                    .speed(speed)
                    .suffix(unit.suffix())
                    .max_decimals(3),
            );
            let changed = (v != before).then(|| v.to_string());
            (Some(r), changed)
        }
        Kind::Int { min, max, default } => {
            let mut v: i32 = current.and_then(|c| c.parse().ok()).unwrap_or(default);
            let before = v;
            let r = ui.add(egui::DragValue::new(&mut v).range(min..=max).speed(0.2));
            let changed = (v != before).then(|| v.to_string());
            (Some(r), changed)
        }
        Kind::Enum(options, default) => {
            let mut index: usize = current
                .and_then(|c| c.parse::<usize>().ok())
                .unwrap_or(default)
                .min(options.len().saturating_sub(1));
            let before = index;
            let r = egui::ComboBox::from_id_salt(s.id)
                .selected_text(options[index])
                .width(150.0)
                .show_ui(ui, |ui| {
                    for (i, option) in options.iter().enumerate() {
                        ui.selectable_value(&mut index, i, *option);
                    }
                })
                .response;
            let changed = (index != before).then(|| index.to_string());
            (Some(r), changed)
        }
    }
}

/// Draws a schema panel and its sub-panels (Blender's collapsible panels with optional header
/// checkbox). `panels` is the whole table; children are found by `parent`.
pub fn schema_panels(
    ui: &mut egui::Ui,
    world: &mut World,
    ed: &mut Editor,
    panels: &'static [Panel],
    parent: Option<&'static str>,
) {
    for panel in panels.iter().filter(|p| p.parent == parent) {
        if panel.advanced && !ed.ui.show_advanced {
            continue;
        }
        let id = ui.make_persistent_id(panel.id);
        let mut state =
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
        let contains_focus = ed
            .ui
            .focus_setting
            .is_some_and(|(sid, _)| subtree_has(panels, panel, sid));
        if contains_focus {
            state.set_open(true);
        }
        let header = state.show_header(ui, |ui| {
            if let Some(toggle) = panel.header_toggle.filter(|t| t.status == Status::Live) {
                let r = ui
                    .add_enabled_ui(toggle.status == Status::Live, |ui| {
                        let current = toggle
                            .cvar
                            .and_then(ge_core::cvar::find)
                            .map(|c| c.value_string());
                        setting_widget(ui, &toggle, current.as_deref())
                    })
                    .inner;
                if let (Some(name), (Some(resp), Some(after))) = (toggle.cvar, r)
                    && let Some(before) = ge_core::cvar::find(name).map(|c| c.value_string())
                {
                    ed.history.execute(
                        world,
                        Box::new(SetCVar {
                            name,
                            before,
                            after,
                            label: format!("Toggle {}", toggle.label),
                        }),
                        Some(resp.id.value()),
                    );
                }
            }
            let label = ui
                .label(egui::RichText::new(panel.label).strong())
                .on_hover_text(panel.help);
            // A toggle that isn't live yet: show its milestone instead of a dead checkbox.
            if let Some(toggle) = panel.header_toggle.filter(|t| t.status != Status::Live) {
                status_badge(ui, toggle.status);
                label.on_hover_text(help_text(toggle.help, toggle.status));
            }
        });
        header.body(|ui| {
            for setting in panel.settings {
                if matches!(setting.status, Status::NotApplicable(_)) && !ed.ui.show_advanced {
                    continue;
                }
                schema_setting(ui, world, ed, setting);
            }
            schema_panels(ui, world, ed, panels, Some(panel.id));
        });
    }
}

fn subtree_has(panels: &'static [Panel], panel: &Panel, setting_id: &str) -> bool {
    panel
        .settings
        .iter()
        .chain(panel.header_toggle.iter())
        .any(|s| s.id == setting_id)
        || panels
            .iter()
            .filter(|p| p.parent == Some(panel.id))
            .any(|p| subtree_has(panels, p, setting_id))
}

/// A disabled row for a setting that exists in Blender but isn't wired up yet, with the same
/// tooltip/badge treatment as schema rows (for hand-built tabs).
pub fn planned_row(ui: &mut egui::Ui, label: &str, value: &str, help: &str, status: Status) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [LABEL_WIDTH, ui.spacing().interact_size.y],
            egui::Label::new(egui::RichText::new(label).color(theme::TEXT_MUTED)).truncate(),
        );
        ui.add_enabled(
            false,
            egui::Button::new(value).min_size(egui::vec2(120.0, 0.0)),
        );
        status_badge(ui, status);
    })
    .response
    .on_hover_text(help_text(help, status));
}

/// A collapsible section for hand-built tabs (same look as schema panels).
pub fn section(
    ui: &mut egui::Ui,
    id: &str,
    label: &str,
    default_open: bool,
    body: impl FnOnce(&mut egui::Ui),
) {
    let id = ui.make_persistent_id(id);
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, default_open)
        .show_header(ui, |ui| {
            ui.label(egui::RichText::new(label).strong());
        })
        .body(body);
}

/// Linear-RGB color picker (egui edits sRGB-gamma by default; our data is linear).
pub fn color_linear(ui: &mut egui::Ui, color: &mut Vec3) -> egui::Response {
    let mut rgb = color.to_array();
    let r = egui::color_picker::color_edit_button_rgb(ui, &mut rgb);
    *color = Vec3::from(rgb);
    r
}
