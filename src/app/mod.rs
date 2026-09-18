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
    ui::{
        components::{self, StatusTone},
        theme,
    },
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

#[derive(Default)]
struct WorkspaceRequests {
    capture: Option<CaptureAction>,
    capture_monitor: bool,
    capture_active_window: bool,
    export: bool,
    add_last: bool,
    save_last: bool,
    annotate_last: bool,
    pin_last: bool,
    unpin: bool,
    startup_change: Option<bool>,
    reorder: Option<(usize, i32)>,
    remove: Option<usize>,
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
    selection_pointer_down: bool,
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
    workspace_was_visible_before_capture: bool,
    overlay_bounds_guard: Option<windows::OverlayBoundsGuard>,
}

impl ProofSnipApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);
        let capture_initialization_error = dxgi::initialize().err();
        let hotkeys = HotkeyReceiver::start(cc.egui_ctx.clone());
        let (worker_sender, worker_receiver) = mpsc::channel();
        let startup_enabled = startup::is_enabled().unwrap_or(false);
        let (tray, mut status) = match TrayReceiver::start(cc.egui_ctx.clone()) {
            Ok(tray) => (
                Some(tray),
                "Ready · Ctrl+Shift+4 captures a region".to_owned(),
            ),
            Err(error) => (
                None,
                format!("Ready · notification area unavailable: {error}"),
            ),
        };
        if let Some(error) = capture_initialization_error {
            status = format!("Capture initialization will retry on first use: {error}");
        }
        Self {
            mode: AppMode::Workspace,
            hotkeys,
            session: EvidenceSession::default(),
            overlay_frame: None,
            overlay_texture: None,
            selection_start: None,
            selection_end: None,
            selection_pointer_down: false,
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
            workspace_was_visible_before_capture: false,
            overlay_bounds_guard: None,
        }
    }

    fn poll_tray(&mut self, context: &egui::Context) {
        while let Some(event) = self.tray.as_ref().and_then(|tray| tray.try_recv().ok()) {
            match event {
                TrayEvent::ShowWorkspace => {
                    self.mode = AppMode::Workspace;
                    context.send_viewport_cmd(egui::ViewportCommand::Visible(true));
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
                    context.send_viewport_cmd(egui::ViewportCommand::Visible(true));
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
        self.workspace_was_visible_before_capture = windows::is_workspace_visible();
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
                self.selection_pointer_down = false;
                self.capture_action = action;
                self.mode = AppMode::Overlay;
                configure_overlay_viewport(context, bounds);
                if let Err(error) = windows::show_overlay(bounds) {
                    self.status = error;
                    self.mode = AppMode::Workspace;
                    self.restore_after_capture_interruption();
                    return;
                }
                match windows::OverlayBoundsGuard::start(bounds) {
                    Ok(guard) => self.overlay_bounds_guard = Some(guard),
                    Err(error) => {
                        self.status = error;
                        self.mode = AppMode::Workspace;
                        self.restore_after_capture_interruption();
                        return;
                    }
                }
                if let Some(started) = hotkey_received {
                    self.timings.hotkey_to_overlay = Some(started.elapsed());
                }
                context.request_repaint();
            }
            Err(error) => {
                self.status = error;
                self.mode = AppMode::Workspace;
                self.restore_after_capture_interruption();
            }
        }
    }

    fn capture_same_region(&mut self, context: &egui::Context) {
        let Some(region) = self.last_region else {
            self.status = "No previous region is available yet".into();
            self.mode = AppMode::Workspace;
            context.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            let _ = windows::show_workspace();
            return;
        };

        self.workspace_was_visible_before_capture = windows::is_workspace_visible();
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
                context.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                let _ = windows::show_toast(region);
            }
            Err(error) => {
                self.status = error;
                self.mode = AppMode::Workspace;
                self.restore_after_capture_interruption();
            }
        }
    }

    fn capture_rect(&mut self, context: &egui::Context, rect: PixelRect, kind: &str) {
        let started = Instant::now();
        self.workspace_was_visible_before_capture = windows::is_workspace_visible();
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
                context.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                let _ = windows::show_toast(rect);
            }
            Err(error) => {
                self.status = error;
                self.mode = AppMode::Workspace;
                self.restore_after_capture_interruption();
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
        let down = windows::primary_button_down();
        let pressed = down && !self.selection_pointer_down;
        let released = !down && self.selection_pointer_down;
        self.selection_pointer_down = down;
        if pressed {
            self.selection_start = cursor;
            self.selection_end = cursor;
        } else if down && self.selection_start.is_some() {
            self.selection_end = cursor;
        }

        let native_bounds = frame.bounds();
        if let Err(error) = windows::ensure_overlay_bounds(native_bounds) {
            self.status = error;
            self.cancel_overlay();
            return;
        }
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
                        Stroke::new(2.5, theme::ACCENT_HOVER),
                        StrokeKind::Inside,
                    );
                    for point in [
                        selected_ui.left_top(),
                        selected_ui.right_top(),
                        selected_ui.left_bottom(),
                        selected_ui.right_bottom(),
                    ] {
                        painter.circle_filled(point, 5.5, theme::SURFACE_ELEVATED);
                        painter.circle_stroke(point, 5.5, Stroke::new(2.0, theme::ACCENT_HOVER));
                    }
                    let label = format!("{} × {}", selected.width(), selected.height());
                    let label_size = egui::vec2(label.chars().count() as f32 * 7.5 + 20.0, 28.0);
                    let mut label_rect = egui::Rect::from_min_size(
                        selected_ui.left_top() + egui::vec2(8.0, 8.0),
                        label_size,
                    );
                    if !rect.contains_rect(label_rect) {
                        label_rect = egui::Rect::from_min_size(
                            selected_ui.left_bottom() + egui::vec2(8.0, -label_size.y - 8.0),
                            label_size,
                        );
                    }
                    painter.rect_filled(
                        label_rect,
                        CornerRadius::same(theme::RADIUS_SM),
                        theme::SURFACE_ELEVATED.gamma_multiply(0.94),
                    );
                    painter.rect_stroke(
                        label_rect,
                        CornerRadius::same(theme::RADIUS_SM),
                        Stroke::new(1.0, theme::BORDER_FOCUS),
                        StrokeKind::Inside,
                    );
                    painter.text(
                        label_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        label,
                        egui::FontId::proportional(13.0),
                        theme::TEXT,
                    );
                } else {
                    painter.rect_filled(rect, CornerRadius::ZERO, Color32::from_black_alpha(92));
                    let instruction_rect =
                        egui::Rect::from_center_size(rect.center(), egui::vec2(278.0, 46.0));
                    painter.rect_filled(
                        instruction_rect,
                        CornerRadius::same(theme::RADIUS_LG),
                        theme::SURFACE_ELEVATED.gamma_multiply(0.96),
                    );
                    painter.rect_stroke(
                        instruction_rect,
                        CornerRadius::same(theme::RADIUS_LG),
                        Stroke::new(1.0, theme::BORDER_FOCUS),
                        StrokeKind::Inside,
                    );
                    painter.text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "Drag to capture   ·   Esc cancels",
                        egui::FontId::proportional(16.0),
                        theme::TEXT,
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
        self.overlay_bounds_guard = None;
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
        self.overlay_bounds_guard = None;
        self.overlay_frame = None;
        self.overlay_texture = None;
        self.selection_start = None;
        self.selection_end = None;
        self.selection_pointer_down = false;
        self.mode = AppMode::Workspace;
        self.restore_after_capture_interruption();
    }

    fn restore_after_capture_interruption(&mut self) {
        if self.workspace_was_visible_before_capture {
            if let Err(error) = windows::show_workspace() {
                self.status = error;
            }
        } else {
            windows::hide_window();
        }
        self.workspace_was_visible_before_capture = false;
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
            .frame(
                egui::Frame::new()
                    .fill(theme::SURFACE_ELEVATED)
                    .stroke(Stroke::new(1.0, theme::SUCCESS.gamma_multiply(0.72)))
                    .corner_radius(CornerRadius::same(theme::RADIUS_LG))
                    .inner_margin(egui::Margin::symmetric(16, 12)),
            )
            .show(root, |ui| {
                ui.horizontal_centered(|ui| {
                    components::status_badge(ui, "Copied", StatusTone::Success);
                    ui.label(RichText::new(&self.toast_text).color(theme::TEXT).strong());
                });
            });
        if self
            .toast_until
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            windows::hide_window();
            context.send_viewport_cmd(egui::ViewportCommand::Visible(false));
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
            .frame(
                egui::Frame::new()
                    .fill(theme::SURFACE_ELEVATED)
                    .stroke(Stroke::new(1.0, theme::BORDER_FOCUS))
                    .corner_radius(CornerRadius::same(theme::RADIUS_LG))
                    .inner_margin(16.0),
            )
            .show(root, |ui| {
                ui.horizontal(|ui| {
                    components::status_badge(ui, "Evidence", StatusTone::Accent);
                    ui.label(
                        RichText::new("What does this capture show?")
                            .strong()
                            .color(theme::TEXT),
                    );
                });
                ui.add_space(theme::SPACE_1);
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.note_draft)
                        .hint_text("Add a short evidence note")
                        .desired_width(f32::INFINITY),
                );
                response.request_focus();
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Enter adds to evidence · Esc cancels")
                            .small()
                            .color(theme::MUTED),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if components::primary_button(ui, "Add to evidence").clicked() {
                            submit = true;
                        }
                    });
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
                    .stroke(Stroke::new(0.0, Color32::TRANSPARENT))
                    .inner_margin(egui::Margin::symmetric(18, 12)),
            )
            .show(root, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("Annotate capture")
                                .size(17.0)
                                .strong()
                                .color(theme::TEXT),
                        );
                        ui.label(
                            RichText::new("Choose a tool, then draw directly on the screenshot")
                                .small()
                                .color(theme::MUTED),
                        );
                    });
                    components::count_badge(ui, self.annotation_document.items.len());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        components::shortcut_chip(ui, "Enter");
                        if components::primary_button(ui, "Done").clicked() {
                            done_requested = true;
                        }
                        components::shortcut_chip(ui, "C");
                        if components::secondary_button(ui, "Copy").clicked() {
                            copy_requested = true;
                        }
                        if components::secondary_button(ui, "Cancel").clicked() {
                            cancel_requested = true;
                        }
                    });
                });
                ui.add_space(theme::SPACE_3);
                ui.separator();
                ui.add_space(theme::SPACE_2);
                ui.horizontal_wrapped(|ui| {
                    annotation_tool_button(
                        ui,
                        &mut self.annotation_tool,
                        AnnotationTool::Arrow,
                        "Arrow",
                        "A",
                    );
                    annotation_tool_button(
                        ui,
                        &mut self.annotation_tool,
                        AnnotationTool::Rectangle,
                        "Rectangle",
                        "R",
                    );
                    annotation_tool_button(
                        ui,
                        &mut self.annotation_tool,
                        AnnotationTool::Highlight,
                        "Highlight",
                        "H",
                    );
                    annotation_tool_button(
                        ui,
                        &mut self.annotation_tool,
                        AnnotationTool::Text,
                        "Text",
                        "T",
                    );
                    annotation_tool_button(
                        ui,
                        &mut self.annotation_tool,
                        AnnotationTool::Redact,
                        "Redact",
                        "B",
                    );
                    annotation_tool_button(
                        ui,
                        &mut self.annotation_tool,
                        AnnotationTool::Marker,
                        "Number",
                        "1",
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
                });
                if self.annotation_tool == AnnotationTool::Text {
                    ui.add_space(theme::SPACE_2);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.annotation_text)
                            .hint_text("Type text, then click the screenshot")
                            .desired_width(f32::INFINITY),
                    );
                }
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::BG).inner_margin(18.0))
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

    fn render_capture_section(
        &mut self,
        ui: &mut egui::Ui,
        context: &egui::Context,
        requests: &mut WorkspaceRequests,
    ) {
        components::card_frame().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                components::section_header(
                    ui,
                    "Capture",
                    Some("Fast paths for the screenshot you need right now"),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    components::status_badge(ui, "Clipboard first", StatusTone::Success);
                });
            });
            ui.add_space(theme::SPACE_4);

            components::section_frame().show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    if components::primary_button(ui, "Capture region").clicked() {
                        requests.capture = Some(CaptureAction::CopyOnly);
                    }
                    components::shortcut_chip(ui, "Ctrl+Shift+4");
                    ui.label(
                        RichText::new("Drag, release, and paste immediately.")
                            .small()
                            .color(theme::MUTED),
                    );
                });
            });

            ui.add_space(theme::SPACE_3);
            ui.horizontal_wrapped(|ui| {
                if components::secondary_button(ui, "Capture to evidence").clicked() {
                    requests.capture = Some(CaptureAction::AddEvidence);
                }
                if components::secondary_button(ui, "Capture + note").clicked() {
                    requests.capture = Some(CaptureAction::CaptureNote);
                }
                if components::secondary_button(ui, "Full monitor").clicked() {
                    requests.capture_monitor = true;
                }
                components::shortcut_chip(ui, "Ctrl+Shift+7");
                if components::secondary_button(ui, "Active window").clicked() {
                    requests.capture_active_window = true;
                }
                components::shortcut_chip(ui, "Ctrl+Shift+8");
            });

            ui.add_space(theme::SPACE_2);
            ui.horizontal_wrapped(|ui| {
                ui.add_enabled_ui(self.last_region.is_some(), |ui| {
                    if components::secondary_button(ui, "Capture same region").clicked() {
                        self.capture_same_region(context);
                    }
                });
                components::shortcut_chip(ui, "Ctrl+Shift+5");
                ui.label(
                    RichText::new("Workspace shortcut: Ctrl+Shift+6")
                        .small()
                        .color(theme::MUTED),
                );
            });
        });
    }

    fn render_latest_section(&self, ui: &mut egui::Ui, requests: &mut WorkspaceRequests) {
        components::card_frame().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            components::section_header(ui, "Latest capture", Some("Review once, then keep moving"));
            ui.add_space(theme::SPACE_3);

            if let (Some(frame), Some(texture)) = (&self.last_capture, &self.last_capture_texture) {
                ui.horizontal_wrapped(|ui| {
                    components::status_badge(
                        ui,
                        format!("{} × {}", frame.width, frame.height),
                        StatusTone::Neutral,
                    );
                    components::status_badge(ui, "Ready", StatusTone::Success);
                    if self.pinned_capture.is_some() {
                        components::status_badge(ui, "Pinned", StatusTone::Accent);
                    }
                });
                ui.add_space(theme::SPACE_2);

                let available_width = ui.available_width().max(1.0);
                let preview_height = 242.0;
                let scale = (available_width / frame.width as f32)
                    .min(preview_height / frame.height as f32)
                    .min(1.0);
                egui::Frame::new()
                    .fill(theme::BG)
                    .stroke(Stroke::new(1.0, theme::BORDER))
                    .corner_radius(CornerRadius::same(theme::RADIUS_MD))
                    .inner_margin(theme::SPACE_2)
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.centered_and_justified(|ui| {
                            ui.image((
                                texture.id(),
                                egui::vec2(frame.width as f32 * scale, frame.height as f32 * scale),
                            ));
                        });
                    });

                ui.add_space(theme::SPACE_3);
                ui.horizontal_wrapped(|ui| {
                    if components::primary_button(ui, "Add to evidence").clicked() {
                        requests.add_last = true;
                    }
                    if components::secondary_button(ui, "Annotate").clicked() {
                        requests.annotate_last = true;
                    }
                    if components::secondary_button(ui, "Save PNG").clicked() {
                        requests.save_last = true;
                    }
                    if self.pinned_capture.is_some() {
                        if components::secondary_button(ui, "Unpin").clicked() {
                            requests.unpin = true;
                        }
                    } else if components::secondary_button(ui, "Pin above windows").clicked() {
                        requests.pin_last = true;
                    }
                });
            } else {
                components::section_frame().show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.vertical_centered(|ui| {
                        ui.add_space(theme::SPACE_5);
                        ui.label(
                            RichText::new("Your latest capture will appear here")
                                .strong()
                                .color(theme::TEXT),
                        );
                        ui.label(
                            RichText::new("Press Ctrl+Shift+4 to capture a region.")
                                .small()
                                .color(theme::MUTED),
                        );
                        ui.add_space(theme::SPACE_5);
                    });
                });
            }
        });
    }

    fn render_session_section(&mut self, ui: &mut egui::Ui) {
        components::card_frame().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                components::section_header(
                    ui,
                    "Evidence session",
                    Some("Give the collection enough context to stand on its own"),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    components::count_badge(ui, self.session.captures.len());
                });
            });
            ui.add_space(theme::SPACE_3);
            ui.add(
                egui::TextEdit::singleline(&mut self.session.name)
                    .hint_text("Evidence title")
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(theme::SPACE_2);
            ui.add(
                egui::TextEdit::multiline(&mut self.session.description)
                    .hint_text("Optional context, environment, or reproduction notes")
                    .desired_rows(2)
                    .desired_width(f32::INFINITY),
            );
        });
    }

    fn render_evidence_cards(&mut self, ui: &mut egui::Ui, requests: &mut WorkspaceRequests) {
        ui.horizontal(|ui| {
            components::section_header(
                ui,
                "Evidence captures",
                Some("Order the story, label each step, and keep captions concise"),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                components::count_badge(ui, self.session.captures.len());
            });
        });
        ui.add_space(theme::SPACE_3);

        if self.session.captures.is_empty() {
            components::card_frame().show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.vertical_centered(|ui| {
                    ui.add_space(theme::SPACE_6);
                    ui.label(
                        RichText::new("No evidence captures yet")
                            .size(17.0)
                            .strong()
                            .color(theme::TEXT),
                    );
                    ui.add_space(theme::SPACE_1);
                    ui.label(
                        RichText::new(
                            "Capture directly to evidence, or add the latest screenshot when ready.",
                        )
                        .color(theme::MUTED),
                    );
                    ui.add_space(theme::SPACE_6);
                });
            });
            return;
        }

        let capture_count = self.session.captures.len();
        for (index, capture) in self.session.captures.iter_mut().enumerate() {
            let id = capture.id;
            components::card_frame().show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal_top(|ui| {
                    egui::Frame::new()
                        .fill(theme::BG)
                        .stroke(Stroke::new(1.0, theme::BORDER))
                        .corner_radius(CornerRadius::same(theme::RADIUS_MD))
                        .inner_margin(theme::SPACE_2)
                        .show(ui, |ui| {
                            ui.set_width(214.0);
                            ui.set_height(132.0);
                            if let Some(texture) = self.evidence_textures.get(&id) {
                                let scale = (198.0 / capture.frame.width as f32)
                                    .min(116.0 / capture.frame.height as f32)
                                    .min(1.0);
                                ui.centered_and_justified(|ui| {
                                    ui.image((
                                        texture.id(),
                                        egui::vec2(
                                            capture.frame.width as f32 * scale,
                                            capture.frame.height as f32 * scale,
                                        ),
                                    ));
                                });
                            }
                        });

                    ui.vertical(|ui| {
                        ui.set_min_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!("Capture {}", index + 1))
                                    .size(16.0)
                                    .strong()
                                    .color(theme::TEXT),
                            );
                            components::status_badge(
                                ui,
                                format!("{} × {}", capture.frame.width, capture.frame.height),
                                StatusTone::Neutral,
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    egui::ComboBox::from_id_salt(("label", id))
                                        .selected_text(capture.label.display())
                                        .width(118.0)
                                        .show_ui(ui, |ui| {
                                            for label in EvidenceLabel::ALL {
                                                ui.selectable_value(
                                                    &mut capture.label,
                                                    label,
                                                    label.display(),
                                                );
                                            }
                                        });
                                    ui.label(RichText::new("Stage").small().color(theme::MUTED));
                                },
                            );
                        });
                        ui.add_space(theme::SPACE_2);
                        ui.add(
                            egui::TextEdit::multiline(&mut capture.caption)
                                .hint_text("What does this screenshot prove?")
                                .desired_rows(3)
                                .desired_width(f32::INFINITY),
                        );
                        ui.add_space(theme::SPACE_2);
                        ui.horizontal_wrapped(|ui| {
                            ui.add_enabled_ui(index > 0, |ui| {
                                if components::secondary_button(ui, "Move up").clicked() {
                                    requests.reorder = Some((index, -1));
                                }
                            });
                            ui.add_enabled_ui(index + 1 < capture_count, |ui| {
                                if components::secondary_button(ui, "Move down").clicked() {
                                    requests.reorder = Some((index, 1));
                                }
                            });
                            if components::danger_button(
                                ui,
                                RichText::new("Remove").color(theme::DANGER),
                            )
                            .clicked()
                            {
                                requests.remove = Some(index);
                            }
                        });
                    });
                });
            });
            ui.add_space(theme::SPACE_3);
        }
    }

    fn render_settings_card(&mut self, ui: &mut egui::Ui, requests: &mut WorkspaceRequests) {
        components::card_frame().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            components::section_header(
                ui,
                "Startup",
                Some("Keep ProofSnip ready without opening the workspace"),
            );
            ui.add_space(theme::SPACE_3);
            let response = ui.checkbox(
                &mut self.startup_enabled,
                "Start ProofSnip when I sign in to Windows",
            );
            if response.changed() {
                requests.startup_change = Some(self.startup_enabled);
            }
            ui.label(
                RichText::new("Stored locally in the current user's Windows Run key.")
                    .small()
                    .color(theme::MUTED),
            );
        });
    }

    fn render_diagnostics_card(&self, ui: &mut egui::Ui) {
        components::card_frame().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            egui::CollapsingHeader::new(
                RichText::new("Capture diagnostics")
                    .strong()
                    .color(theme::TEXT),
            )
            .default_open(false)
            .show(ui, |ui| {
                ui.label(
                    RichText::new("Development timings for the most recent capture paths.")
                        .small()
                        .color(theme::MUTED),
                );
                ui.add_space(theme::SPACE_2);
                if self.timing_history.is_empty() {
                    ui.label(
                        RichText::new("Capture timings appear after the first snip.")
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
        });
    }

    fn render_workspace(&mut self, root: &mut egui::Ui) {
        let context = root.ctx().clone();
        let mut requests = WorkspaceRequests::default();
        let has_evidence = !self.session.captures.is_empty();

        egui::Panel::top("header")
            .frame(
                egui::Frame::new()
                    .fill(theme::BG)
                    .stroke(Stroke::new(0.0, Color32::TRANSPARENT))
                    .inner_margin(egui::Margin::symmetric(24, 16)),
            )
            .show(root, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.heading(
                                RichText::new("ProofSnip")
                                    .size(25.0)
                                    .strong()
                                    .color(theme::TEXT),
                            );
                            components::status_badge(ui, "Resident", StatusTone::Success);
                        });
                        ui.label(
                            RichText::new("Capture instantly. Build evidence deliberately.")
                                .color(theme::MUTED),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.export_in_progress {
                            ui.spinner();
                            components::status_badge(ui, "Exporting PDF", StatusTone::Accent);
                        } else {
                            ui.add_enabled_ui(has_evidence, |ui| {
                                if components::primary_button(ui, "Export evidence PDF").clicked() {
                                    requests.export = true;
                                }
                            });
                        }
                    });
                });
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme::BG)
                    .inner_margin(egui::Margin::symmetric(22, 18)),
            )
            .show(root, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let content_width = ui.available_width().min(1180.0);
                        let left_padding = ((ui.available_width() - content_width) * 0.5).max(0.0);
                        ui.horizontal(|ui| {
                            ui.add_space(left_padding);
                            ui.allocate_ui_with_layout(
                                egui::vec2(content_width, 0.0),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    components::status_banner(
                                        ui,
                                        status_tone(&self.status),
                                        self.status.clone(),
                                    );
                                    ui.add_space(theme::SPACE_4);

                                    if ui.available_width() >= 860.0 {
                                        ui.columns(2, |columns| {
                                            self.render_capture_section(
                                                &mut columns[0],
                                                &context,
                                                &mut requests,
                                            );
                                            self.render_latest_section(
                                                &mut columns[1],
                                                &mut requests,
                                            );
                                        });
                                    } else {
                                        self.render_capture_section(ui, &context, &mut requests);
                                        ui.add_space(theme::SPACE_3);
                                        self.render_latest_section(ui, &mut requests);
                                    }

                                    ui.add_space(theme::SPACE_4);
                                    self.render_session_section(ui);
                                    ui.add_space(theme::SPACE_5);
                                    self.render_evidence_cards(ui, &mut requests);
                                    ui.add_space(theme::SPACE_3);

                                    if ui.available_width() >= 860.0 {
                                        ui.columns(2, |columns| {
                                            self.render_settings_card(
                                                &mut columns[0],
                                                &mut requests,
                                            );
                                            self.render_diagnostics_card(&mut columns[1]);
                                        });
                                    } else {
                                        self.render_settings_card(ui, &mut requests);
                                        ui.add_space(theme::SPACE_3);
                                        self.render_diagnostics_card(ui);
                                    }
                                    ui.add_space(theme::SPACE_5);
                                },
                            );
                        });
                    });
            });

        if let Some(action) = requests.capture {
            self.begin_capture(&context, action, None);
        }
        if requests.capture_monitor {
            match windows::monitor_under_cursor() {
                Ok(rect) => self.capture_rect(&context, rect, "monitor"),
                Err(error) => self.status = error,
            }
        }
        if requests.capture_active_window {
            match windows::active_window_rect() {
                Ok(rect) => self.capture_rect(&context, rect, "active window"),
                Err(error) => self.status = error,
            }
        }
        if requests.add_last
            && let Some(frame) = self.last_capture.clone()
        {
            self.add_evidence_frame(&context, frame, String::new());
            self.status = "Latest capture added to evidence".into();
        }
        if requests.save_last {
            self.save_last_png(&context);
        }
        if requests.annotate_last {
            self.start_annotation(&context);
        }
        if requests.pin_last
            && let (Some(frame), Some(texture)) = (&self.last_capture, &self.last_capture_texture)
        {
            self.pinned_capture = Some(PinnedCapture {
                texture: texture.clone(),
                width: frame.width,
                height: frame.height,
            });
            self.status = "Latest capture pinned above other windows".into();
        }
        if requests.unpin {
            self.pinned_capture = None;
            context.send_viewport_cmd_to(
                egui::ViewportId::from_hash_of("proofsnip-pin"),
                egui::ViewportCommand::Close,
            );
            self.status = "Pinned capture closed".into();
        }
        if let Some(enabled) = requests.startup_change {
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
        if let Some((index, direction)) = requests.reorder {
            if direction < 0 {
                self.session.move_up(index);
            } else {
                self.session.move_down(index);
            }
            self.status = "Evidence order updated".into();
        }
        if let Some(index) = requests.remove {
            let id = self.session.captures.get(index).map(|capture| capture.id);
            self.session.remove(index);
            if let Some(id) = id {
                self.evidence_textures.remove(&id);
            }
            self.status = "Evidence capture removed".into();
        }
        if requests.export {
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
                    .frame(
                        egui::Frame::new()
                            .fill(theme::BG)
                            .stroke(Stroke::new(1.0, theme::BORDER_FOCUS))
                            .inner_margin(10.0),
                    )
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

        if self.mode == AppMode::Overlay {
            if let Some(bounds) = self.overlay_frame.as_ref().map(BgraFrame::bounds)
                && let Err(error) = windows::ensure_overlay_bounds(bounds)
            {
                self.status = error;
                self.cancel_overlay();
            } else {
                context.request_repaint_after(Duration::from_millis(8));
            }
        }

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

fn status_tone(status: &str) -> StatusTone {
    let status = status.to_ascii_lowercase();
    if status.contains("failed")
        || status.contains("error")
        || status.contains("could not")
        || status.contains("unavailable")
    {
        StatusTone::Danger
    } else if status.contains("saving") || status.contains("exporting") {
        StatusTone::Accent
    } else if status.contains("cancel") || status.contains("no previous") {
        StatusTone::Warning
    } else if status.contains("copied")
        || status.contains("saved")
        || status.contains("exported")
        || status.contains("added")
        || status.contains("applied")
        || status.contains("updated")
        || status.contains("ready")
    {
        StatusTone::Success
    } else {
        StatusTone::Neutral
    }
}

fn annotation_tool_button(
    ui: &mut egui::Ui,
    selected: &mut AnnotationTool,
    tool: AnnotationTool,
    label: &str,
    shortcut: &str,
) {
    let active = *selected == tool;
    let response = ui.add(
        egui::Button::new(
            RichText::new(format!("{label}   {shortcut}"))
                .strong()
                .color(if active { Color32::WHITE } else { theme::TEXT }),
        )
        .fill(if active {
            theme::ACCENT
        } else {
            theme::SURFACE_ELEVATED
        })
        .stroke(Stroke::new(
            1.0,
            if active {
                theme::ACCENT_HOVER
            } else {
                theme::BORDER
            },
        ))
        .corner_radius(CornerRadius::same(theme::RADIUS_MD))
        .min_size(egui::vec2(0.0, theme::CONTROL_HEIGHT)),
    );
    if response.clicked() {
        *selected = tool;
    }
}

fn configure_overlay_viewport(context: &egui::Context, bounds: PixelRect) {
    let pixels_per_point = context.pixels_per_point().max(0.1);
    context.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(egui::vec2(1.0, 1.0)));
    context.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(
        bounds.left as f32 / pixels_per_point,
        bounds.top as f32 / pixels_per_point,
    )));
    context.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
        bounds.width() as f32 / pixels_per_point,
        bounds.height() as f32 / pixels_per_point,
    )));
    context.send_viewport_cmd(egui::ViewportCommand::Decorations(false));
    context.send_viewport_cmd(egui::ViewportCommand::Resizable(false));
    context.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
        egui::WindowLevel::AlwaysOnTop,
    ));
    context.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    context.send_viewport_cmd(egui::ViewportCommand::Focus);
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
