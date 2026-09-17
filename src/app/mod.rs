use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

use egui::{Color32, CornerRadius, RichText, Stroke, StrokeKind, TextureHandle};

use crate::{
    annotations::{AnnotationDocument, AnnotationItem, AnnotationPoint, AnnotationTool},
    capture::{
        BgraFrame, dxgi,
        region::{PixelPoint, PixelRect},
    },
    encoding::wic,
    evidence::{EvidenceLabel, EvidenceSession, default_pdf_filename},
    export::{Exporter, pdf::PdfExporter},
    platform::{
        clipboard, dialog,
        hotkeys::{HotkeyEvent, HotkeyReceiver},
        startup,
        tray::{TrayEvent, TrayReceiver},
        windows,
    },
    ui::theme,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AppMode {
    Workspace,
    Overlay,
    Note,
    Toast,
    Annotate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CaptureAction {
    CopyOnly,
    AddEvidence,
    CaptureNote,
}

enum WorkerMessage {
    PdfFinished {
        path: PathBuf,
        duration: Duration,
        result: Result<(), String>,
    },
    PngFinished {
        path: PathBuf,
        duration: Duration,
        result: Result<(), String>,
    },
}

#[derive(Default)]
struct CaptureTimings {
    hotkey_to_overlay: Option<Duration>,
    desktop_capture: Option<Duration>,
    crop: Option<Duration>,
    release_to_clipboard: Option<Duration>,
}

#[derive(Clone)]
struct PinnedCapture {
    texture: TextureHandle,
    width: u32,
    height: u32,
}

pub struct ProofSnipApp {
    mode: AppMode,
    hotkeys: HotkeyReceiver,
    session: EvidenceSession,
    overlay_frame: Option<BgraFrame>,
    overlay_texture: Option<TextureHandle>,
    selection_start: Option<PixelPoint>,
    selection_end: Option<PixelPoint>,
    last_region: Option<PixelRect>,
    last_capture: Option<BgraFrame>,
    last_capture_texture: Option<TextureHandle>,
    evidence_textures: HashMap<u64, TextureHandle>,
    capture_action: CaptureAction,
    note_draft: String,
    note_frame: Option<BgraFrame>,
    note_anchor: PixelRect,
    toast_until: Option<Instant>,
    toast_text: String,
    status: String,
    timings: CaptureTimings,
    timing_history: VecDeque<String>,
    worker_sender: Sender<WorkerMessage>,
    worker_receiver: Receiver<WorkerMessage>,
    export_in_progress: bool,
    pinned_capture: Option<PinnedCapture>,
    startup_enabled: bool,
    tray: Option<TrayReceiver>,
    exit_requested: bool,
    annotation_source: Option<BgraFrame>,
    annotation_texture: Option<TextureHandle>,
    annotation_document: AnnotationDocument,
    annotation_tool: AnnotationTool,
    annotation_drag_start: Option<AnnotationPoint>,
    annotation_text: String,
}

impl ProofSnipApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);
        let hotkeys = HotkeyReceiver::start(cc.egui_ctx.clone());
        let (worker_sender, worker_receiver) = mpsc::channel();
        let startup_enabled = startup::is_enabled().unwrap_or(false);
        let (tray, status) = match TrayReceiver::start(cc.egui_ctx.clone()) {
            Ok(tray) => (
                Some(tray),
                "Ready · Ctrl+Shift+4 captures a region".to_owned(),
            ),
            Err(error) => (
                None,
                format!("Ready · notification area unavailable: {error}"),
            ),
        };
        Self {
            mode: AppMode::Workspace,
            hotkeys,
            session: EvidenceSession::default(),
            overlay_frame: None,
            overlay_texture: None,
            selection_start: None,
            selection_end: None,
            last_region: None,
            last_capture: None,
            last_capture_texture: None,
            evidence_textures: HashMap::new(),
            capture_action: CaptureAction::CopyOnly,
            note_draft: String::new(),
            note_frame: None,
            note_anchor: PixelRect::default(),
            toast_until: None,
            toast_text: String::new(),
            status,
            timings: CaptureTimings::default(),
            timing_history: VecDeque::new(),
            worker_sender,
            worker_receiver,
            export_in_progress: false,
            pinned_capture: None,
            startup_enabled,
            tray,
            exit_requested: false,
            annotation_source: None,
            annotation_texture: None,
            annotation_document: AnnotationDocument::new(),
            annotation_tool: AnnotationTool::Arrow,
            annotation_drag_start: None,
            annotation_text: String::new(),
        }
    }

    fn poll_tray(&mut self, context: &egui::Context) {
        while let Some(event) = self.tray.as_ref().and_then(|tray| tray.try_recv().ok()) {
            match event {
                TrayEvent::ShowWorkspace => {
                    self.mode = AppMode::Workspace;
                    if let Err(error) = windows::show_workspace() {
                        self.status = error;
                    }
                }
                TrayEvent::Exit => {
                    self.exit_requested = true;
                    context.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
    }

    fn poll_hotkeys(&mut self, context: &egui::Context) {
        while let Some(event) = self.hotkeys.try_recv() {
            match event {
                Ok(HotkeyEvent::CaptureRegion) => {
                    self.begin_capture(context, CaptureAction::CopyOnly, Some(Instant::now()))
                }
                Ok(HotkeyEvent::CaptureSameRegion) => self.capture_same_region(context),
                Ok(HotkeyEvent::CaptureMonitor) => match windows::monitor_under_cursor() {
                    Ok(rect) => self.capture_rect(context, rect, "monitor"),
                    Err(error) => self.status = error,
                },
                Ok(HotkeyEvent::CaptureActiveWindow) => match windows::active_window_rect() {
                    Ok(rect) => self.capture_rect(context, rect, "active window"),
                    Err(error) => self.status = error,
                },
                Ok(HotkeyEvent::ShowWorkspace) => {
                    self.mode = AppMode::Workspace;
                    if let Err(error) = windows::show_workspace() {
                        self.status = error;
                    }
                    context.request_repaint();
                }
                Err(error) => self.status = error,
            }
        }
    }

    fn poll_workers(&mut self) {
        while let Ok(message) = self.worker_receiver.try_recv() {
            match message {
                WorkerMessage::PdfFinished {
                    path,
                    duration,
                    result,
                } => {
                    self.export_in_progress = false;
                    self.status = match result {
                        Ok(()) => format!(
                            "PDF exported to {} in {:.1} ms",
                            path.display(),
                            duration.as_secs_f64() * 1000.0
                        ),
                        Err(error) => format!("PDF export failed: {error}"),
                    };
                }
                WorkerMessage::PngFinished {
                    path,
                    duration,
                    result,
                } => {
                    self.status = match result {
                        Ok(()) => format!(
                            "PNG saved to {} in {:.1} ms",
                            path.display(),
                            duration.as_secs_f64() * 1000.0
                        ),
                        Err(error) => format!("PNG save failed: {error}"),
                    };
                }
            }
        }
    }

    fn begin_capture(
        &mut self,
        context: &egui::Context,
        action: CaptureAction,
        hotkey_received: Option<Instant>,
    ) {
        if let Err(error) = windows::hide_for_capture() {
            self.status = error;
            return;
        }
        let capture_started = Instant::now();
        match dxgi::capture_desktop() {
            Ok(frame) => {
                self.timings.desktop_capture = Some(capture_started.elapsed());
                let bounds = frame.bounds();
                let texture = context.load_texture(
                    "desktop-capture",
                    frame.to_egui_image(),
                    egui::TextureOptions::LINEAR,
                );
                self.overlay_frame = Some(frame);
                self.overlay_texture = Some(texture);
                self.selection_start = None;
                self.selection_end = None;
                self.capture_action = action;
                self.mode = AppMode::Overlay;
                if let Err(error) = windows::show_overlay(bounds) {
                    self.status = error;
                    self.mode = AppMode::Workspace;
                    let _ = windows::show_workspace();
                    return;
                }
                if let Some(started) = hotkey_received {
                    self.timings.hotkey_to_overlay = Some(started.elapsed());
                }
                context.request_repaint();
            }
            Err(error) => {
                self.status = error;
                self.mode = AppMode::Workspace;
                let _ = windows::show_workspace();
            }
        }
    }

    fn capture_same_region(&mut self, context: &egui::Context) {
        let Some(region) = self.last_region else {
            self.status = "No previous region is available yet".into();
            self.mode = AppMode::Workspace;
            let _ = windows::show_workspace();
            return;
        };

        if let Err(error) = windows::hide_for_capture() {
            self.status = error;
            return;
        }
        let started = Instant::now();
        let result = dxgi::capture_desktop()
            .and_then(|desktop| desktop.crop(region))
            .and_then(|frame| {
                clipboard::copy_bgra_to_clipboard(&frame)?;
                Ok(frame)
            });
        match result {
            Ok(frame) => {
                self.last_capture_texture = Some(context.load_texture(
                    "last-capture",
                    frame.to_egui_image(),
                    egui::TextureOptions::LINEAR,
                ));
                self.last_capture = Some(frame);
                self.toast_text = format!(
                    "Copied same region · {} × {} · {:.1} ms",
                    region.width(),
                    region.height(),
                    started.elapsed().as_secs_f64() * 1000.0
                );
                self.mode = AppMode::Toast;
                self.toast_until = Some(Instant::now() + Duration::from_millis(650));
                let _ = windows::show_toast(region);
            }
            Err(error) => {
                self.status = error;
                self.mode = AppMode::Workspace;
                let _ = windows::show_workspace();
            }
        }
    }

    fn capture_rect(&mut self, context: &egui::Context, rect: PixelRect, kind: &str) {
        let started = Instant::now();
        if let Err(error) = windows::hide_for_capture() {
            self.status = error;
            return;
        }
        let capture_started = Instant::now();
        let result = dxgi::capture_desktop().and_then(|desktop| {
            self.timings.desktop_capture = Some(capture_started.elapsed());
            let crop_started = Instant::now();
            let frame = desktop.crop(rect)?;
            self.timings.crop = Some(crop_started.elapsed());
            let clipboard_started = Instant::now();
            clipboard::copy_bgra_to_clipboard(&frame)?;
            self.timings.release_to_clipboard = Some(clipboard_started.elapsed());
            Ok(frame)
        });
        match result {
            Ok(frame) => {
                self.last_region = Some(rect);
                self.last_capture_texture = Some(context.load_texture(
                    "last-capture",
                    frame.to_egui_image(),
                    egui::TextureOptions::LINEAR,
                ));
                self.last_capture = Some(frame.clone());
                self.toast_text = format!(
                    "Copied {kind} · {} × {} · {:.1} ms",
                    frame.width,
                    frame.height,
                    started.elapsed().as_secs_f64() * 1000.0
                );
                self.record_timings();
                self.mode = AppMode::Toast;
                self.toast_until = Some(Instant::now() + Duration::from_millis(650));
                let _ = windows::show_toast(rect);
            }
            Err(error) => {
                self.status = error;
                self.mode = AppMode::Workspace;
                let _ = windows::show_workspace();
            }
        }
    }

    fn render_overlay(&mut self, root: &mut egui::Ui) {
        let context = root.ctx().clone();
        let Some(frame) = self.overlay_frame.as_ref() else {
            self.cancel_overlay();
            return;
        };
        let Some(texture) = self.overlay_texture.as_ref() else {
            self.cancel_overlay();
            return;
        };

        if context.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.cancel_overlay();
            return;
        }

        let cursor = windows::cursor_position();
        let pressed = context.input(|input| input.pointer.primary_pressed());
        let down = context.input(|input| input.pointer.primary_down());
        let released = context.input(|input| input.pointer.primary_released());
        if pressed {
            self.selection_start = cursor;
            self.selection_end = cursor;
        } else if down && self.selection_start.is_some() {
            self.selection_end = cursor;
        }

        let native_bounds = frame.bounds();
        egui::CentralPanel::default()
            .frame(egui::Frame::new())
            .show(root, |ui| {
                let rect = ui.max_rect();
                let painter = ui.painter();
                painter.image(
                    texture.id(),
                    rect,
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );

                if let (Some(start), Some(end)) = (self.selection_start, self.selection_end) {
                    let selected = PixelRect::from_points(start, end).clamp_to(native_bounds);
                    let selected_ui = pixel_rect_to_ui(selected, native_bounds, rect);
                    paint_dimmed_outside(painter, rect, selected_ui);
                    painter.rect_stroke(
                        selected_ui,
                        CornerRadius::ZERO,
                        Stroke::new(2.0, theme::ACCENT),
                        StrokeKind::Inside,
                    );
                    for point in [
                        selected_ui.left_top(),
                        selected_ui.right_top(),
                        selected_ui.left_bottom(),
                        selected_ui.right_bottom(),
                    ] {
                        painter.circle_filled(point, 4.0, Color32::WHITE);
                        painter.circle_stroke(point, 4.0, Stroke::new(1.5, theme::ACCENT));
                    }
                    let label = format!("{} × {}", selected.width(), selected.height());
                    painter.text(
                        selected_ui.left_top() + egui::vec2(8.0, 8.0),
                        egui::Align2::LEFT_TOP,
                        label,
                        egui::FontId::proportional(13.0),
                        Color32::WHITE,
                    );
                } else {
                    painter.rect_filled(rect, CornerRadius::ZERO, Color32::from_black_alpha(92));
                    painter.text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "Drag to capture · Esc cancels",
                        egui::FontId::proportional(16.0),
                        Color32::WHITE,
                    );
                }
            });

        if released
            && let (Some(start), Some(end)) = (self.selection_start, cursor.or(self.selection_end))
        {
            let selected = PixelRect::from_points(start, end).clamp_to(native_bounds);
            if !selected.is_empty() && selected.width() > 1 && selected.height() > 1 {
                self.finish_selection(&context, selected);
            }
        }
        context.request_repaint_after(Duration::from_millis(8));
    }

    fn finish_selection(&mut self, context: &egui::Context, selected: PixelRect) {
        let released_at = Instant::now();
        let crop_started = Instant::now();
        let result = self
            .overlay_frame
            .as_ref()
            .ok_or_else(|| "capture frame disappeared".to_owned())
            .and_then(|frame| frame.crop(selected));
        self.timings.crop = Some(crop_started.elapsed());

        let frame = match result {
            Ok(frame) => frame,
            Err(error) => {
                self.status = error;
                self.cancel_overlay();
                return;
            }
        };
        if let Err(error) = clipboard::copy_bgra_to_clipboard(&frame) {
            self.status = error;
            self.cancel_overlay();
            return;
        }
        self.timings.release_to_clipboard = Some(released_at.elapsed());
        self.last_region = Some(selected);
        self.last_capture_texture = Some(context.load_texture(
            "last-capture",
            frame.to_egui_image(),
            egui::TextureOptions::LINEAR,
        ));
        self.last_capture = Some(frame.clone());
        self.record_timings();
        self.overlay_texture = None;
        self.overlay_frame = None;

        match self.capture_action {
            CaptureAction::CopyOnly => self.show_capture_toast(selected, &frame),
            CaptureAction::AddEvidence => {
                self.add_evidence_frame(context, frame.clone(), String::new());
                self.show_capture_toast(selected, &frame);
            }
            CaptureAction::CaptureNote => {
                self.note_frame = Some(frame);
                self.note_draft.clear();
                self.note_anchor = selected;
                self.mode = AppMode::Note;
                if let Err(error) = windows::show_note_prompt(selected) {
                    self.status = error;
                    self.mode = AppMode::Workspace;
                    let _ = windows::show_workspace();
                }
            }
        }
    }

    fn show_capture_toast(&mut self, selected: PixelRect, frame: &BgraFrame) {
        self.toast_text = format!("Copied · {} × {}", frame.width, frame.height);
        self.toast_until = Some(Instant::now() + Duration::from_millis(650));
        self.mode = AppMode::Toast;
        let _ = windows::show_toast(selected);
    }

    fn cancel_overlay(&mut self) {
        self.overlay_frame = None;
        self.overlay_texture = None;
        self.selection_start = None;
        self.selection_end = None;
        self.mode = AppMode::Workspace;
        if let Err(error) = windows::show_workspace() {
            self.status = error;
        }
    }

    fn record_timings(&mut self) {
        let format_duration = |value: Option<Duration>| {
            value
                .map(|duration| format!("{:.2} ms", duration.as_secs_f64() * 1000.0))
                .unwrap_or_else(|| "n/a".into())
        };
        let line = format!(
            "hotkey→overlay {} · desktop {} · crop {} · release→clipboard {}",
            format_duration(self.timings.hotkey_to_overlay),
            format_duration(self.timings.desktop_capture),
            format_duration(self.timings.crop),
            format_duration(self.timings.release_to_clipboard),
        );
        self.timing_history.push_front(line);
        self.timing_history.truncate(8);
    }

    fn render_toast(&mut self, root: &mut egui::Ui) {
        let context = root.ctx().clone();
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::SURFACE).inner_margin(12.0))
            .show(root, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.colored_label(theme::SUCCESS, "●");
                    ui.label(RichText::new(&self.toast_text).color(theme::TEXT).strong());
                });
            });
        if self
            .toast_until
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            windows::hide_window();
            self.toast_until = None;
            self.mode = AppMode::Workspace;
        } else {
            context.request_repaint_after(Duration::from_millis(25));
        }
    }

    fn render_note(&mut self, root: &mut egui::Ui) {
        let context = root.ctx().clone();
        let mut submit = context.input(|input| input.key_pressed(egui::Key::Enter));
        let cancel = context.input(|input| input.key_pressed(egui::Key::Escape));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::SURFACE).inner_margin(14.0))
            .show(root, |ui| {
                ui.label(RichText::new("Capture + Note").strong().color(theme::TEXT));
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.note_draft)
                        .hint_text("What does this screenshot prove?")
                        .desired_width(f32::INFINITY),
                );
                response.request_focus();
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Enter saves · Esc cancels")
                            .small()
                            .color(theme::MUTED),
                    );
                    if ui.button("Add to evidence").clicked() {
                        submit = true;
                    }
                });
            });

        if cancel {
            self.note_frame = None;
            windows::hide_window();
            self.mode = AppMode::Workspace;
        } else if submit {
            if let Some(frame) = self.note_frame.take() {
                let caption = self.note_draft.trim().to_owned();
                self.add_evidence_frame(&context, frame, caption);
                self.status = "Capture added to evidence".into();
            }
            windows::hide_window();
            self.mode = AppMode::Workspace;
        }
    }

    fn add_evidence_frame(&mut self, context: &egui::Context, frame: BgraFrame, caption: String) {
        let id = self.session.add_capture(frame.clone(), caption);
        let texture = context.load_texture(
            format!("evidence-{id}"),
            frame.to_egui_image(),
            egui::TextureOptions::LINEAR,
        );
        self.evidence_textures.insert(id, texture);
    }

    fn start_annotation(&mut self, context: &egui::Context) {
        let Some(frame) = self.last_capture.clone() else {
            self.status = "Capture an image before annotating it".into();
            return;
        };
        self.annotation_source = Some(frame);
        self.annotation_texture = self.last_capture_texture.clone();
        self.annotation_document.clear();
        self.annotation_tool = AnnotationTool::Arrow;
        self.annotation_drag_start = None;
        self.annotation_text.clear();
        self.mode = AppMode::Annotate;
        if let Err(error) = windows::show_workspace() {
            self.status = error;
        }
        context.request_repaint();
    }

    fn render_annotation(&mut self, root: &mut egui::Ui) {
        let context = root.ctx().clone();
        let Some(source) = self.annotation_source.as_ref() else {
            self.mode = AppMode::Workspace;
            return;
        };
        let Some(texture) = self.annotation_texture.clone() else {
            self.mode = AppMode::Workspace;
            return;
        };
        let image_width = source.width;
        let image_height = source.height;
        let mut copy_requested = false;
        let mut done_requested = false;
        let mut cancel_requested = false;

        if !context.egui_wants_keyboard_input() {
            context.input(|input| {
                if input.key_pressed(egui::Key::A) {
                    self.annotation_tool = AnnotationTool::Arrow;
                } else if input.key_pressed(egui::Key::R) {
                    self.annotation_tool = AnnotationTool::Rectangle;
                } else if input.key_pressed(egui::Key::H) {
                    self.annotation_tool = AnnotationTool::Highlight;
                } else if input.key_pressed(egui::Key::T) {
                    self.annotation_tool = AnnotationTool::Text;
                } else if input.key_pressed(egui::Key::B) {
                    self.annotation_tool = AnnotationTool::Redact;
                } else if input.key_pressed(egui::Key::Num1) {
                    self.annotation_tool = AnnotationTool::Marker;
                }
                copy_requested = input.key_pressed(egui::Key::C);
                done_requested = input.key_pressed(egui::Key::Enter);
                cancel_requested = input.key_pressed(egui::Key::Escape);
            });
        }

        egui::Panel::top("annotation-toolbar")
            .frame(
                egui::Frame::new()
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::symmetric(14, 10)),
            )
            .show(root, |ui| {
                ui.horizontal_wrapped(|ui| {
                    annotation_tool_button(
                        ui,
                        &mut self.annotation_tool,
                        AnnotationTool::Arrow,
                        "Arrow  A",
                    );
                    annotation_tool_button(
                        ui,
                        &mut self.annotation_tool,
                        AnnotationTool::Rectangle,
                        "Rectangle  R",
                    );
                    annotation_tool_button(
                        ui,
                        &mut self.annotation_tool,
                        AnnotationTool::Highlight,
                        "Highlight  H",
                    );
                    annotation_tool_button(
                        ui,
                        &mut self.annotation_tool,
                        AnnotationTool::Text,
                        "Text  T",
                    );
                    annotation_tool_button(
                        ui,
                        &mut self.annotation_tool,
                        AnnotationTool::Redact,
                        "Blur  B",
                    );
                    annotation_tool_button(
                        ui,
                        &mut self.annotation_tool,
                        AnnotationTool::Marker,
                        "Number  1",
                    );
                    ui.separator();
                    if ui
                        .add_enabled(
                            !self.annotation_document.items.is_empty(),
                            egui::Button::new("Undo"),
                        )
                        .clicked()
                    {
                        self.annotation_document.undo();
                    }
                    if ui
                        .add_enabled(
                            !self.annotation_document.items.is_empty(),
                            egui::Button::new("Clear"),
                        )
                        .clicked()
                    {
                        self.annotation_document.clear();
                    }
                    if ui.button("Copy  C").clicked() {
                        copy_requested = true;
                    }
                    if ui.button("Done  Enter").clicked() {
                        done_requested = true;
                    }
                    if ui.button("Cancel  Esc").clicked() {
                        cancel_requested = true;
                    }
                });
                if self.annotation_tool == AnnotationTool::Text {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.annotation_text)
                            .hint_text("Type text, then click the screenshot")
                            .desired_width(420.0),
                    );
                }
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::BG).inner_margin(12.0))
            .show(root, |ui| {
                let available = ui.available_size();
                let scale = (available.x / image_width as f32)
                    .min(available.y / image_height as f32)
                    .clamp(0.01, 1.0);
                let image_size =
                    egui::vec2(image_width as f32 * scale, image_height as f32 * scale);
                ui.centered_and_justified(|ui| {
                    let (image_rect, response) =
                        ui.allocate_exact_size(image_size, egui::Sense::click_and_drag());
                    ui.painter().image(
                        texture.id(),
                        image_rect,
                        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );

                    if response.drag_started() {
                        self.annotation_drag_start =
                            response.interact_pointer_pos().map(|position| {
                                ui_to_annotation(position, image_rect, image_width, image_height)
                            });
                    }
                    if response.drag_stopped()
                        && let (Some(start), Some(position)) = (
                            self.annotation_drag_start.take(),
                            response.interact_pointer_pos(),
                        )
                    {
                        let end = ui_to_annotation(position, image_rect, image_width, image_height);
                        self.add_drag_annotation(start, end);
                    }
                    if response.clicked()
                        && let Some(position) = response.interact_pointer_pos()
                    {
                        let point =
                            ui_to_annotation(position, image_rect, image_width, image_height);
                        match self.annotation_tool {
                            AnnotationTool::Marker => {
                                self.annotation_document.add_marker(point);
                            }
                            AnnotationTool::Text if !self.annotation_text.trim().is_empty() => {
                                self.annotation_document.add_item(AnnotationItem::Text {
                                    position: point,
                                    text: self.annotation_text.trim().to_owned(),
                                    size: 24,
                                });
                            }
                            _ => {}
                        }
                    }

                    for item in &self.annotation_document.items {
                        paint_annotation(ui.painter(), item, image_rect, image_width, image_height);
                    }
                    if let (Some(start), Some(position)) =
                        (self.annotation_drag_start, response.interact_pointer_pos())
                    {
                        let end = ui_to_annotation(position, image_rect, image_width, image_height);
                        if let Some(preview) = annotation_for_drag(self.annotation_tool, start, end)
                        {
                            paint_annotation(
                                ui.painter(),
                                &preview,
                                image_rect,
                                image_width,
                                image_height,
                            );
                        }
                    }
                });
            });

        if cancel_requested {
            self.annotation_source = None;
            self.annotation_texture = None;
            self.annotation_document.clear();
            self.mode = AppMode::Workspace;
            self.status = "Annotation cancelled".into();
        } else if done_requested {
            self.finish_annotation(&context, true);
        } else if copy_requested {
            self.copy_annotation_preview();
        }
    }

    fn add_drag_annotation(&mut self, start: AnnotationPoint, end: AnnotationPoint) {
        if let Some(item) = annotation_for_drag(self.annotation_tool, start, end) {
            self.annotation_document.add_item(item);
        }
    }

    fn copy_annotation_preview(&mut self) {
        let Some(source) = self.annotation_source.as_ref() else {
            return;
        };
        let rendered = self.annotation_document.render(source);
        self.status = match clipboard::copy_bgra_to_clipboard(&rendered) {
            Ok(()) => "Annotated screenshot copied".into(),
            Err(error) => error,
        };
    }

    fn finish_annotation(&mut self, context: &egui::Context, copy_to_clipboard: bool) {
        let Some(source) = self.annotation_source.take() else {
            self.mode = AppMode::Workspace;
            return;
        };
        let rendered = self.annotation_document.render(&source);
        let clipboard_result = if copy_to_clipboard {
            clipboard::copy_bgra_to_clipboard(&rendered)
        } else {
            Ok(())
        };
        self.last_capture_texture = Some(context.load_texture(
            "latest-annotated",
            rendered.to_egui_image(),
            egui::TextureOptions::LINEAR,
        ));
        self.last_capture = Some(rendered);
        self.annotation_texture = None;
        self.annotation_document.clear();
        self.mode = AppMode::Workspace;
        self.status = match clipboard_result {
            Ok(()) => "Annotations applied and screenshot copied".into(),
            Err(error) => format!("Annotations applied, but clipboard copy failed: {error}"),
        };
    }

    fn render_workspace(&mut self, root: &mut egui::Ui) {
        let context = root.ctx().clone();
        let mut capture_request = None;
        let mut export_requested = false;
        let mut add_last = false;
        let mut save_last = false;
        let mut annotate_last = false;
        let mut pin_last = false;
        let mut unpin = false;
        let mut startup_change = None;
        let mut reorder: Option<(usize, i32)> = None;
        let mut remove = None;

        egui::Panel::top("header")
            .frame(
                egui::Frame::new()
                    .fill(theme::BG)
                    .inner_margin(egui::Margin::symmetric(20, 14)),
            )
            .show(root, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.heading(RichText::new("ProofSnip").color(theme::TEXT));
                        ui.label(
                            RichText::new("Fast capture. Clean evidence.").color(theme::MUTED),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(!self.export_in_progress, egui::Button::new("Export PDF"))
                            .clicked()
                        {
                            export_requested = true;
                        }
                        if self.export_in_progress {
                            ui.spinner();
                            ui.label("Exporting…");
                        }
                    });
                });
            });

        egui::CentralPanel::default().show(root, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                theme::card().show(ui, |ui| {
                    ui.label(RichText::new("Capture").strong().color(theme::TEXT));
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Region   Ctrl+Shift+4").clicked() {
                            capture_request = Some(CaptureAction::CopyOnly);
                        }
                        if ui.button("Add to evidence").clicked() {
                            capture_request = Some(CaptureAction::AddEvidence);
                        }
                        if ui.button("Capture + Note").clicked() {
                            capture_request = Some(CaptureAction::CaptureNote);
                        }
                        ui.label(
                            RichText::new("Monitor Ctrl+Shift+7 · Active window Ctrl+Shift+8")
                                .small()
                                .color(theme::MUTED),
                        );
                        if ui
                            .add_enabled(
                                self.last_region.is_some(),
                                egui::Button::new("Same region   Ctrl+Shift+5"),
                            )
                            .clicked()
                        {
                            self.capture_same_region(&context);
                        }
                    });
                    ui.label(
                        RichText::new("Ctrl+Shift+6 reopens this workspace")
                            .small()
                            .color(theme::MUTED),
                    );
                });

                ui.add_space(10.0);
                theme::card().show(ui, |ui| {
                    ui.label(RichText::new("Settings").strong().color(theme::TEXT));
                    let response = ui.checkbox(
                        &mut self.startup_enabled,
                        "Start ProofSnip when I sign in to Windows",
                    );
                    if response.changed() {
                        startup_change = Some(self.startup_enabled);
                    }
                    ui.label(
                        RichText::new("Stored locally in the current user's Windows Run key.")
                            .small()
                            .color(theme::MUTED),
                    );
                });

                ui.add_space(10.0);
                if let (Some(frame), Some(texture)) =
                    (&self.last_capture, &self.last_capture_texture)
                {
                    theme::card().show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Latest capture").strong());
                            ui.label(
                                RichText::new(format!("{} × {}", frame.width, frame.height))
                                    .color(theme::MUTED),
                            );
                        });
                        let max_width = ui.available_width().min(520.0);
                        let scale = (max_width / frame.width as f32)
                            .min(220.0 / frame.height as f32)
                            .min(1.0);
                        ui.image((
                            texture.id(),
                            egui::vec2(frame.width as f32 * scale, frame.height as f32 * scale),
                        ));
                        ui.horizontal(|ui| {
                            if ui.button("Add to evidence").clicked() {
                                add_last = true;
                            }
                            if ui.button("Save PNG").clicked() {
                                save_last = true;
                            }
                            if ui.button("Annotate").clicked() {
                                annotate_last = true;
                            }
                            if ui.button("Pin latest").clicked() {
                                pin_last = true;
                            }
                            if self.pinned_capture.is_some() && ui.button("Unpin").clicked() {
                                unpin = true;
                            }
                        });
                    });
                    ui.add_space(10.0);
                }

                theme::card().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("Evidence session")
                                .strong()
                                .color(theme::TEXT),
                        );
                        ui.label(
                            RichText::new(format!("{} captures", self.session.captures.len()))
                                .color(theme::MUTED),
                        );
                    });
                    ui.add(
                        egui::TextEdit::singleline(&mut self.session.name)
                            .hint_text("Session title"),
                    );
                    ui.add(
                        egui::TextEdit::multiline(&mut self.session.description)
                            .hint_text("Optional context or reproduction notes")
                            .desired_rows(2),
                    );
                });

                ui.add_space(10.0);
                let capture_count = self.session.captures.len();
                for (index, capture) in self.session.captures.iter_mut().enumerate() {
                    let id = capture.id;
                    theme::card().show(ui, |ui| {
                        ui.horizontal_top(|ui| {
                            if let Some(texture) = self.evidence_textures.get(&id) {
                                let scale = (180.0 / capture.frame.width as f32)
                                    .min(112.0 / capture.frame.height as f32)
                                    .min(1.0);
                                ui.image((
                                    texture.id(),
                                    egui::vec2(
                                        capture.frame.width as f32 * scale,
                                        capture.frame.height as f32 * scale,
                                    ),
                                ));
                            }
                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(format!("Capture {}", index + 1)).strong(),
                                    );
                                    egui::ComboBox::from_id_salt(("label", id))
                                        .selected_text(capture.label.display())
                                        .show_ui(ui, |ui| {
                                            for label in EvidenceLabel::ALL {
                                                ui.selectable_value(
                                                    &mut capture.label,
                                                    label,
                                                    label.display(),
                                                );
                                            }
                                        });
                                });
                                ui.add(
                                    egui::TextEdit::multiline(&mut capture.caption)
                                        .hint_text("Optional caption")
                                        .desired_rows(2)
                                        .desired_width(f32::INFINITY),
                                );
                                ui.horizontal(|ui| {
                                    if ui.add_enabled(index > 0, egui::Button::new("↑")).clicked()
                                    {
                                        reorder = Some((index, -1));
                                    }
                                    if ui
                                        .add_enabled(
                                            index + 1 < capture_count,
                                            egui::Button::new("↓"),
                                        )
                                        .clicked()
                                    {
                                        reorder = Some((index, 1));
                                    }
                                    if ui
                                        .button(RichText::new("Remove").color(theme::DANGER))
                                        .clicked()
                                    {
                                        remove = Some(index);
                                    }
                                });
                            });
                        });
                    });
                    ui.add_space(8.0);
                }

                if self.session.captures.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(28.0);
                        ui.label(RichText::new("No evidence captures yet").color(theme::MUTED));
                        ui.label(
                            RichText::new("Normal snipping remains independent and fast.")
                                .small()
                                .color(theme::MUTED),
                        );
                        ui.add_space(28.0);
                    });
                }

                theme::card().show(ui, |ui| {
                    ui.label(RichText::new("Performance").strong());
                    if self.timing_history.is_empty() {
                        ui.label(
                            RichText::new("Capture timings appear here after the first snip.")
                                .color(theme::MUTED),
                        );
                    }
                    for timing in &self.timing_history {
                        ui.label(
                            RichText::new(timing)
                                .monospace()
                                .small()
                                .color(theme::MUTED),
                        );
                    }
                });
                ui.add_space(8.0);
                ui.label(RichText::new(&self.status).color(theme::MUTED));
            });
        });

        if let Some(action) = capture_request {
            self.begin_capture(&context, action, None);
        }
        if add_last && let Some(frame) = self.last_capture.clone() {
            self.add_evidence_frame(&context, frame, String::new());
            self.status = "Latest capture added to evidence".into();
        }
        if save_last {
            self.save_last_png(&context);
        }
        if annotate_last {
            self.start_annotation(&context);
        }
        if pin_last
            && let (Some(frame), Some(texture)) = (&self.last_capture, &self.last_capture_texture)
        {
            self.pinned_capture = Some(PinnedCapture {
                texture: texture.clone(),
                width: frame.width,
                height: frame.height,
            });
            self.status = "Latest capture pinned above other windows".into();
        }
        if unpin {
            self.pinned_capture = None;
            context.send_viewport_cmd_to(
                egui::ViewportId::from_hash_of("proofsnip-pin"),
                egui::ViewportCommand::Close,
            );
        }
        if let Some(enabled) = startup_change {
            match startup::set_enabled(enabled) {
                Ok(()) => {
                    self.status = if enabled {
                        "ProofSnip will start with Windows".into()
                    } else {
                        "ProofSnip removed from Windows startup".into()
                    };
                }
                Err(error) => {
                    self.startup_enabled = !enabled;
                    self.status = error;
                }
            }
        }
        if let Some((index, direction)) = reorder {
            if direction < 0 {
                self.session.move_up(index);
            } else {
                self.session.move_down(index);
            }
        }
        if let Some(index) = remove {
            let id = self.session.captures.get(index).map(|capture| capture.id);
            self.session.remove(index);
            if let Some(id) = id {
                self.evidence_textures.remove(&id);
            }
        }
        if export_requested {
            self.start_pdf_export(&context);
        }
    }

    fn save_last_png(&mut self, context: &egui::Context) {
        let Some(frame) = self.last_capture.clone() else {
            return;
        };
        let default_name = format!("ProofSnip-{}x{}.png", frame.width, frame.height);
        match dialog::choose_png_path(&default_name) {
            Ok(Some(path)) => {
                let sender = self.worker_sender.clone();
                let repaint = context.clone();
                thread::Builder::new()
                    .name("proofsnip-png".into())
                    .spawn(move || {
                        let started = Instant::now();
                        let result = wic::encode_png(&frame, &path);
                        let _ = sender.send(WorkerMessage::PngFinished {
                            path,
                            duration: started.elapsed(),
                            result,
                        });
                        repaint.request_repaint();
                    })
                    .expect("failed to start PNG worker");
                self.status = "Saving PNG in background…".into();
            }
            Ok(None) => {}
            Err(error) => self.status = error,
        }
    }

    fn start_pdf_export(&mut self, context: &egui::Context) {
        if self.session.captures.is_empty() {
            self.status = "Add at least one capture before exporting PDF".into();
            return;
        }
        let filename = default_pdf_filename(&self.session.name);
        let path = match dialog::choose_pdf_path(&filename) {
            Ok(Some(path)) => path,
            Ok(None) => return,
            Err(error) => {
                self.status = error;
                return;
            }
        };
        let model = self.session.export_model();
        let sender = self.worker_sender.clone();
        let repaint = context.clone();
        self.export_in_progress = true;
        self.status = "Exporting PDF in background…".into();
        thread::Builder::new()
            .name("proofsnip-pdf".into())
            .spawn(move || {
                let started = Instant::now();
                let result = PdfExporter.export(&model, &path);
                let _ = sender.send(WorkerMessage::PdfFinished {
                    path,
                    duration: started.elapsed(),
                    result,
                });
                repaint.request_repaint();
            })
            .expect("failed to start PDF worker");
    }

    fn render_pinned_capture(&mut self, context: &egui::Context) {
        let Some(pinned) = self.pinned_capture.clone() else {
            return;
        };
        let viewport_id = egui::ViewportId::from_hash_of("proofsnip-pin");
        let max_width = 900.0_f32;
        let max_height = 700.0_f32;
        let scale = (max_width / pinned.width as f32)
            .min(max_height / pinned.height as f32)
            .min(1.0);
        let image_size = egui::vec2(pinned.width as f32 * scale, pinned.height as f32 * scale);
        let viewport_size = image_size + egui::vec2(16.0, 16.0);
        let close_requested = context.show_viewport_immediate(
            viewport_id,
            egui::ViewportBuilder::default()
                .with_title("ProofSnip Pin")
                .with_inner_size(viewport_size)
                .with_min_inner_size([180.0, 120.0])
                .with_resizable(true)
                .with_always_on_top(),
            move |ui, _class| {
                let close_requested = ui.input(|input| input.viewport().close_requested());
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(Color32::BLACK).inner_margin(8.0))
                    .show(ui, |ui| {
                        let available = ui.available_size();
                        let fit = (available.x / pinned.width as f32)
                            .min(available.y / pinned.height as f32)
                            .min(1.0);
                        ui.centered_and_justified(|ui| {
                            ui.image((
                                pinned.texture.id(),
                                egui::vec2(pinned.width as f32 * fit, pinned.height as f32 * fit),
                            ));
                        });
                    });
                close_requested
            },
        );
        if close_requested {
            self.pinned_capture = None;
        }
    }
}

impl eframe::App for ProofSnipApp {
    fn logic(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_hotkeys(context);
        self.poll_tray(context);
        self.poll_workers();

        if self.tray.is_some()
            && !self.exit_requested
            && context.input(|input| input.viewport().close_requested())
        {
            context.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            windows::hide_window();
            self.status = "ProofSnip is still running in the notification area".into();
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let context = ui.ctx().clone();
        match self.mode {
            AppMode::Workspace => self.render_workspace(ui),
            AppMode::Overlay => self.render_overlay(ui),
            AppMode::Note => self.render_note(ui),
            AppMode::Toast => self.render_toast(ui),
            AppMode::Annotate => self.render_annotation(ui),
        }
        self.render_pinned_capture(&context);
    }
}

fn annotation_tool_button(
    ui: &mut egui::Ui,
    selected: &mut AnnotationTool,
    tool: AnnotationTool,
    text: &str,
) {
    if ui.selectable_label(*selected == tool, text).clicked() {
        *selected = tool;
    }
}

fn annotation_for_drag(
    tool: AnnotationTool,
    start: AnnotationPoint,
    end: AnnotationPoint,
) -> Option<AnnotationItem> {
    if start == end {
        return None;
    }
    match tool {
        AnnotationTool::Arrow => Some(AnnotationItem::Arrow {
            start,
            end,
            thickness: 4,
        }),
        AnnotationTool::Rectangle => Some(AnnotationItem::Rectangle {
            start,
            end,
            thickness: 4,
        }),
        AnnotationTool::Highlight => Some(AnnotationItem::Highlight { start, end }),
        AnnotationTool::Redact => Some(AnnotationItem::Redact {
            start,
            end,
            block_size: 12,
        }),
        AnnotationTool::Text | AnnotationTool::Marker => None,
    }
}

fn ui_to_annotation(
    position: egui::Pos2,
    image_rect: egui::Rect,
    width: u32,
    height: u32,
) -> AnnotationPoint {
    let x = ((position.x - image_rect.left()) / image_rect.width() * width as f32)
        .floor()
        .clamp(0.0, width.saturating_sub(1) as f32) as i32;
    let y = ((position.y - image_rect.top()) / image_rect.height() * height as f32)
        .floor()
        .clamp(0.0, height.saturating_sub(1) as f32) as i32;
    AnnotationPoint::new(x, y)
}

fn annotation_to_ui(
    point: AnnotationPoint,
    image_rect: egui::Rect,
    width: u32,
    height: u32,
) -> egui::Pos2 {
    egui::pos2(
        image_rect.left() + point.x as f32 / width.max(1) as f32 * image_rect.width(),
        image_rect.top() + point.y as f32 / height.max(1) as f32 * image_rect.height(),
    )
}

fn paint_annotation(
    painter: &egui::Painter,
    item: &AnnotationItem,
    image_rect: egui::Rect,
    width: u32,
    height: u32,
) {
    let red = Color32::from_rgb(230, 32, 32);
    let blue = Color32::from_rgb(35, 105, 220);
    let point = |point| annotation_to_ui(point, image_rect, width, height);
    match item {
        AnnotationItem::Arrow {
            start,
            end,
            thickness,
        } => {
            let start = point(*start);
            let end = point(*end);
            let stroke = Stroke::new(*thickness as f32, red);
            painter.line_segment([start, end], stroke);
            let vector = start - end;
            let length = vector.length();
            if length > 0.0 {
                let direction = vector / length;
                let perpendicular = egui::vec2(-direction.y, direction.x);
                let head = (length * 0.25).clamp(12.0, 28.0);
                painter.line_segment(
                    [end, end + direction * head + perpendicular * head * 0.5],
                    stroke,
                );
                painter.line_segment(
                    [end, end + direction * head - perpendicular * head * 0.5],
                    stroke,
                );
            }
        }
        AnnotationItem::Rectangle {
            start,
            end,
            thickness,
        } => {
            painter.rect_stroke(
                egui::Rect::from_two_pos(point(*start), point(*end)),
                CornerRadius::ZERO,
                Stroke::new(*thickness as f32, red),
                StrokeKind::Inside,
            );
        }
        AnnotationItem::Highlight { start, end } => {
            painter.rect_filled(
                egui::Rect::from_two_pos(point(*start), point(*end)),
                CornerRadius::ZERO,
                Color32::from_rgba_unmultiplied(255, 230, 0, 96),
            );
        }
        AnnotationItem::Text {
            position,
            text,
            size,
        } => {
            painter.text(
                point(*position),
                egui::Align2::LEFT_TOP,
                text,
                egui::FontId::proportional(*size as f32),
                Color32::WHITE,
            );
        }
        AnnotationItem::Redact { start, end, .. } => {
            painter.rect_filled(
                egui::Rect::from_two_pos(point(*start), point(*end)),
                CornerRadius::ZERO,
                Color32::from_black_alpha(190),
            );
        }
        AnnotationItem::Marker { center, number } => {
            let center = point(*center);
            painter.circle_filled(center, 14.0, blue);
            painter.circle_stroke(center, 14.0, Stroke::new(1.5, Color32::WHITE));
            painter.text(
                center,
                egui::Align2::CENTER_CENTER,
                number,
                egui::FontId::proportional(13.0),
                Color32::WHITE,
            );
        }
    }
}

fn pixel_rect_to_ui(selection: PixelRect, bounds: PixelRect, ui: egui::Rect) -> egui::Rect {
    let x_scale = ui.width() / bounds.width() as f32;
    let y_scale = ui.height() / bounds.height() as f32;
    egui::Rect::from_min_max(
        egui::pos2(
            ui.left() + (selection.left - bounds.left) as f32 * x_scale,
            ui.top() + (selection.top - bounds.top) as f32 * y_scale,
        ),
        egui::pos2(
            ui.left() + (selection.right - bounds.left) as f32 * x_scale,
            ui.top() + (selection.bottom - bounds.top) as f32 * y_scale,
        ),
    )
}

fn paint_dimmed_outside(painter: &egui::Painter, outer: egui::Rect, inner: egui::Rect) {
    let dim = Color32::from_black_alpha(118);
    painter.rect_filled(
        egui::Rect::from_min_max(outer.min, egui::pos2(outer.right(), inner.top())),
        CornerRadius::ZERO,
        dim,
    );
    painter.rect_filled(
        egui::Rect::from_min_max(egui::pos2(outer.left(), inner.bottom()), outer.max),
        CornerRadius::ZERO,
        dim,
    );
    painter.rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(outer.left(), inner.top()),
            egui::pos2(inner.left(), inner.bottom()),
        ),
        CornerRadius::ZERO,
        dim,
    );
    painter.rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(inner.right(), inner.top()),
            egui::pos2(outer.right(), inner.bottom()),
        ),
        CornerRadius::ZERO,
        dim,
    );
}
