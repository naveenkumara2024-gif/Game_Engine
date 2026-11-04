//! Outliner (right sidebar): the scene hierarchy with visibility toggles, rename and a context
//! menu. Editor-only and disabled (deleted, stashed) entities are hidden.

use egui_phosphor::regular as icon;
use ge_ecs::prelude::*;
use ge_render::Visibility;

use crate::actions::{Op, set_visible};
use crate::state::{Editor, EditorOnly, describe};
use crate::theme;

pub fn draw(ui: &mut egui::Ui, world: &mut World, ed: &mut Editor) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(icon::TREE_STRUCTURE).color(theme::ACCENT));
        ui.label(egui::RichText::new("Outliner").strong());
    });
    ui.separator();
    let mut roots =
        world.query_filtered::<Entity, (With<Transform>, Without<ChildOf>, Without<EditorOnly>)>();
    let mut roots: Vec<Entity> = roots.iter(world).collect();
    roots.sort_by_key(|e| describe(world, *e).0.to_lowercase());

    let mut ops = Vec::new();
    egui::ScrollArea::vertical()
        .auto_shrink([false, true])
        .max_height(ui.available_height())
        .show(ui, |ui| {
            for root in roots {
                node(ui, world, ed, root, 0, &mut ops);
            }
        });
    for op in ops {
        op.run(world, ed);
    }
}

fn node(
    ui: &mut egui::Ui,
    world: &mut World,
    ed: &mut Editor,
    e: Entity,
    depth: usize,
    ops: &mut Vec<Op>,
) {
    let (name, glyph) = describe(world, e);
    let children: Vec<Entity> = world
        .get::<Children>(e)
        .map(|c| {
            c.iter()
                .copied()
                .filter(|c| world.get::<Disabled>(*c).is_none())
                .collect::<Vec<Entity>>()
        })
        .unwrap_or_default();
    let visible = world.get::<Visibility>(e).is_none_or(|v| v.visible);
    let selected = ed.selected == Some(e);
    let id = ui.make_persistent_id(("outliner", e));
    let state =
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);

    let row = |ui: &mut egui::Ui, world: &mut World, ed: &mut Editor, ops: &mut Vec<Op>| {
        ui.horizontal(|ui| {
            ui.add_space(depth as f32 * 4.0);
            let renaming = ed.ui.rename.as_ref().is_some_and(|(re, _)| *re == e);
            if renaming {
                let mut done = false;
                if let Some((_, text)) = &mut ed.ui.rename {
                    let r = ui.add(egui::TextEdit::singleline(text).desired_width(140.0));
                    r.request_focus();
                    done = r.lost_focus();
                }
                if done && let Some((_, text)) = ed.ui.rename.take() {
                    rename(world, ed, e, text);
                }
            } else {
                let text = egui::RichText::new(format!("{glyph}  {name}")).color(if visible {
                    theme::TEXT
                } else {
                    theme::TEXT_MUTED
                });
                let r = ui.add(egui::Button::selectable(selected, text).frame_when_inactive(false));
                if r.clicked() {
                    ed.selected = Some(e);
                }
                if r.double_clicked() {
                    ed.ui.rename = Some((e, name.clone()));
                }
                r.context_menu(|ui| {
                    ed.selected = Some(e);
                    if ui
                        .button(format!("{} Rename", icon::PENCIL_SIMPLE))
                        .clicked()
                    {
                        ed.ui.rename = Some((e, name.clone()));
                        ui.close();
                    }
                    if ui.button(format!("{} Duplicate", icon::COPY)).clicked() {
                        ops.push(Op::Duplicate);
                        ui.close();
                    }
                    if ui.button(format!("{} Frame", icon::CORNERS_OUT)).clicked() {
                        ops.push(Op::FrameSelected);
                        ui.close();
                    }
                    if ui.button(format!("{} Delete", icon::TRASH)).clicked() {
                        ops.push(Op::Delete);
                        ui.close();
                    }
                });
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let eye = if visible { icon::EYE } else { icon::EYE_SLASH };
                if ui
                    .add(egui::Button::new(eye).frame(false))
                    .on_hover_text("Show / hide in viewport (H, Alt+H)")
                    .clicked()
                {
                    set_visible(world, ed, e, !visible);
                }
            });
        });
    };

    if children.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(18.0);
            row(ui, world, ed, ops);
        });
    } else {
        state
            .show_header(ui, |ui| row(ui, world, ed, ops))
            .body(|ui| {
                for child in children {
                    node(ui, world, ed, child, depth + 1, ops);
                }
            });
    }
}

fn rename(world: &mut World, ed: &mut Editor, e: Entity, text: String) {
    let text = text.trim().to_owned();
    if text.is_empty() {
        return;
    }
    let before = world.get::<Name>(e).cloned();
    let after = Name::new(text);
    match before {
        Some(before) if before != after => ed.history.execute(
            world,
            Box::new(crate::commands::SetComponent {
                entity: e,
                before,
                after,
                label: "Rename",
            }),
            None,
        ),
        Some(_) => {}
        None => ed.history.execute(
            world,
            Box::new(crate::commands::AddComponent {
                entity: e,
                value: after,
                label: "Rename",
            }),
            None,
        ),
    }
}
