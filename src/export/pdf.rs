use std::{fs, path::Path};

use printpdf::{
    Color, Mm, Op, ParsedFont, PdfDocument, PdfFontHandle, PdfPage, PdfSaveOptions, Point, Pt,
    RawImage, RawImageData, RawImageFormat, Rgb, TextItem, XObjectTransform,
};

use crate::{
    capture::BgraFrame,
    evidence::{EvidenceLabel, PdfExportCapture, PdfExportModel},
    export::Exporter,
};

const A4_WIDTH_MM: f32 = 210.0;
const A4_HEIGHT_MM: f32 = 297.0;
const MARGIN_MM: f32 = 14.0;
const HEADER_MM: f32 = 22.0;
const CAPTION_MM: f32 = 20.0;
const FOOTER_MM: f32 = 9.0;
const FONT_BYTES: &[u8] = include_bytes!("../../assets/DejaVuSans.ttf");

#[derive(Default)]
pub struct PdfExporter;

impl Exporter<PdfExportModel> for PdfExporter {
    fn export(&self, model: &PdfExportModel, path: &Path) -> Result<(), String> {
        if model.captures.is_empty() {
            return Err("the evidence session has no screenshots".into());
        }

        let mut font_warnings = Vec::new();
        let font = ParsedFont::from_bytes(FONT_BYTES, 0, &mut font_warnings)
            .ok_or_else(|| "embedded PDF font could not be parsed".to_owned())?;
        let mut document = PdfDocument::new(if model.title.is_empty() {
            "Captur Evidence"
        } else {
            &model.title
        });
        let font_id = document.add_font(&font);
        let font_handle = PdfFontHandle::External(font_id);
        let mut pages = Vec::new();

        for capture in &model.captures {
            let segments = split_tall_capture(&capture.frame);
            let segment_count = segments.len();
            for (segment_index, segment) in segments.into_iter().enumerate() {
                pages.push(build_capture_page(
                    &mut document,
                    model,
                    capture,
                    segment,
                    segment_index,
                    segment_count,
                    &font_handle,
                ));
            }
        }

        let mut pdf_warnings = Vec::new();
        let bytes = document.with_pages(pages).save(
            &PdfSaveOptions {
                subset_fonts: true,
                ..Default::default()
            },
            &mut pdf_warnings,
        );
        fs::write(path, bytes).map_err(|error| format!("could not write PDF: {error}"))
    }
}

fn build_capture_page(
    document: &mut PdfDocument,
    model: &PdfExportModel,
    capture: &PdfExportCapture,
    segment: BgraFrame,
    segment_index: usize,
    segment_count: usize,
    font: &PdfFontHandle,
) -> PdfPage {
    let landscape = segment.width as f32 / segment.height as f32 > 1.45;
    let (page_width, page_height) = if landscape {
        (A4_HEIGHT_MM, A4_WIDTH_MM)
    } else {
        (A4_WIDTH_MM, A4_HEIGHT_MM)
    };
    let mut operations = Vec::new();

    let title = if model.title.trim().is_empty() {
        "Captur Evidence"
    } else {
        model.title.trim()
    };
    add_text(
        &mut operations,
        font,
        15.0,
        MARGIN_MM,
        page_height - MARGIN_MM - 2.0,
        title,
        Rgb::new(0.10, 0.12, 0.16, None),
    );

    let label = match capture.label {
        EvidenceLabel::None => String::new(),
        other => format!(" · {}", other.display()),
    };
    let part = if segment_count > 1 {
        format!(" · part {}/{}", segment_index + 1, segment_count)
    } else {
        String::new()
    };
    add_text(
        &mut operations,
        font,
        10.0,
        MARGIN_MM,
        page_height - MARGIN_MM - 9.0,
        &format!("Capture {}{}{}", capture.number, label, part),
        Rgb::new(0.24, 0.29, 0.38, None),
    );

    if capture.number == 1 && segment_index == 0 && !model.description.is_empty() {
        let description = truncate_line(&model.description, 130);
        add_text(
            &mut operations,
            font,
            8.5,
            MARGIN_MM,
            page_height - MARGIN_MM - 15.0,
            &description,
            Rgb::new(0.38, 0.42, 0.50, None),
        );
    }

    let image_top = page_height - MARGIN_MM - HEADER_MM;
    let image_bottom = MARGIN_MM + FOOTER_MM + CAPTION_MM;
    let available_width = page_width - MARGIN_MM * 2.0;
    let available_height = image_top - image_bottom;
    let width_scale = available_width / segment.width as f32;
    let height_scale = available_height / segment.height as f32;
    let mm_per_pixel = width_scale.min(height_scale).min(25.4 / 96.0);
    let image_width_mm = segment.width as f32 * mm_per_pixel;
    let image_height_mm = segment.height as f32 * mm_per_pixel;
    let image_x_mm = (page_width - image_width_mm) / 2.0;
    let image_y_mm = image_bottom + (available_height - image_height_mm) / 2.0;

    let image = RawImage {
        pixels: RawImageData::U8(tight_bgra(&segment)),
        width: segment.width as usize,
        height: segment.height as usize,
        data_format: RawImageFormat::BGRA8,
        tag: Vec::new(),
    };
    let image_id = document.add_image(&image);
    let target_width_pt: Pt = Mm(image_width_mm).into();
    let target_height_pt: Pt = Mm(image_height_mm).into();
    operations.push(Op::UseXobject {
        id: image_id,
        transform: XObjectTransform {
            translate_x: Some(Mm(image_x_mm).into()),
            translate_y: Some(Mm(image_y_mm).into()),
            scale_x: Some(target_width_pt.0 / segment.width as f32),
            scale_y: Some(target_height_pt.0 / segment.height as f32),
            dpi: Some(72.0),
            ..Default::default()
        },
    });

    if !capture.caption.is_empty() && segment_index + 1 == segment_count {
        let lines = wrap_text(&capture.caption, if landscape { 115 } else { 85 }, 2);
        for (index, line) in lines.iter().enumerate() {
            add_text(
                &mut operations,
                font,
                9.0,
                MARGIN_MM,
                MARGIN_MM + FOOTER_MM + 8.0 - index as f32 * 4.2,
                line,
                Rgb::new(0.16, 0.19, 0.24, None),
            );
        }
    }

    add_text(
        &mut operations,
        font,
        7.0,
        MARGIN_MM,
        7.0,
        &format!(
            "Generated by Captur · {} · captured {}",
            format_unix_ms(model.generated_at_unix_ms),
            format_unix_ms(capture.captured_at_unix_ms)
        ),
        Rgb::new(0.48, 0.51, 0.57, None),
    );

    PdfPage::new(Mm(page_width), Mm(page_height), operations)
}

fn add_text(
    operations: &mut Vec<Op>,
    font: &PdfFontHandle,
    size: f32,
    x_mm: f32,
    y_mm: f32,
    text: &str,
    color: Rgb,
) {
    operations.extend([
        Op::StartTextSection,
        Op::SetFillColor {
            col: Color::Rgb(color),
        },
        Op::SetFont {
            font: font.clone(),
            size: Pt(size),
        },
        Op::SetTextCursor {
            pos: Point::new(Mm(x_mm), Mm(y_mm)),
        },
        Op::ShowText {
            items: vec![TextItem::Text(text.to_owned())],
        },
        Op::EndTextSection,
    ]);
}

fn split_tall_capture(frame: &BgraFrame) -> Vec<BgraFrame> {
    let page_ratio = 1.38_f32;
    if frame.height as f32 / frame.width as f32 <= 2.35 {
        return vec![frame.clone()];
    }

    let segment_height = (frame.width as f32 * page_ratio).round().max(1.0) as u32;
    let mut segments = Vec::new();
    let mut y = 0_u32;
    while y < frame.height {
        let height = segment_height.min(frame.height - y);
        let mut pixels = vec![0_u8; frame.width as usize * height as usize * 4];
        let stride = frame.width as usize * 4;
        for row in 0..height as usize {
            let source = (y as usize + row) * frame.stride;
            let destination = row * stride;
            pixels[destination..destination + stride]
                .copy_from_slice(&frame.pixels[source..source + stride]);
        }
        segments.push(BgraFrame {
            origin_x: frame.origin_x,
            origin_y: frame.origin_y + y as i32,
            width: frame.width,
            height,
            stride,
            pixels: pixels.into(),
        });
        y += height;
    }
    segments
}

fn tight_bgra(frame: &BgraFrame) -> Vec<u8> {
    let tight_stride = frame.width as usize * 4;
    if frame.stride == tight_stride {
        return frame.pixels.to_vec();
    }
    let mut pixels = vec![0_u8; tight_stride * frame.height as usize];
    for row in 0..frame.height as usize {
        pixels[row * tight_stride..(row + 1) * tight_stride]
            .copy_from_slice(&frame.pixels[row * frame.stride..row * frame.stride + tight_stride]);
    }
    pixels
}

fn truncate_line(value: &str, max_chars: usize) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= max_chars {
        normalized
    } else {
        format!(
            "{}…",
            normalized
                .chars()
                .take(max_chars.saturating_sub(1))
                .collect::<String>()
        )
    }
}

fn wrap_text(value: &str, max_chars: usize, max_lines: usize) -> Vec<String> {
    let mut lines = vec![String::new()];
    for word in value.split_whitespace() {
        let line_is_empty = lines.last().is_none_or(String::is_empty);
        let additional = word.chars().count() + usize::from(!line_is_empty);
        let line_len = lines.last().map_or(0, |line| line.chars().count());
        if line_len + additional > max_chars && lines.len() < max_lines {
            lines.push(word.to_owned());
        } else {
            let line = lines.last_mut().unwrap();
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
    }
    if let Some(last) = lines.last_mut()
        && last.chars().count() > max_chars
    {
        *last = truncate_line(last, max_chars);
    }
    lines
}

fn format_unix_ms(milliseconds: u64) -> String {
    let seconds = (milliseconds / 1000) as i64;
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3600;
    let minute = (seconds_of_day % 3600) / 60;
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02} UTC")
}

fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use std::{fs, sync::Arc};

    use super::*;

    #[test]
    fn unix_epoch_formats_correctly() {
        assert_eq!(format_unix_ms(0), "1970-01-01 00:00 UTC");
    }

    #[test]
    fn tall_capture_splits_without_pixel_loss() {
        let frame = BgraFrame {
            origin_x: 0,
            origin_y: 0,
            width: 10,
            height: 100,
            stride: 40,
            pixels: Arc::from(vec![1_u8; 4000]),
        };
        let segments = split_tall_capture(&frame);
        assert!(segments.len() > 1);
        assert_eq!(segments.iter().map(|s| s.height).sum::<u32>(), 100);
    }

    #[test]
    fn exports_realistic_multi_page_evidence_pdf() {
        let dimensions = [
            (1920, 1080),
            (2560, 1440),
            (3840, 2160),
            (1080, 1920),
            (3440, 1440),
            (800, 3600),
        ];
        let captures = dimensions
            .into_iter()
            .enumerate()
            .map(|(index, (width, height))| PdfExportCapture {
                number: index + 1,
                frame: solid_frame(width, height, [32 + index as u8 * 12, 78, 140, 255]),
                caption: if index % 2 == 0 {
                    format!("Evidence caption for capture {}", index + 1)
                } else {
                    String::new()
                },
                label: match index % 3 {
                    0 => EvidenceLabel::Before,
                    1 => EvidenceLabel::Action,
                    _ => EvidenceLabel::After,
                },
                captured_at_unix_ms: 1_789_627_200_000 + index as u64 * 1000,
            })
            .collect();
        let model = PdfExportModel {
            title: "Order total deletion evidence".into(),
            description: "The total remains unchanged until the page is refreshed.".into(),
            generated_at_unix_ms: 1_789_627_200_000,
            captures,
        };
        let scratch = std::env::var_os("JCODE_SCRATCH_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let path = scratch.join(format!(
            "captur-pdf-test-{}-{}.pdf",
            std::process::id(),
            model.generated_at_unix_ms
        ));

        PdfExporter.export(&model, &path).unwrap();
        let bytes = fs::read(&path).unwrap();
        assert!(bytes.starts_with(b"%PDF-"));
        assert!(bytes.len() > 20_000);
        let page_markers = bytes
            .windows(b"/Type/Page".len())
            .filter(|window| *window == b"/Type/Page")
            .count();
        assert!(page_markers >= model.captures.len());
        let _ = fs::remove_file(path);
    }

    fn solid_frame(width: u32, height: u32, bgra: [u8; 4]) -> BgraFrame {
        let stride = width as usize * 4;
        let mut pixels = vec![0_u8; stride * height as usize];
        for pixel in pixels.as_chunks_mut::<4>().0 {
            pixel.copy_from_slice(&bgra);
        }
        BgraFrame {
            origin_x: 0,
            origin_y: 0,
            width,
            height,
            stride,
            pixels: Arc::from(pixels),
        }
    }
}
