use egui::{
    Color32, CornerRadius, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle, Theme, Visuals,
};

// Color tokens
pub const BG: Color32 = Color32::from_rgb(13, 16, 23);
pub const SURFACE: Color32 = Color32::from_rgb(22, 27, 37);
pub const SURFACE_ELEVATED: Color32 = Color32::from_rgb(29, 35, 47);
pub const SURFACE_HOVER: Color32 = Color32::from_rgb(37, 45, 59);
pub const BORDER: Color32 = Color32::from_rgb(55, 66, 84);
pub const BORDER_FOCUS: Color32 = Color32::from_rgb(106, 148, 255);
pub const TEXT: Color32 = Color32::from_rgb(238, 242, 249);
pub const MUTED: Color32 = Color32::from_rgb(150, 161, 180);
pub const ACCENT: Color32 = Color32::from_rgb(92, 137, 255);
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(113, 155, 255);
pub const SUCCESS: Color32 = Color32::from_rgb(73, 199, 137);
pub const WARNING: Color32 = Color32::from_rgb(239, 181, 77);
pub const DANGER: Color32 = Color32::from_rgb(239, 96, 108);

// Spacing tokens
pub const SPACE_1: f32 = 4.0;
pub const SPACE_2: f32 = 8.0;
pub const SPACE_3: f32 = 12.0;
pub const SPACE_4: f32 = 16.0;
pub const SPACE_5: f32 = 24.0;
pub const SPACE_6: f32 = 32.0;

// Shape and sizing tokens
pub const RADIUS_SM: u8 = 5;
pub const RADIUS_MD: u8 = 9;
pub const RADIUS_LG: u8 = 13;
pub const CONTROL_HEIGHT: f32 = 34.0;

/// Installs Captur's coherent dark appearance for both explicitly dark and active UI.
pub fn apply(context: &egui::Context) {
    let mut style = (*context.style_of(Theme::Dark)).clone();
    let mut visuals = Visuals::dark();

    // Panels, windows, text edits, menus, and popups.
    visuals.override_text_color = Some(TEXT);
    visuals.panel_fill = BG;
    visuals.window_fill = SURFACE_ELEVATED;
    visuals.window_stroke = Stroke::new(1.0, BORDER);
    visuals.window_corner_radius = CornerRadius::same(RADIUS_LG);
    visuals.window_shadow = Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(110),
    };
    visuals.extreme_bg_color = Color32::from_rgb(9, 12, 18);
    visuals.faint_bg_color = SURFACE;
    visuals.code_bg_color = SURFACE_ELEVATED;
    visuals.menu_corner_radius = CornerRadius::same(RADIUS_MD);
    visuals.popup_shadow = Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(120),
    };

    // Selection and keyboard focus.
    visuals.selection.bg_fill = ACCENT.gamma_multiply(0.55);
    visuals.selection.stroke = Stroke::new(1.0, ACCENT_HOVER);
    visuals.hyperlink_color = ACCENT_HOVER;
    visuals.disabled_alpha = 0.48;

    // Non-interactive content and disabled controls.
    visuals.widgets.noninteractive.bg_fill = SURFACE;
    visuals.widgets.noninteractive.weak_bg_fill = SURFACE;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, MUTED);
    visuals.widgets.noninteractive.corner_radius = CornerRadius::same(RADIUS_MD);

    // Resting controls, including text edits.
    visuals.widgets.inactive.bg_fill = SURFACE_ELEVATED;
    visuals.widgets.inactive.weak_bg_fill = SURFACE;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(RADIUS_MD);

    // Pointer hover.
    visuals.widgets.hovered.bg_fill = SURFACE_HOVER;
    visuals.widgets.hovered.weak_bg_fill = SURFACE_HOVER;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT_HOVER);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(RADIUS_MD);
    visuals.widgets.hovered.expansion = 0.0;

    // Pressed and focused controls.
    visuals.widgets.active.bg_fill = ACCENT;
    visuals.widgets.active.weak_bg_fill = ACCENT;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, BORDER_FOCUS);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    visuals.widgets.active.corner_radius = CornerRadius::same(RADIUS_MD);
    visuals.widgets.active.expansion = 0.0;
    visuals.widgets.open.bg_fill = SURFACE_HOVER;
    visuals.widgets.open.weak_bg_fill = SURFACE_HOVER;
    visuals.widgets.open.bg_stroke = Stroke::new(1.0, BORDER_FOCUS);
    visuals.widgets.open.fg_stroke = Stroke::new(1.0, TEXT);
    visuals.widgets.open.corner_radius = CornerRadius::same(RADIUS_MD);

    style.visuals = visuals;
    style.spacing.item_spacing = egui::vec2(SPACE_3, SPACE_2);
    style.spacing.button_padding = egui::vec2(SPACE_3, SPACE_2);
    style.spacing.menu_margin = Margin::same(SPACE_2 as i8);
    style.spacing.interact_size.y = CONTROL_HEIGHT;
    style.spacing.combo_height = 240.0;
    style.spacing.scroll.bar_width = 10.0;
    style.spacing.scroll.floating = false;

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
    style.text_styles.insert(
        TextStyle::Small,
        FontId::new(12.0, FontFamily::Proportional),
    );

    context.set_style_of(Theme::Dark, style);
    context.set_theme(Theme::Dark);
}

pub fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(RADIUS_LG))
        .inner_margin(SPACE_4)
}

pub fn section() -> egui::Frame {
    egui::Frame::new()
        .fill(SURFACE_ELEVATED)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(RADIUS_MD))
        .inner_margin(SPACE_3)
}
