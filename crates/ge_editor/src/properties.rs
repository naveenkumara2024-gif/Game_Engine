//! The properties editor (left panel): Blender's tab strip + context-sensitive panels
//! (finding B2). Tabs that don't apply to the selection are hidden — the main tool for keeping
//! the UI calm. Render-tab rows come from the `ge_render::settings` schema; the object, light,
//! camera, mesh and material tabs are hand-built until reflection (M6).

use egui_phosphor::regular as icon;
use ge_ecs::prelude::*;
use ge_render::lighting::{R_VIEWPORT_SHADING, blackbody};
use ge_render::settings::{self, Status};
use ge_render::{
    Camera3d, Light, LightKind, Material, MeshKind, MeshRenderer, Primitive, PrimitiveMesh,
    Visibility, WorldSettings,
};

use crate::commands::{AddComponent, SetComponent};
use crate::state::{Editor, Tab, ViewportDisplay, describe};
use crate::theme;
use crate::widgets::{
    color_linear, edit_component, edit_resource, planned_row, row, schema_panels, section,
};

/// Which tabs exist for the current selection, in Blender's order.
pub fn tabs(world: &World, selected: Option<Entity>) -> Vec<Tab> {
    let mut tabs = vec![Tab::Render, Tab::World];
    if let Some(e) = selected {
        tabs.push(Tab::Object);
        let mesh = world.get::<MeshRenderer>(e).is_some();
        if mesh || world.get::<Light>(e).is_some() || world.get::<Camera3d>(e).is_some() {
            tabs.push(Tab::Data);
        }
        if mesh {
            tabs.push(Tab::Material);
        }
    }
    tabs
}

fn tab_icon(world: &World, selected: Option<Entity>, tab: Tab) -> (&'static str, &'static str) {
    match tab {
        Tab::Render => (icon::CAMERA, "Render"),
        Tab::World => (icon::GLOBE, "World"),
        Tab::Object => (icon::BOUNDING_BOX, "Object"),
        Tab::Material => (icon::CIRCLE_HALF, "Material"),
        Tab::Data => match selected {
            Some(e) if world.get::<Light>(e).is_some() => (icon::LIGHTBULB, "Light"),
            Some(e) if world.get::<Camera3d>(e).is_some() => (icon::VIDEO_CAMERA, "Camera"),
            _ => (icon::TRIANGLE, "Mesh"),
        },
    }
}

pub fn draw(ui: &mut egui::Ui, world: &mut World, ed: &mut Editor) {
    let available = tabs(world, ed.selected);
    if !available.contains(&ed.ui.tab) {
        ed.ui.tab = Tab::Render;
    }
    ui.horizontal_top(|ui| {
        // Vertical icon strip.
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            for tab in &available {
                let (glyph, name) = tab_icon(world, ed.selected, *tab);
                let selected = ed.ui.tab == *tab;
                let button = egui::Button::new(egui::RichText::new(glyph).size(17.0))
                    .min_size(egui::vec2(30.0, 30.0))
                    .selected(selected);
                if ui.add(button).on_hover_text(name).clicked() {
                    ed.ui.tab = *tab;
                }
            }
        });
        ui.add_space(4.0);
        ui.vertical(|ui| {
            ui.set_width(318.0);
            egui::ScrollArea::vertical()
                .auto_shrink([false, true])
                .show(ui, |ui| match ed.ui.tab {
                    Tab::Render => render_tab(ui, world, ed),
                    Tab::World => world_tab(ui, world, ed),
                    Tab::Object => object_tab(ui, world, ed),
                    Tab::Data => data_tab(ui, world, ed),
                    Tab::Material => material_tab(ui, world, ed),
                });
        });
    });
}

fn heading(ui: &mut egui::Ui, glyph: &str, text: &str) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(glyph).size(16.0).color(theme::ACCENT));
        ui.label(egui::RichText::new(text).heading());
    });
    ui.add_space(2.0);
}

fn legend(ui: &mut egui::Ui, ed: &mut Editor) {
    ui.horizontal(|ui| {
        crate::widgets::live_dot(ui, theme::LIVE);
        ui.label(egui::RichText::new("Live").small().color(theme::TEXT_MUTED));
        ui.label(egui::RichText::new("M4").color(theme::PLANNED).small());
        ui.label(
            egui::RichText::new("Planned (hover for details)")
                .small()
                .color(theme::TEXT_MUTED),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.toggle_value(&mut ed.ui.show_advanced, "Show advanced")
                .on_hover_text("Also show advanced panels and settings we don't plan to support");
        });
    });
    ui.separator();
}

fn render_tab(ui: &mut egui::Ui, world: &mut World, ed: &mut Editor) {
    heading(ui, icon::CAMERA, "Render");
    row(ui, "Render Engine", |ui| {
        ui.add_enabled_ui(false, |ui| {
            egui::ComboBox::from_id_salt("render_engine")
                .selected_text("Forward+ (EEVEE-style)")
                .width(170.0)
                .show_ui(ui, |_| {});
        })
        .response
        .on_disabled_hover_text(
            "Our renderer: a forward pass with per-pixel lights, modelled on EEVEE's settings. \
             Clustered forward+ with shadows arrives in M4.",
        );
    });
    shading_hint(ui);
    legend(ui, ed);
    schema_panels(ui, world, ed, settings::RENDER_PANELS, None);
}

/// Lights only show in Rendered shading; say so where it matters.
fn shading_hint(ui: &mut egui::Ui) {
    if R_VIEWPORT_SHADING.get() != 1 {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!(
                    "{} Solid shading ignores scene lights.",
                    icon::INFO
                ))
                .small()
                .color(theme::PLANNED),
            );
            if ui.small_button("Switch to Rendered").clicked() {
                R_VIEWPORT_SHADING.set(1);
            }
        });
    }
}

fn world_tab(ui: &mut egui::Ui, world: &mut World, ed: &mut Editor) {
    heading(ui, icon::GLOBE, "World");
    shading_hint(ui);
    legend(ui, ed);
    if !world.contains_resource::<WorldSettings>() {
        world.init_resource::<WorldSettings>();
    }
    section(ui, "world.surface", "Surface", true, |ui| {
        row(ui, "Color", |ui| {
            edit_resource::<WorldSettings, _>(ui, world, ed, "World Color", |ui, w| {
                color_linear(ui, &mut w.color)
            })
        })
        .map(|r| {
            r.on_hover_text(
                "The world's color: the background, and (in Rendered shading) light arriving \
                 from every direction. A darker world makes your lamps stand out.",
            )
        });
        row(ui, "Strength", |ui| {
            edit_resource::<WorldSettings, _>(ui, world, ed, "World Strength", |ui, w| {
                ui.add(
                    egui::DragValue::new(&mut w.strength)
                        .range(0.0..=100.0)
                        .speed(0.01),
                )
            })
        })
        .map(|r| r.on_hover_text("Brightness multiplier of the world light and background."));
    });
    schema_panels(ui, world, ed, settings::WORLD_PANELS, None);
}

fn name_header(ui: &mut egui::Ui, world: &mut World, ed: &mut Editor, e: Entity) {
    let (_, glyph) = describe(world, e);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(glyph).size(16.0).color(theme::ACCENT));
        if world.get::<Name>(e).is_none() {
            let fallback = describe(world, e).0;
            world.entity_mut(e).insert(Name::new(fallback));
        }
        edit_component::<Name, _>(ui, world, ed, e, "Rename", |ui, name| {
            let mut text = name.as_str().to_owned();
            let r = ui.add(egui::TextEdit::singleline(&mut text).desired_width(240.0));
            if text != name.as_str() && !text.trim().is_empty() {
                name.set(text);
            }
            r
        });
    });
    ui.add_space(2.0);
}

fn vec3_row(
    ui: &mut egui::Ui,
    label: &str,
    v: &mut Vec3,
    speed: f32,
    suffix: &str,
) -> egui::Response {
    row(ui, label, |ui| {
        let mut response: Option<egui::Response> = None;
        for (axis, value, color) in [
            ("X", &mut v.x, theme::AXIS_X),
            ("Y", &mut v.y, theme::AXIS_Y),
            ("Z", &mut v.z, theme::AXIS_Z),
        ] {
            ui.label(egui::RichText::new(axis).color(color).small());
            let r = ui.add_sized(
                [56.0, 20.0],
                egui::DragValue::new(value)
                    .speed(speed)
                    .suffix(suffix)
                    .max_decimals(3),
            );
            response = Some(match response {
                Some(prev) => prev.union(r),
                None => r,
            });
        }
        response.expect("three axes")
    })
}

fn object_tab(ui: &mut egui::Ui, world: &mut World, ed: &mut Editor) {
    let Some(e) = ed.selected else { return };
    name_header(ui, world, ed, e);

    if world.get::<ge_render::SceneRoot>(e).is_some() {
        section(ui, "object.model", "Model Instance", true, |ui| {
            model_source_rows(ui, world, e);
            ui.label(
                egui::RichText::new(
                    "Its parts are children (expand it in the Outliner). Re-importing replaces \
                     them, so edit the instance's transform here, not the parts.",
                )
                .small()
                .color(theme::TEXT_MUTED),
            );
        });
    }

    section(ui, "object.transform", "Transform", true, |ui| {
        edit_component::<Transform, _>(ui, world, ed, e, "Move", |ui, t| {
            vec3_row(ui, "Location", &mut t.translation, 0.02, " m")
        });
        // Euler angles are cached per entity so editing one axis never flips the others.
        let quat = world
            .get::<Transform>(e)
            .map_or(Quat::IDENTITY, |t| t.rotation);
        let euler = match ed.ui.euler_cache {
            Some((ce, cq, ev)) if ce == e && cq.abs_diff_eq(quat, 1e-6) => ev,
            _ => {
                let (x, y, z) = quat.to_euler(EulerRot::XYZ);
                Vec3::new(x, y, z) * (180.0 / std::f32::consts::PI)
            }
        };
        let mut degrees = euler;
        let response = vec3_row(ui, "Rotation", &mut degrees, 0.5, "°");
        if degrees != euler
            && let Some(before) = world.get::<Transform>(e).copied()
        {
            let r = degrees * (std::f32::consts::PI / 180.0);
            let mut after = before;
            after.rotation = Quat::from_euler(EulerRot::XYZ, r.x, r.y, r.z);
            world.entity_mut(e).insert(after);
            ed.history.record(
                world,
                Box::new(SetComponent {
                    entity: e,
                    before,
                    after,
                    label: "Rotate",
                }),
                Some(response.id.value()),
            );
            ed.ui.euler_cache = Some((e, after.rotation, degrees));
        } else if !matches!(ed.ui.euler_cache, Some((ce, ..)) if ce == e) {
            ed.ui.euler_cache = Some((e, quat, euler));
        }
        if response.dragged() || response.has_focus() {
            ed.history.keep_open(response.id.value());
        }
        row(ui, "Mode", |ui| {
            ui.label(egui::RichText::new("XYZ Euler").color(theme::TEXT_MUTED))
                .on_hover_text("Rotation is shown as XYZ Euler angles and stored as a quaternion.")
        });
        edit_component::<Transform, _>(ui, world, ed, e, "Scale", |ui, t| {
            vec3_row(ui, "Scale", &mut t.scale, 0.01, "")
        });
        planned_row(
            ui,
            "Delta Transform",
            "—",
            "An extra offset on top of the transform, handy for animation.",
            Status::Planned("M6", "the reflection inspector"),
        );
    });

    section(ui, "object.relations", "Relations", false, |ui| {
        let parent = world.get::<ChildOf>(e).map(|c| c.parent());
        row(ui, "Parent", |ui| match parent {
            Some(p) => {
                let (name, glyph) = describe(world, p);
                if ui.link(format!("{glyph} {name}")).clicked() {
                    ed.selected = Some(p);
                }
            }
            None => {
                ui.label(egui::RichText::new("None (scene root)").color(theme::TEXT_MUTED));
            }
        });
    });

    section(ui, "object.visibility", "Visibility", false, |ui| {
        if world.get::<Visibility>(e).is_none() {
            row(ui, "Show in Viewport", |ui| {
                let mut v = true;
                if ui.checkbox(&mut v, "").changed() {
                    ed.history.execute(
                        world,
                        Box::new(AddComponent {
                            entity: e,
                            value: Visibility { visible: v },
                            label: "Hide",
                        }),
                        None,
                    );
                }
            });
        } else {
            row(ui, "Show in Viewport", |ui| {
                edit_component::<Visibility, _>(ui, world, ed, e, "Toggle Visibility", |ui, v| {
                    ui.checkbox(&mut v.visible, "")
                })
            });
        }
    });

    section(ui, "object.display", "Viewport Display", false, |ui| {
        if world.get::<ViewportDisplay>(e).is_none() {
            world.entity_mut(e).insert(ViewportDisplay::default());
        }
        for (label, which) in [("Name", 0), ("Axes", 1), ("Bounds", 2)] {
            row(ui, label, |ui| {
                edit_component::<ViewportDisplay, _>(
                    ui,
                    world,
                    ed,
                    e,
                    "Viewport Display",
                    |ui, d| {
                        let flag = match which {
                            0 => &mut d.show_name,
                            1 => &mut d.show_axes,
                            _ => &mut d.show_bounds,
                        };
                        ui.checkbox(flag, "")
                    },
                )
            });
        }
    });
}

fn data_tab(ui: &mut egui::Ui, world: &mut World, ed: &mut Editor) {
    let Some(e) = ed.selected else { return };
    if world.get::<Light>(e).is_some() {
        light_tab(ui, world, ed, e);
    } else if world.get::<Camera3d>(e).is_some() {
        camera_tab(ui, world, ed, e);
    } else if world.get::<MeshRenderer>(e).is_some() {
        mesh_tab(ui, world, ed, e);
    }
}

fn mesh_tab(ui: &mut egui::Ui, world: &mut World, ed: &mut Editor, e: Entity) {
    heading(ui, icon::TRIANGLE, "Mesh");
    let Some(mesh) = world.get::<MeshRenderer>(e).copied() else {
        return;
    };
    match mesh.mesh {
        MeshKind::Primitive(p) => {
            section(ui, "mesh.primitive", p.shape.label(), true, |ui| {
                ui.label(
                    egui::RichText::new(
                        "Generator settings, like Blender's Adjust Last Operation panel.",
                    )
                    .small()
                    .color(theme::TEXT_MUTED),
                );
                edit_component::<MeshRenderer, _>(ui, world, ed, e, "Edit Primitive", |ui, m| {
                    let MeshKind::Primitive(pm) = &mut m.mesh else {
                        return ui.label("");
                    };
                    primitive_settings(ui, pm)
                });
            });
            section(ui, "mesh.shading", "Shading", true, |ui| {
                row(ui, "Shade", |ui| {
                    edit_component::<MeshRenderer, _>(ui, world, ed, e, "Shade Smooth", |ui, m| {
                        let MeshKind::Primitive(pm) = &mut m.mesh else {
                            return ui.label("");
                        };
                        let r1 = ui.selectable_value(&mut pm.smooth, false, "Flat");
                        let r2 = ui.selectable_value(&mut pm.smooth, true, "Smooth");
                        r1.union(r2)
                    })
                })
                .map(|r| {
                    r.on_hover_text(
                        "Flat: each face lit as one flat plane (faceted). Smooth: normals are \
                         interpolated across faces so curved surfaces catch light smoothly.",
                    )
                });
            });
            let built = p.shape.build(p.smooth);
            section(ui, "mesh.stats", "Statistics", false, |ui| {
                row(ui, "Vertices", |ui| {
                    ui.label(built.vertices.len().to_string())
                });
                row(ui, "Triangles", |ui| {
                    ui.label((built.indices.len() / 3).to_string())
                });
            });
        }
        MeshKind::Cube | MeshKind::Plane => {
            ui.label(
                egui::RichText::new(
                    "A built-in game mesh (unit size; scale it in the Object tab). Meshes added \
                     with Shift+A have generator settings here.",
                )
                .color(theme::TEXT_MUTED),
            );
        }
        MeshKind::Asset(id) => asset_mesh_info(ui, world, e, id),
    }
}

fn primitive_settings(ui: &mut egui::Ui, pm: &mut PrimitiveMesh) -> egui::Response {
    let len = |ui: &mut egui::Ui, label: &str, v: &mut f32| {
        row(ui, label, |ui| {
            ui.add(
                egui::DragValue::new(v)
                    .range(0.0001..=1000.0)
                    .speed(0.01)
                    .suffix(" m"),
            )
        })
    };
    let count = |ui: &mut egui::Ui, label: &str, v: &mut u32, min: u32, max: u32| {
        row(ui, label, |ui| {
            ui.add(egui::DragValue::new(v).range(min..=max).speed(0.2))
        })
    };
    let r = match &mut pm.shape {
        Primitive::Plane { size } | Primitive::Cube { size } => len(ui, "Size", size),
        Primitive::Circle { vertices, radius } => {
            count(ui, "Vertices", vertices, 3, 512).union(len(ui, "Radius", radius))
        }
        Primitive::UvSphere {
            segments,
            rings,
            radius,
        } => count(ui, "Segments", segments, 3, 512)
            .union(count(ui, "Rings", rings, 3, 256))
            .union(len(ui, "Radius", radius)),
        Primitive::IcoSphere {
            subdivisions,
            radius,
        } => count(ui, "Subdivisions", subdivisions, 1, 6).union(len(ui, "Radius", radius)),
        Primitive::Cylinder {
            vertices,
            radius,
            depth,
        } => count(ui, "Vertices", vertices, 3, 512)
            .union(len(ui, "Radius", radius))
            .union(len(ui, "Depth", depth)),
        Primitive::Cone {
            vertices,
            radius1,
            radius2,
            depth,
        } => count(ui, "Vertices", vertices, 3, 512)
            .union(len(ui, "Radius 1", radius1))
            .union(len(ui, "Radius 2", radius2))
            .union(len(ui, "Depth", depth)),
        Primitive::Torus {
            major_segments,
            minor_segments,
            major_radius,
            minor_radius,
        } => count(ui, "Major Segments", major_segments, 3, 512)
            .union(count(ui, "Minor Segments", minor_segments, 3, 256))
            .union(len(ui, "Major Radius", major_radius))
            .union(len(ui, "Minor Radius", minor_radius)),
    };
    pm.shape = pm.shape.sanitized();
    r
}

fn light_tab(ui: &mut egui::Ui, world: &mut World, ed: &mut Editor, e: Entity) {
    heading(ui, icon::LIGHTBULB, "Light");
    shading_hint(ui);
    let Some(light) = world.get::<Light>(e).copied() else {
        return;
    };
    section(ui, "light.main", "Light", true, |ui| {
        edit_component::<Light, _>(ui, world, ed, e, "Light Type", |ui, l| {
            ui.horizontal(|ui| {
                let mut response: Option<egui::Response> = None;
                for kind in LightKind::ALL {
                    let r = ui.selectable_value(&mut l.kind, kind, kind.label());
                    response = Some(match response {
                        Some(p) => p.union(r),
                        None => r,
                    });
                }
                response.expect("four kinds")
            })
            .inner
        });
        row(ui, "Temperature", |ui| {
            edit_component::<Light, _>(ui, world, ed, e, "Light Temperature", |ui, l| {
                let r = ui.checkbox(&mut l.use_temperature, "");
                let d = ui.add_enabled(
                    l.use_temperature,
                    egui::DragValue::new(&mut l.temperature)
                        .range(800.0..=20000.0)
                        .speed(10.0)
                        .suffix(" K"),
                );
                let [r_, g, b] = blackbody(l.temperature);
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
                ui.painter().rect_filled(
                    rect,
                    egui::CornerRadius::same(3),
                    egui::Color32::from(egui::Rgba::from_rgb(r_, g, b)),
                );
                r.union(d)
            })
        })
        .map(|r| {
            r.on_hover_text(
                "Color by black-body temperature in Kelvin: ~1900 K candle, 3200 K tungsten, \
                 5500 K daylight, 6500 K overcast, 10000 K blue sky. The Color below then tints \
                 it.",
            )
        });
        row(
            ui,
            if light.use_temperature {
                "Tint"
            } else {
                "Color"
            },
            |ui| {
                edit_component::<Light, _>(ui, world, ed, e, "Light Color", |ui, l| {
                    color_linear(ui, &mut l.color)
                })
            },
        );
        let power_label = if light.kind == LightKind::Sun {
            "Strength"
        } else {
            "Power"
        };
        row(ui, power_label, |ui| {
            edit_component::<Light, _>(ui, world, ed, e, "Light Power", |ui, l| {
                let (suffix, speed, max) = if l.kind == LightKind::Sun {
                    (" W/m²", 0.01, 1000.0)
                } else {
                    (" W", 5.0, 1.0e6)
                };
                ui.add(
                    egui::DragValue::new(&mut l.power)
                        .range(0.0..=max)
                        .speed(speed)
                        .suffix(suffix),
                )
            })
        })
        .map(|r| {
            r.on_hover_text(if light.kind == LightKind::Sun {
                "Sun strength in watts per square metre. 1 is a soft overcast sun; 3–5 a bright \
                 day (with AgX). It doesn't fade with distance."
            } else {
                "Emitted power in watts (radiometric, as in Blender: not a light-bulb's \
                 electrical watts). Brightness falls off with the square of distance."
            })
        });
        row(ui, "Exposure", |ui| {
            edit_component::<Light, _>(ui, world, ed, e, "Light Exposure", |ui, l| {
                ui.add(
                    egui::DragValue::new(&mut l.exposure)
                        .range(-32.0..=32.0)
                        .speed(0.02)
                        .suffix(" EV"),
                )
            })
        })
        .map(|r| r.on_hover_text("Extra stops on top of the power: +1 doubles it."));
        match light.kind {
            LightKind::Point | LightKind::Spot => {
                planned_row(
                    ui,
                    "Radius",
                    &format!("{:.2} m", light.radius),
                    "Size of the light bulb: bigger = softer shadows and broader highlights.",
                    Status::Planned("M4", "soft shadows"),
                );
                planned_row(
                    ui,
                    "Soft Falloff",
                    if light.soft_falloff { "On" } else { "Off" },
                    "Smooths the light's falloff near its surface to avoid hot spots.",
                    Status::Planned("M4", "soft shadows"),
                );
            }
            LightKind::Sun => planned_row(
                ui,
                "Angle",
                &format!("{:.2}°", light.angle.to_degrees()),
                "Angular diameter of the sun as seen from the ground (Earth's is 0.53°): bigger \
                 = softer shadows.",
                Status::Planned("M4", "soft shadows"),
            ),
            LightKind::Area => {
                ui.label(
                    egui::RichText::new(format!(
                        "{} Until M4 an area light is shaded as a 180° light with cosine \
                         falloff; its size and shape don't change the lighting yet.",
                        icon::INFO
                    ))
                    .small()
                    .color(theme::PLANNED),
                );
                planned_row(
                    ui,
                    "Shape / Size",
                    &format!("Rectangle {:.1}×{:.1} m", light.size.x, light.size.y),
                    "A rectangle, square, disk or ellipse emitting from one side. Larger areas \
                     give softer light.",
                    Status::Planned("M4", "area lights (LTC)"),
                );
                planned_row(
                    ui,
                    "Normalize",
                    if light.normalize { "On" } else { "Off" },
                    "Keep total power the same when resizing the area.",
                    Status::Planned("M4", "area lights (LTC)"),
                );
            }
        }
    });

    if light.kind == LightKind::Spot {
        section(ui, "light.beam", "Beam Shape", true, |ui| {
            row(ui, "Spot Size", |ui| {
                edit_component::<Light, _>(ui, world, ed, e, "Spot Size", |ui, l| {
                    let mut deg = l.spot_size.to_degrees();
                    let r = ui.add(
                        egui::DragValue::new(&mut deg)
                            .range(1.0..=180.0)
                            .speed(0.5)
                            .suffix("°"),
                    );
                    l.spot_size = deg.to_radians();
                    r
                })
            })
            .map(|r| r.on_hover_text("Full opening angle of the cone."));
            row(ui, "Blend", |ui| {
                edit_component::<Light, _>(ui, world, ed, e, "Spot Blend", |ui, l| {
                    ui.add(egui::Slider::new(&mut l.spot_blend, 0.0..=1.0))
                })
            })
            .map(|r| {
                r.on_hover_text("Softness of the cone's edge: 0 hard, 1 fades from the center.")
            });
            row(ui, "Show Cone", |ui| {
                edit_component::<Light, _>(ui, world, ed, e, "Show Cone", |ui, l| {
                    ui.checkbox(&mut l.show_cone, "")
                })
            });
        });
    }

    if light.kind != LightKind::Sun {
        section(ui, "light.distance", "Custom Distance", false, |ui| {
            row(ui, "Use", |ui| {
                edit_component::<Light, _>(ui, world, ed, e, "Custom Distance", |ui, l| {
                    ui.checkbox(&mut l.use_custom_distance, "")
                })
            });
            row(ui, "Distance", |ui| {
                edit_component::<Light, _>(ui, world, ed, e, "Custom Distance", |ui, l| {
                    ui.add_enabled(
                        l.use_custom_distance,
                        egui::DragValue::new(&mut l.custom_distance)
                            .range(0.01..=10000.0)
                            .speed(0.1)
                            .suffix(" m"),
                    )
                })
            })
            .map(|r| {
                r.on_hover_text(
                    "The light's influence ends smoothly at this distance. Real light never \
                     stops, but cutting it off keeps many small lights cheap.",
                )
            });
        });
    }

    section(ui, "light.influence", "Influence", false, |ui| {
        row(ui, "Diffuse", |ui| {
            edit_component::<Light, _>(ui, world, ed, e, "Light Diffuse", |ui, l| {
                ui.add(egui::Slider::new(&mut l.diffuse_factor, 0.0..=1.0))
            })
        })
        .map(|r| r.on_hover_text("How much this light shows in matte (diffuse) shading."));
        row(ui, "Glossy", |ui| {
            edit_component::<Light, _>(ui, world, ed, e, "Light Glossy", |ui, l| {
                ui.add(egui::Slider::new(&mut l.specular_factor, 0.0..=1.0))
            })
        })
        .map(|r| r.on_hover_text("How much this light shows in reflections and highlights."));
        planned_row(
            ui,
            "Transmission",
            "1.00",
            "How much this light passes through refractive materials.",
            Status::Planned("M12", "refraction"),
        );
        planned_row(
            ui,
            "Volume Scatter",
            "1.00",
            "How much this light lights up fog and smoke.",
            Status::Planned("T3", "volumetrics (after M12)"),
        );
    });

    schema_panels(ui, world, ed, settings::LIGHT_PANELS, None);
}

fn camera_tab(ui: &mut egui::Ui, world: &mut World, ed: &mut Editor, e: Entity) {
    heading(ui, icon::VIDEO_CAMERA, "Camera");
    section(ui, "camera.lens", "Lens", true, |ui| {
        row(ui, "Type", |ui| {
            ui.label("Perspective");
            crate::widgets::status_badge(ui, Status::Live);
        });
        row(ui, "Field of View", |ui| {
            edit_component::<Camera3d, _>(ui, world, ed, e, "Field of View", |ui, c| {
                let mut deg = c.fov_y.to_degrees();
                let r = ui.add(
                    egui::DragValue::new(&mut deg)
                        .range(1.0..=170.0)
                        .speed(0.2)
                        .suffix("°"),
                );
                c.fov_y = deg.to_radians();
                r
            })
        })
        .map(|r| {
            r.on_hover_text(
                "Vertical field of view. Wide angles (90°+) exaggerate depth; narrow ones \
                 (20°) flatten it like a telephoto lens.",
            )
        });
        let fov = world.get::<Camera3d>(e).map_or(1.0, |c| c.fov_y);
        // Blender's default 36 mm sensor, fitted vertically for a 16:9 frame.
        let sensor_h = 36.0 * 9.0 / 16.0;
        row(ui, "Focal Length", |ui| {
            ui.label(format!("≈ {:.0} mm", sensor_h / 2.0 / (fov / 2.0).tan()))
                .on_hover_text(
                    "The same lens as a focal length on Blender's 36 mm sensor (16:9). 50 mm is \
                     the classic 'normal' lens.",
                );
        });
        row(ui, "Clip Start", |ui| {
            edit_component::<Camera3d, _>(ui, world, ed, e, "Clip Start", |ui, c| {
                ui.add(
                    egui::DragValue::new(&mut c.near)
                        .range(0.001..=100.0)
                        .speed(0.005)
                        .suffix(" m"),
                )
            })
        })
        .map(|r| {
            r.on_hover_text(
                "Nothing closer than this is drawn. Keep it as large as you can: it sets how \
                 precise the depth buffer is.",
            )
        });
        planned_row(
            ui,
            "Clip End",
            "∞",
            "Our depth buffer is reversed-Z with an infinite far plane, so there is no far clip \
             distance to set: everything to the horizon is drawn with good precision.",
            Status::NotApplicable("reversed-Z makes the far plane infinite"),
        );
        planned_row(
            ui,
            "Orthographic",
            "—",
            "A camera without perspective (architectural and 2D views).",
            Status::Planned("M6", "the editor's view modes"),
        );
    });
    section(ui, "camera.active", "Game Camera", true, |ui| {
        row(ui, "Priority", |ui| {
            edit_component::<Camera3d, _>(ui, world, ed, e, "Camera Priority", |ui, c| {
                ui.add(egui::DragValue::new(&mut c.priority).range(-100..=100))
            })
        })
        .map(|r| r.on_hover_text("When playing, the camera with the highest priority is used."));
        if ui.button("Set Active Camera").clicked() {
            let max = {
                let mut q = world.query::<&Camera3d>();
                q.iter(world).map(|c| c.priority).max().unwrap_or(0)
            };
            if let Some(before) = world.get::<Camera3d>(e).copied() {
                let mut after = before;
                after.priority = max + 1;
                ed.history.execute(
                    world,
                    Box::new(SetComponent {
                        entity: e,
                        before,
                        after,
                        label: "Set Active Camera",
                    }),
                    None,
                );
            }
        }
    });
    planned_row(
        ui,
        "Depth of Field",
        "Off",
        "Focus distance and f-stop: blur things nearer or farther than the focus.",
        Status::Planned("M12", "post-processing stack"),
    );
}

fn material_tab(ui: &mut egui::Ui, world: &mut World, ed: &mut Editor) {
    let Some(e) = ed.selected else { return };
    heading(ui, icon::CIRCLE_HALF, "Material");
    shading_hint(ui);
    let asset_material = world
        .get::<MeshRenderer>(e)
        .and_then(|m| m.material)
        .and_then(|id| {
            world
                .get_resource::<ge_asset::AssetServer>()
                .and_then(|s| s.get(id))
                .cloned()
        });
    if let Some(asset) = &asset_material {
        section(ui, "material.asset", "From the model file", true, |ui| {
            row(ui, "Name", |ui| ui.label(asset.name.as_str()));
            let [r, g, b, _] = asset.base_color;
            row(ui, "Base Color", |ui| {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(40.0, 16.0), egui::Sense::hover());
                ui.painter().rect_filled(
                    rect,
                    egui::CornerRadius::same(3),
                    egui::Color32::from(egui::Rgba::from_rgb(r, g, b)),
                );
                ui.label(if asset.base_color_texture.is_some() {
                    "× texture"
                } else {
                    ""
                })
            });
            row(ui, "Metallic", |ui| {
                ui.label(format!("{:.2}", asset.metallic))
            });
            row(ui, "Roughness", |ui| {
                ui.label(format!("{:.2}", asset.roughness))
            });
            let textures = [
                ("Base color", asset.base_color_texture.is_some()),
                ("Metal/rough", asset.metallic_roughness_texture.is_some()),
                ("Emission", asset.emissive_texture.is_some()),
            ];
            row(ui, "Textures", |ui| {
                let used: Vec<&str> = textures
                    .iter()
                    .filter(|(_, on)| *on)
                    .map(|(n, _)| *n)
                    .collect();
                ui.label(if used.is_empty() {
                    "none".into()
                } else {
                    used.join(", ")
                })
            });
            planned_row(
                ui,
                "Normal / Occlusion",
                if asset.normal_texture.is_some() || asset.occlusion_texture.is_some() {
                    "in file"
                } else {
                    "—"
                },
                "Normal maps add small surface detail to lighting; occlusion darkens crevices. \
                 They are imported and stored now, and rendered with full PBR.",
                Status::Planned("M4", "full PBR"),
            );
        });
    }
    if world.get::<Material>(e).is_none() {
        let override_text = if asset_material.is_some() {
            "Edits to the file's material come from re-exporting it. Override to tweak it here \
             (the file's textures stay applied)."
        } else {
            "This object has no material: it uses its game color."
        };
        ui.label(egui::RichText::new(override_text).color(theme::TEXT_MUTED));
        if let Some(asset) = &asset_material
            && ui
                .button(format!("{} Override Material", icon::PENCIL_SIMPLE))
                .on_hover_text("Copy the file's values into an editable material on this object")
                .clicked()
        {
            let [r, g, b, _] = asset.base_color;
            ed.history.execute(
                world,
                Box::new(AddComponent {
                    entity: e,
                    value: Material {
                        base_color: Vec3::new(r, g, b),
                        metallic: asset.metallic,
                        roughness: asset.roughness,
                        emission_color: Vec3::from(asset.emissive),
                        emission_strength: if asset.emissive == [0.0; 3] { 0.0 } else { 1.0 },
                        backface_culling: !asset.double_sided,
                    },
                    label: "Override Material",
                }),
                None,
            );
            return;
        }
        if asset_material.is_none()
            && ui
                .button(format!("{} New Material", icon::PLUS))
                .on_hover_text("Adds a Principled BSDF starting from the object's current color")
                .clicked()
        {
            let tint = world
                .get::<MeshRenderer>(e)
                .map_or(Vec3::splat(0.8), |m| m.tint.truncate());
            if let Some(before) = world.get::<MeshRenderer>(e).copied() {
                let mut after = before;
                after.tint = Vec4::ONE;
                ed.history.execute(
                    world,
                    Box::new(SetComponent {
                        entity: e,
                        before,
                        after,
                        label: "New Material",
                    }),
                    None,
                );
            }
            ed.history.execute(
                world,
                Box::new(AddComponent {
                    entity: e,
                    value: Material {
                        base_color: tint,
                        ..Default::default()
                    },
                    label: "New Material",
                }),
                None,
            );
        }
        return;
    }
    section(
        ui,
        "material.surface",
        "Surface · Principled BSDF",
        true,
        |ui| {
            row(ui, "Base Color", |ui| {
                edit_component::<Material, _>(ui, world, ed, e, "Base Color", |ui, m| {
                    color_linear(ui, &mut m.base_color)
                })
            })
            .map(|r| {
                r.on_hover_text(
                    "The surface color: diffuse color for non-metals, reflection color for metals.",
                )
            });
            row(ui, "Metallic", |ui| {
                edit_component::<Material, _>(ui, world, ed, e, "Metallic", |ui, m| {
                    ui.add(egui::Slider::new(&mut m.metallic, 0.0..=1.0))
                })
            })
            .map(|r| {
                r.on_hover_text(
                    "0 = dielectric (plastic, wood, stone: colored diffuse + white reflections). \
                 1 = metal (no diffuse, reflections tinted by the base color). Real materials \
                 are almost always 0 or 1.",
                )
            });
            row(ui, "Roughness", |ui| {
                edit_component::<Material, _>(ui, world, ed, e, "Roughness", |ui, m| {
                    ui.add(egui::Slider::new(&mut m.roughness, 0.0..=1.0))
                })
            })
            .map(|r| {
                r.on_hover_text(
                    "Microscopic surface bumpiness. 0 = mirror-sharp highlights, 1 = broad, dull \
                 highlights. Move a light around a sphere and watch the highlight spread.",
                )
            });
            row(ui, "Emission", |ui| {
                edit_component::<Material, _>(ui, world, ed, e, "Emission Color", |ui, m| {
                    color_linear(ui, &mut m.emission_color)
                })
            });
            row(ui, "Emission Strength", |ui| {
                edit_component::<Material, _>(ui, world, ed, e, "Emission Strength", |ui, m| {
                    ui.add(
                        egui::DragValue::new(&mut m.emission_strength)
                            .range(0.0..=1000.0)
                            .speed(0.02),
                    )
                })
            })
            .map(|r| {
                r.on_hover_text(
                    "Light the surface gives off by itself. In EEVEE emission doesn't light other \
                 objects unless GI picks it up; here it only brightens the surface itself.",
                )
            });
        },
    );
    section(ui, "material.settings", "Settings", false, |ui| {
        row(ui, "Backface Culling", |ui| {
            edit_component::<Material, _>(ui, world, ed, e, "Backface Culling", |ui, m| {
                ui.checkbox(&mut m.backface_culling, "Camera")
            })
        })
        .map(|r| {
            r.on_hover_text(
                "Skip drawing faces that point away from the camera. Off (Blender's default) \
                 shows both sides of planes and open meshes.",
            )
        });
    });
    schema_panels(ui, world, ed, settings::MATERIAL_PANELS, None);
}

/// The model instance root (or the instance this entity belongs to), if any.
fn scene_root_of(world: &World, mut e: Entity) -> Option<Entity> {
    loop {
        if world.get::<ge_render::SceneRoot>(e).is_some() {
            return Some(e);
        }
        e = world.get::<ChildOf>(e)?.parent();
    }
}

/// Source file, GUID and load state of the model `e` belongs to.
fn model_source_rows(ui: &mut egui::Ui, world: &World, e: Entity) {
    let Some(root) = scene_root_of(world, e) else {
        return;
    };
    let (Some(scene), Some(server)) = (
        world.get::<ge_render::SceneRoot>(root),
        world.get_resource::<ge_asset::AssetServer>(),
    ) else {
        return;
    };
    row(ui, "Source", |ui| {
        ui.label(scene.0.source().to_string())
            .on_hover_text("The file under assets/. Re-export it and the model updates live.")
    });
    if let Some(guid) = server.guid_of(&scene.0) {
        row(ui, "GUID", |ui| {
            ui.label(
                egui::RichText::new(guid.to_string())
                    .small()
                    .color(theme::TEXT_MUTED),
            )
            .on_hover_text(
                "Stored in the .meta file next to the source. Moving or renaming the file \
                     together with its .meta keeps this id, so references survive.",
            )
        });
    }
    let state = server.state(scene.0.id());
    row(ui, "State", |ui| match state {
        ge_asset::LoadState::Loaded => {
            ui.label(format!("Loaded (version {})", server.version(scene.0.id())))
        }
        ge_asset::LoadState::Loading => ui.label("Loading…"),
        ge_asset::LoadState::Failed(err) => {
            ui.label(egui::RichText::new(format!("Failed: {err}")).color(theme::AXIS_X))
        }
        ge_asset::LoadState::NotLoaded => ui.label("Not loaded"),
    });
}

fn asset_mesh_info(
    ui: &mut egui::Ui,
    world: &World,
    e: Entity,
    id: ge_asset::AssetId<ge_asset::Mesh>,
) {
    section(ui, "mesh.asset", "Imported Mesh", true, |ui| {
        model_source_rows(ui, world, e);
        if let Some(mesh) = world
            .get_resource::<ge_asset::AssetServer>()
            .and_then(|s| s.get(id))
        {
            row(ui, "Vertices", |ui| {
                ui.label(mesh.vertex_count().to_string())
            });
            row(ui, "Triangles", |ui| {
                ui.label(mesh.triangle_count().to_string())
            });
            let size = glam::Vec3::from(mesh.bounds_max) - glam::Vec3::from(mesh.bounds_min);
            row(ui, "Size", |ui| {
                ui.label(format!("{:.2} × {:.2} × {:.2} m", size.x, size.y, size.z))
            });
        }
        planned_row(
            ui,
            "Import Settings",
            "Edit .meta",
            "Scale, UV flip and mip generation live in the .meta file next to the source; \
             editing them here comes with the asset browser.",
            Status::Planned("M6", "the asset browser"),
        );
    });
}
