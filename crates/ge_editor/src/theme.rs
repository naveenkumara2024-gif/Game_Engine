//! Look and feel: the dark floating-canvas theme from the reference layout (plan 0004,
//! "Layout"). Every color the editor uses is a token here.

use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle};

pub const PANEL: Color32 = Color32::from_rgb(0x23, 0x23, 0x29);
pub const PANEL_RAISED: Color32 = Color32::from_rgb(0x2c, 0x2c, 0x34);
pub const PANEL_BORDER: Color32 = Color32::from_rgb(0x2e, 0x2d, 0x39);
pub const TEXT: Color32 = Color32::from_rgb(0xe3, 0xe3, 0xe8);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(0x8f, 0x8f, 0x9d);
pub const ACCENT: Color32 = Color32::from_rgb(0xa8, 0xa5, 0xff);
pub const ACCENT_BG: Color32 = Color32::from_rgb(0x40, 0x3e, 0x6a);
pub const ACCENT_TEXT: Color32 = Color32::from_rgb(0x1b, 0x1b, 0x1f);
pub const SELECTION: Color32 = Color32::from_rgb(0xff, 0xa5, 0x3a);
pub const LIVE: Color32 = Color32::from_rgb(0x6f, 0xcf, 0x8f);
pub const PLANNED: Color32 = Color32::from_rgb(0xe0, 0xb0, 0x5a);
pub const AXIS_X: Color32 = Color32::from_rgb(0xdc, 0x38, 0x45);
pub const AXIS_Y: Color32 = Color32::from_rgb(0x7c, 0xc4, 0x3c);
pub const AXIS_Z: Color32 = Color32::from_rgb(0x30, 0x73, 0xdc);
pub const LIGHT_ICON: Color32 = Color32::from_rgb(0xff, 0xe0, 0x8a);
pub const CAMERA_ICON: Color32 = Color32::from_rgb(0xb8, 0xc8, 0xe8);

pub const RADIUS: u8 = 8;
pub const GUTTER: f32 = 16.0;

/// A floating panel's frame (rounded, bordered, soft shadow).
pub fn panel_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(PANEL)
        .stroke(Stroke::new(1.0, PANEL_BORDER))
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(Margin::same(8))
        .shadow(Shadow {
            offset: [0, 4],
            blur: 18,
            spread: 0,
            color: Color32::from_black_alpha(110),
        })
}

pub fn install(ctx: &egui::Context, ui_scale: f32) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    ctx.set_fonts(fonts);
    ctx.set_theme(egui::Theme::Dark);
    apply_style(ctx, ui_scale);
}

pub fn apply_style(ctx: &egui::Context, ui_scale: f32) {
    ctx.set_zoom_factor(ui_scale.clamp(0.5, 3.0));
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        style.text_styles = [
            (
                TextStyle::Small,
                FontId::new(11.0, FontFamily::Proportional),
            ),
            (TextStyle::Body, FontId::new(13.0, FontFamily::Proportional)),
            (
                TextStyle::Button,
                FontId::new(13.0, FontFamily::Proportional),
            ),
            (
                TextStyle::Heading,
                FontId::new(15.0, FontFamily::Proportional),
            ),
            (
                TextStyle::Monospace,
                FontId::new(12.0, FontFamily::Monospace),
            ),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(6.0, 5.0);
        style.spacing.button_padding = egui::vec2(7.0, 4.0);
        style.spacing.interact_size.y = 22.0;
        style.spacing.slider_width = 120.0;
        style.spacing.combo_width = 120.0;

        let v = &mut style.visuals;
        v.dark_mode = true;
        v.override_text_color = Some(TEXT);
        v.panel_fill = PANEL;
        v.window_fill = PANEL;
        v.window_stroke = Stroke::new(1.0, PANEL_BORDER);
        v.window_corner_radius = CornerRadius::same(RADIUS);
        v.menu_corner_radius = CornerRadius::same(RADIUS);
        v.extreme_bg_color = Color32::from_rgb(0x1a, 0x1a, 0x1f);
        v.faint_bg_color = PANEL_RAISED;
        v.selection.bg_fill = ACCENT_BG;
        v.selection.stroke = Stroke::new(1.0, ACCENT);
        v.hyperlink_color = ACCENT;
        v.popup_shadow = Shadow {
            offset: [0, 4],
            blur: 16,
            spread: 0,
            color: Color32::from_black_alpha(120),
        };
        let radius = CornerRadius::same(6);
        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.corner_radius = radius;
        }
        v.widgets.noninteractive.bg_fill = PANEL;
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, PANEL_BORDER);
        v.widgets.inactive.bg_fill = PANEL_RAISED;
        v.widgets.inactive.weak_bg_fill = PANEL_RAISED;
        // A faint border so checkboxes and fields stay visible, even disabled ones.
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x3a, 0x39, 0x48));
        v.widgets.hovered.bg_fill = Color32::from_rgb(0x36, 0x36, 0x42);
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x36, 0x36, 0x42);
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x4a, 0x48, 0x6e));
        v.widgets.active.bg_fill = ACCENT_BG;
        v.widgets.active.weak_bg_fill = ACCENT_BG;
        v.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
        v.widgets.open.bg_fill = ACCENT_BG;
        v.widgets.open.weak_bg_fill = ACCENT_BG;
    });
}
