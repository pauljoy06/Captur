use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle, Theme, Visuals};

pub const BG: Color32 = Color32::from_rgb(16, 19, 27);
pub const SURFACE: Color32 = Color32::from_rgb(25, 30, 41);
pub const SURFACE_HOVER: Color32 = Color32::from_rgb(34, 41, 55);
pub const BORDER: Color32 = Color32::from_rgb(55, 65, 82);
pub const TEXT: Color32 = Color32::from_rgb(235, 239, 247);
pub const MUTED: Color32 = Color32::from_rgb(149, 160, 180);
pub const ACCENT: Color32 = Color32::from_rgb(93, 139, 255);
pub const SUCCESS: Color32 = Color32::from_rgb(77, 201, 139);
pub const DANGER: Color32 = Color32::from_rgb(239, 99, 108);
pub const RADIUS: u8 = 9;
pub const SPACE: f32 = 12.0;

pub fn apply(context: &egui::Context) {
    let mut style = (*context.style_of(Theme::Dark)).clone();
    style.visuals = Visuals::dark();
    style.visuals.panel_fill = BG;
    style.visuals.window_fill = SURFACE;
    style.visuals.extreme_bg_color = Color32::from_rgb(11, 14, 20);
    style.visuals.widgets.inactive.bg_fill = SURFACE;
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    style.visuals.widgets.hovered.bg_fill = SURFACE_HOVER;
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
    style.visuals.widgets.active.bg_fill = ACCENT;
    style.visuals.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    style.visuals.window_corner_radius = CornerRadius::same(RADIUS);
    style.visuals.menu_corner_radius = CornerRadius::same(RADIUS);
    style.spacing.item_spacing = egui::vec2(SPACE, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 7.0);
    style.text_styles.insert(
        TextStyle::Heading,
        FontId::new(24.0, FontFamily::Proportional),
    );
    style
        .text_styles
        .insert(TextStyle::Body, FontId::new(14.0, FontFamily::Proportional));
    style.text_styles.insert(
        TextStyle::Button,
        FontId::new(13.0, FontFamily::Proportional),
    );
    context.set_style_of(Theme::Dark, style);
}

pub fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(12.0)
}
