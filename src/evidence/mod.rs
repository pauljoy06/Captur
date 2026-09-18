use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(target_os = "windows")]
use std::{fs, path::Path};

use serde::{Deserialize, Serialize};

use crate::capture::BgraFrame;
#[cfg(target_os = "windows")]
use crate::encoding::wic;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum EvidenceLabel {
    #[default]
    None,
    Before,
    Action,
    After,
}

impl EvidenceLabel {
    pub const ALL: [Self; 4] = [Self::None, Self::Before, Self::Action, Self::After];

    pub const fn display(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Before => "Before",
            Self::Action => "Action",
            Self::After => "After",
        }
    }
}

#[derive(Clone, Debug)]
pub struct EvidenceCapture {
    pub id: u64,
    pub frame: BgraFrame,
    pub caption: String,
    pub label: EvidenceLabel,
    pub captured_at_unix_ms: u64,
    #[allow(dead_code)]
    pub png_path: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct EvidenceSession {
    pub name: String,
    pub description: String,
    pub captures: Vec<EvidenceCapture>,
    next_id: u64,
}

impl Default for EvidenceSession {
    fn default() -> Self {
        Self {
            name: "Evidence Session".into(),
            description: String::new(),
            captures: Vec::new(),
            next_id: 1,
        }
    }
}

impl EvidenceSession {
    pub fn add_capture(&mut self, frame: BgraFrame, caption: String) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.captures.push(EvidenceCapture {
            id,
            frame,
            caption,
            label: EvidenceLabel::None,
            captured_at_unix_ms: now_unix_ms(),
            png_path: None,
        });
        id
    }

    pub fn move_up(&mut self, index: usize) {
        if index > 0 && index < self.captures.len() {
            self.captures.swap(index, index - 1);
        }
    }

    pub fn move_down(&mut self, index: usize) {
        if index + 1 < self.captures.len() {
            self.captures.swap(index, index + 1);
        }
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.captures.len() {
            self.captures.remove(index);
        }
    }

    pub fn export_model(&self) -> PdfExportModel {
        PdfExportModel {
            title: self.name.trim().to_owned(),
            description: self.description.trim().to_owned(),
            generated_at_unix_ms: now_unix_ms(),
            captures: self
                .captures
                .iter()
                .enumerate()
                .map(|(index, capture)| PdfExportCapture {
                    number: index + 1,
                    frame: capture.frame.clone(),
                    caption: capture.caption.trim().to_owned(),
                    label: capture.label,
                    captured_at_unix_ms: capture.captured_at_unix_ms,
                })
                .collect(),
        }
    }

    #[cfg(target_os = "windows")]
    #[allow(dead_code)]
    pub fn save_folder(&mut self, root: &Path) -> Result<(), String> {
        fs::create_dir_all(root)
            .map_err(|error| format!("could not create session folder: {error}"))?;
        for (index, capture) in self.captures.iter_mut().enumerate() {
            let filename = format!("{:03}.png", index + 1);
            let path = root.join(&filename);
            wic::encode_png(&capture.frame, &path)?;
            capture.png_path = Some(path);
        }

        let metadata = SessionMetadata {
            name: self.name.clone(),
            description: self.description.clone(),
            captures: self
                .captures
                .iter()
                .enumerate()
                .map(|(index, capture)| CaptureMetadata {
                    id: capture.id,
                    order: index,
                    caption: capture.caption.clone(),
                    label: capture.label,
                    captured_at_unix_ms: capture.captured_at_unix_ms,
                    width: capture.frame.width,
                    height: capture.frame.height,
                    filename: capture
                        .png_path
                        .as_ref()
                        .and_then(|path| path.file_name())
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                })
                .collect(),
        };
        let json = serde_json::to_vec_pretty(&metadata)
            .map_err(|error| format!("could not serialize session: {error}"))?;
        fs::write(root.join("session.json"), json)
            .map_err(|error| format!("could not write session metadata: {error}"))
    }
}

#[derive(Clone, Debug)]
pub struct PdfExportModel {
    pub title: String,
    pub description: String,
    pub generated_at_unix_ms: u64,
    pub captures: Vec<PdfExportCapture>,
}

#[derive(Clone, Debug)]
pub struct PdfExportCapture {
    pub number: usize,
    pub frame: BgraFrame,
    pub caption: String,
    pub label: EvidenceLabel,
    pub captured_at_unix_ms: u64,
}

#[derive(Serialize, Deserialize)]
#[allow(dead_code)]
struct SessionMetadata {
    name: String,
    description: String,
    captures: Vec<CaptureMetadata>,
}

#[derive(Serialize, Deserialize)]
#[allow(dead_code)]
struct CaptureMetadata {
    id: u64,
    order: usize,
    caption: String,
    label: EvidenceLabel,
    captured_at_unix_ms: u64,
    width: u32,
    height: u32,
    filename: String,
}

pub fn sanitize_filename(name: &str) -> String {
    let mut result = String::with_capacity(name.len());
    let mut previous_dash = false;
    for character in name.trim().chars() {
        let valid = !matches!(
            character,
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
        ) && !character.is_control();
        let mapped = if valid && !character.is_whitespace() {
            character
        } else {
            '-'
        };
        if mapped == '-' {
            if !previous_dash {
                result.push('-');
            }
            previous_dash = true;
        } else {
            result.push(mapped);
            previous_dash = false;
        }
    }
    result
        .trim_matches([' ', '.', '-'])
        .chars()
        .take(100)
        .collect()
}

pub fn default_pdf_filename(session_name: &str) -> String {
    let sanitized = sanitize_filename(session_name);
    if sanitized.is_empty() || sanitized == "Evidence-Session" {
        format!("Captur-Evidence-{}.pdf", now_unix_ms())
    } else {
        format!("{sanitized}.pdf")
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_windows_filename_characters() {
        assert_eq!(
            sanitize_filename(" Order: total / proof? "),
            "Order-total-proof"
        );
    }
}
