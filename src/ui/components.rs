use egui::{Button, Color32, CornerRadius, Frame, Label, Response, RichText, Stroke, Ui, Vec2};

use super::theme;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatusTone {
    Neutral,
    Accent,
    Success,
    Warning,
    Danger,
}

impl StatusTone {
    fn color(self) -> Color32 {
        match self {
            Self::Neutral => theme::MUTED,
            Self::Accent => theme::ACCENT,
            Self::Success => theme::SUCCESS,
            Self::Warning => theme::WARNING,
            Self::Danger => theme::DANGER,
        }
    }
}

pub fn card_frame() -> Frame {
    theme::card()
}

pub fn section_frame() -> Frame {
    theme::section()
}

pub fn primary_button(ui: &mut Ui, text: impl Into<egui::WidgetText>) -> Response {
    ui.add(
        Button::new(text)
            .fill(theme::ACCENT)
            .stroke(Stroke::new(1.0, theme::ACCENT_HOVER))
            .corner_radius(CornerRadius::same(theme::RADIUS_MD))
            .min_size(Vec2::new(0.0, theme::CONTROL_HEIGHT)),
    )
}

pub fn secondary_button(ui: &mut Ui, text: impl Into<egui::WidgetText>) -> Response {
    ui.add(
        Button::new(text)
            .fill(theme::SURFACE_ELEVATED)
            .stroke(Stroke::new(1.0, theme::BORDER))
            .corner_radius(CornerRadius::same(theme::RADIUS_MD))
            .min_size(Vec2::new(0.0, theme::CONTROL_HEIGHT)),
    )
}

pub fn danger_button(ui: &mut Ui, text: impl Into<egui::WidgetText>) -> Response {
    ui.add(
        Button::new(text)
            .fill(theme::DANGER.gamma_multiply(0.12))
            .stroke(Stroke::new(1.0, theme::DANGER.gamma_multiply(0.72)))
            .corner_radius(CornerRadius::same(theme::RADIUS_MD))
            .min_size(Vec2::new(0.0, theme::CONTROL_HEIGHT)),
    )
}

pub fn shortcut_chip(ui: &mut Ui, shortcut: impl Into<String>) -> Response {
    Frame::new()
        .fill(theme::BG)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(CornerRadius::same(theme::RADIUS_SM))
        .inner_margin(egui::Margin::symmetric(
            theme::SPACE_2 as i8,
            theme::SPACE_1 as i8,
        ))
        .show(ui, |ui| {
            ui.add(Label::new(
                RichText::new(shortcut.into())
                    .monospace()
                    .small()
                    .color(theme::MUTED),
            ))
        })
        .response
}

pub fn count_badge(ui: &mut Ui, count: usize) -> Response {
    badge(ui, count.to_string(), StatusTone::Accent)
}

pub fn status_badge(ui: &mut Ui, text: impl Into<String>, tone: StatusTone) -> Response {
    badge(ui, text, tone)
}

fn badge(ui: &mut Ui, text: impl Into<String>, tone: StatusTone) -> Response {
    let color = tone.color();
    Frame::new()
        .fill(color.gamma_multiply(0.16))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.72)))
        .corner_radius(CornerRadius::same(theme::RADIUS_SM))
        .inner_margin(egui::Margin::symmetric(
            theme::SPACE_2 as i8,
            theme::SPACE_1 as i8,
        ))
        .show(ui, |ui| {
            ui.add(Label::new(
                RichText::new(text.into()).small().strong().color(color),
            ))
        })
        .response
}

pub fn section_header(ui: &mut Ui, title: impl Into<String>, subtitle: Option<&str>) -> Response {
    ui.vertical(|ui| {
        ui.label(
            RichText::new(title.into())
                .size(17.0)
                .strong()
                .color(theme::TEXT),
        );
        if let Some(subtitle) = subtitle {
            ui.add_space(theme::SPACE_1);
            ui.label(RichText::new(subtitle).small().color(theme::MUTED));
        }
    })
    .response
}

pub fn status_banner(ui: &mut Ui, tone: StatusTone, message: impl Into<String>) -> Response {
    let color = tone.color();
    Frame::new()
        .fill(color.gamma_multiply(0.12))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.65)))
        .corner_radius(CornerRadius::same(theme::RADIUS_MD))
        .inner_margin(theme::SPACE_3)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("●").color(color));
                ui.label(RichText::new(message.into()).color(theme::TEXT));
            });
        })
        .response
}
