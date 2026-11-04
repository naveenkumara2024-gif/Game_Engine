//! egui ⇄ engine glue (plan 0004, decision 6). Platform events become `egui::RawInput`; egui's
//! tessellated output becomes a plain `ge_render::ui::UiDrawList`. No `egui-winit` or
//! `egui-wgpu`: the platform layer and the RHI stay the only crates touching winit and wgpu.

use ge_engine::{Modifiers, MouseButton, PlatformEvent};
use ge_render::ui::{UiDrawList, UiMesh, UiTextureId, UiTextureUpdate, UiVertex};

/// Accumulates input between frames and remembers what egui needs across them (cursor,
/// modifiers, which buttons are held).
#[derive(Debug, Default)]
pub struct InputTranslator {
    cursor: Option<egui::Pos2>,
    modifiers: egui::Modifiers,
}

impl InputTranslator {
    /// Builds this frame's input. `scale` is physical pixels per point; positions from the
    /// platform are physical.
    pub fn raw_input(
        &mut self,
        events: &[PlatformEvent],
        window_px: (u32, u32),
        scale: f32,
        time: f64,
        dt: f32,
    ) -> egui::RawInput {
        let scale = scale.max(0.1);
        let mut out = Vec::new();
        for event in events {
            match event {
                PlatformEvent::CursorMoved { x, y } => {
                    let pos = egui::pos2(x / scale, y / scale);
                    self.cursor = Some(pos);
                    out.push(egui::Event::PointerMoved(pos));
                }
                PlatformEvent::CursorLeft => {
                    self.cursor = None;
                    out.push(egui::Event::PointerGone);
                }
                PlatformEvent::MouseButton { button, pressed } => {
                    let button = match button {
                        MouseButton::Left => egui::PointerButton::Primary,
                        MouseButton::Right => egui::PointerButton::Secondary,
                        MouseButton::Middle => egui::PointerButton::Middle,
                        MouseButton::Other(3) => egui::PointerButton::Extra1,
                        MouseButton::Other(_) => egui::PointerButton::Extra2,
                    };
                    if let Some(pos) = self.cursor {
                        out.push(egui::Event::PointerButton {
                            pos,
                            button,
                            pressed: *pressed,
                            modifiers: self.modifiers,
                        });
                    }
                }
                PlatformEvent::MouseWheel { lines, horizontal } => {
                    out.push(egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Line,
                        delta: egui::vec2(*horizontal, *lines),
                        phase: egui::TouchPhase::Move,
                        modifiers: self.modifiers,
                    });
                }
                PlatformEvent::Modifiers(m) => {
                    self.modifiers = modifiers(*m);
                    out.push(egui::Event::ModifiersChanged(self.modifiers));
                }
                PlatformEvent::Text(text) => out.push(egui::Event::Text(text.clone())),
                PlatformEvent::Key {
                    key,
                    pressed,
                    repeat,
                } => {
                    if let Some(k) = key_from_name(key) {
                        // Clipboard shortcuts egui expects as dedicated events.
                        if *pressed && self.modifiers.command {
                            match k {
                                egui::Key::C => out.push(egui::Event::Copy),
                                egui::Key::X => out.push(egui::Event::Cut),
                                _ => {}
                            }
                        }
                        out.push(egui::Event::Key {
                            key: k,
                            physical_key: Some(k),
                            pressed: *pressed,
                            repeat: *repeat,
                            modifiers: self.modifiers,
                        });
                    }
                }
                PlatformEvent::Focused(focused) => {
                    out.push(egui::Event::WindowFocused(*focused));
                    if !focused {
                        self.modifiers = egui::Modifiers::default();
                    }
                }
                _ => {}
            }
        }
        let size = egui::vec2(window_px.0 as f32 / scale, window_px.1 as f32 / scale);
        let mut raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            time: Some(time),
            predicted_dt: dt,
            events: out,
            focused: true,
            max_texture_side: Some(8192),
            ..Default::default()
        };
        if let Some(info) = raw.viewports.get_mut(&raw.viewport_id) {
            info.native_pixels_per_point = Some(scale);
        } else {
            raw.viewports.insert(
                raw.viewport_id,
                egui::ViewportInfo {
                    native_pixels_per_point: Some(scale),
                    ..Default::default()
                },
            );
        }
        raw
    }
}

fn modifiers(m: Modifiers) -> egui::Modifiers {
    egui::Modifiers {
        alt: m.alt,
        ctrl: m.ctrl,
        shift: m.shift,
        mac_cmd: false,
        command: m.ctrl,
    }
}

/// Our layout-independent key names (winit `KeyCode` debug names) → egui keys.
pub fn key_from_name(name: &str) -> Option<egui::Key> {
    use egui::Key;
    if let Some(letter) = name.strip_prefix("Key")
        && letter.len() == 1
    {
        return Key::from_name(letter);
    }
    if let Some(digit) = name.strip_prefix("Digit") {
        return Key::from_name(digit);
    }
    if let Some(n) = name.strip_prefix("Numpad")
        && n.len() == 1
    {
        return Key::from_name(n);
    }
    Some(match name {
        "Escape" => Key::Escape,
        "Enter" | "NumpadEnter" => Key::Enter,
        "Tab" => Key::Tab,
        "Backspace" => Key::Backspace,
        "Delete" => Key::Delete,
        "Insert" => Key::Insert,
        "Home" => Key::Home,
        "End" => Key::End,
        "PageUp" => Key::PageUp,
        "PageDown" => Key::PageDown,
        "Space" => Key::Space,
        "ArrowUp" => Key::ArrowUp,
        "ArrowDown" => Key::ArrowDown,
        "ArrowLeft" => Key::ArrowLeft,
        "ArrowRight" => Key::ArrowRight,
        "Minus" | "NumpadSubtract" => Key::Minus,
        "Equal" | "NumpadAdd" => Key::Plus,
        "Period" | "NumpadDecimal" => Key::Period,
        "Comma" => Key::Comma,
        "Slash" => Key::Slash,
        "Backquote" => Key::Backtick,
        "F1" => Key::F1,
        "F2" => Key::F2,
        "F3" => Key::F3,
        "F4" => Key::F4,
        "F5" => Key::F5,
        "F6" => Key::F6,
        "F7" => Key::F7,
        "F8" => Key::F8,
        "F9" => Key::F9,
        "F10" => Key::F10,
        "F11" => Key::F11,
        "F12" => Key::F12,
        _ => return None,
    })
}

pub fn texture_id(id: egui::TextureId) -> UiTextureId {
    // Managed and user ids share the u64 space; the top bit tells them apart.
    match id {
        egui::TextureId::Managed(n) => UiTextureId(n & !(1 << 63)),
        egui::TextureId::User(n) => UiTextureId(n | (1 << 63)),
    }
}

/// Converts egui's output into our draw list. Texture updates are **appended** (the renderer
/// drains them when it actually draws).
pub fn write_draw_list(
    ctx: &egui::Context,
    mut output: egui::FullOutput,
    scale: f32,
    list: &mut UiDrawList,
) {
    for (id, deltas) in &output.textures_delta.set {
        for delta in deltas {
            let egui::ImageData::Color(image) = &delta.image;
            let pixels: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
            list.textures.push(UiTextureUpdate::Set {
                id: texture_id(*id),
                origin: delta.pos.map(|[x, y]| [x as u32, y as u32]),
                size: [image.size[0] as u32, image.size[1] as u32],
                pixels,
                linear_filter: delta.options.magnification == egui::TextureFilter::Linear,
            });
        }
    }

    let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
    list.meshes.clear();
    for clipped in primitives {
        let egui::epaint::Primitive::Mesh(mesh) = clipped.primitive else {
            continue; // paint callbacks are not supported
        };
        if mesh.indices.is_empty() {
            continue;
        }
        let r = clipped.clip_rect;
        let x0 = (r.min.x * scale).round().max(0.0);
        let y0 = (r.min.y * scale).round().max(0.0);
        let x1 = (r.max.x * scale).round().max(x0);
        let y1 = (r.max.y * scale).round().max(y0);
        list.meshes.push(UiMesh {
            clip: [x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32],
            texture: Some(texture_id(mesh.texture_id)),
            vertices: mesh
                .vertices
                .iter()
                .map(|v| UiVertex {
                    pos: [v.pos.x * scale, v.pos.y * scale],
                    uv: [v.uv.x, v.uv.y],
                    color: v.color.to_array(),
                })
                .collect(),
            indices: mesh.indices,
        });
    }

    for id in &output.textures_delta.free {
        list.textures.push(UiTextureUpdate::Free(texture_id(*id)));
    }
    // Everything was copied into `list`; tell egui the deltas are handled (it asserts on drop).
    output.textures_delta.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_names_map() {
        assert_eq!(key_from_name("KeyA"), Some(egui::Key::A));
        assert_eq!(key_from_name("Digit3"), Some(egui::Key::Num3));
        assert_eq!(key_from_name("Numpad7"), Some(egui::Key::Num7));
        assert_eq!(key_from_name("F3"), Some(egui::Key::F3));
        assert_eq!(key_from_name("ShiftLeft"), None);
    }

    #[test]
    fn cursor_is_converted_to_points() {
        let mut t = InputTranslator::default();
        let raw = t.raw_input(
            &[PlatformEvent::CursorMoved { x: 300.0, y: 150.0 }],
            (1920, 1080),
            1.5,
            0.0,
            1.0 / 60.0,
        );
        assert_eq!(
            raw.events,
            [egui::Event::PointerMoved(egui::pos2(200.0, 100.0))]
        );
        assert_eq!(raw.screen_rect.unwrap().size(), egui::vec2(1280.0, 720.0));
    }

    #[test]
    fn a_frame_of_ui_becomes_draw_data_with_a_font_texture() {
        let ctx = egui::Context::default();
        let mut t = InputTranslator::default();
        let raw = t.raw_input(&[], (800, 600), 1.0, 0.0, 1.0 / 60.0);
        let out = ctx.run_ui(raw, |ui| {
            ui.label("hello");
        });
        let mut list = UiDrawList::default();
        write_draw_list(&ctx, out, 1.0, &mut list);
        assert!(!list.meshes.is_empty());
        assert!(
            list.textures
                .iter()
                .any(|t| matches!(t, UiTextureUpdate::Set { origin: None, .. })),
            "first frame uploads the font atlas"
        );
    }
}
