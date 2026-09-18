//! Annotation primitives and a small software rasterizer for captured BGRA frames.

use std::sync::Arc;

use crate::capture::BgraFrame;

const RED: [u8; 4] = [32, 32, 230, 255];
const BLUE: [u8; 4] = [220, 105, 35, 255];
const YELLOW: [u8; 4] = [0, 230, 255, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnnotationTool {
    Arrow,
    Rectangle,
    Highlight,
    Text,
    Redact,
    Marker,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AnnotationPoint {
    pub x: i32,
    pub y: i32,
}

impl AnnotationPoint {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum AnnotationItem {
    Arrow {
        start: AnnotationPoint,
        end: AnnotationPoint,
        thickness: u32,
    },
    Rectangle {
        start: AnnotationPoint,
        end: AnnotationPoint,
        thickness: u32,
    },
    Highlight {
        start: AnnotationPoint,
        end: AnnotationPoint,
    },
    Text {
        position: AnnotationPoint,
        text: String,
        size: u32,
    },
    Redact {
        start: AnnotationPoint,
        end: AnnotationPoint,
        block_size: u32,
    },
    Marker {
        center: AnnotationPoint,
        number: u32,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct AnnotationDocument {
    pub items: Vec<AnnotationItem>,
    next_marker_number: u32,
}

impl Default for AnnotationDocument {
    fn default() -> Self {
        Self::new()
    }
}

impl AnnotationDocument {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            next_marker_number: 1,
        }
    }

    /// Adds an item. Marker numbers supplied by callers are replaced with the
    /// next document number so numbering remains monotonic and unambiguous.
    pub fn add(&mut self, mut item: AnnotationItem) -> u32 {
        let marker_number = if let AnnotationItem::Marker { number, .. } = &mut item {
            let assigned = self.next_marker_number.max(1);
            *number = assigned;
            self.next_marker_number = assigned.saturating_add(1);
            assigned
        } else {
            0
        };
        self.items.push(item);
        marker_number
    }

    pub fn add_item(&mut self, item: AnnotationItem) -> u32 {
        self.add(item)
    }

    pub fn add_marker(&mut self, center: AnnotationPoint) -> u32 {
        self.add(AnnotationItem::Marker { center, number: 0 })
    }

    pub fn undo(&mut self) -> Option<AnnotationItem> {
        self.items.pop()
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.next_marker_number = 1;
    }

    /// Clones `frame`, applies all annotations in order, and returns the result.
    pub fn render(&self, frame: &BgraFrame) -> BgraFrame {
        let mut rendered = frame.clone();
        self.apply(&mut rendered);
        rendered
    }

    /// Applies all annotations to `frame`, using copy-on-write for its pixels.
    pub fn apply(&self, frame: &mut BgraFrame) {
        let width = frame.width as usize;
        let height = frame.height as usize;
        let stride = frame.stride;
        let pixels = Arc::make_mut(&mut frame.pixels);
        if pixels.len() < stride.saturating_mul(height) {
            return;
        }

        let mut canvas = Canvas {
            pixels,
            width,
            height,
            stride,
        };
        for item in &self.items {
            canvas.draw(item);
        }
        canvas.make_opaque();
    }
}

struct Canvas<'a> {
    pixels: &'a mut [u8],
    width: usize,
    height: usize,
    stride: usize,
}

impl Canvas<'_> {
    fn draw(&mut self, item: &AnnotationItem) {
        match item {
            AnnotationItem::Arrow {
                start,
                end,
                thickness,
            } => {
                let thickness = (*thickness).max(2) as i32;
                self.line(*start, *end, thickness, RED, 255);
                let dx = (end.x - start.x) as f32;
                let dy = (end.y - start.y) as f32;
                let length = (dx * dx + dy * dy).sqrt();
                if length > 0.0 {
                    let ux = dx / length;
                    let uy = dy / length;
                    let head = (thickness as f32 * 3.5).max(12.0).min(length * 0.65);
                    let wing = head * 0.55;
                    let base_x = end.x as f32 - ux * head;
                    let base_y = end.y as f32 - uy * head;
                    let left = AnnotationPoint::new(
                        (base_x - uy * wing).round() as i32,
                        (base_y + ux * wing).round() as i32,
                    );
                    let right = AnnotationPoint::new(
                        (base_x + uy * wing).round() as i32,
                        (base_y - ux * wing).round() as i32,
                    );
                    self.filled_triangle(*end, left, right, RED);
                }
            }
            AnnotationItem::Rectangle {
                start,
                end,
                thickness,
            } => {
                let (left, top, right, bottom) = ordered_rect(*start, *end);
                let t = (*thickness).max(1) as i32;
                self.fill_rect(left, top, right + 1, top + t, RED, 255);
                self.fill_rect(left, bottom - t + 1, right + 1, bottom + 1, RED, 255);
                self.fill_rect(left, top, left + t, bottom + 1, RED, 255);
                self.fill_rect(right - t + 1, top, right + 1, bottom + 1, RED, 255);
            }
            AnnotationItem::Highlight { start, end } => {
                let (left, top, right, bottom) = ordered_rect(*start, *end);
                self.fill_rect(left, top, right + 1, bottom + 1, YELLOW, 96);
            }
            AnnotationItem::Text {
                position,
                text,
                size,
            } => {
                self.text(*position, text, (*size).max(8));
            }
            AnnotationItem::Redact {
                start,
                end,
                block_size,
            } => {
                self.pixelate(*start, *end, (*block_size).max(3) as usize);
            }
            AnnotationItem::Marker { center, number } => self.marker(*center, *number),
        }
    }

    fn pixel(&mut self, x: i32, y: i32, color: [u8; 4], alpha: u8) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let offset = y as usize * self.stride + x as usize * 4;
        if alpha == 255 {
            self.pixels[offset..offset + 4].copy_from_slice(&color);
        } else {
            let a = alpha as u16;
            let inverse = 255 - a;
            for (destination, source) in self.pixels[offset..offset + 3]
                .iter_mut()
                .zip(color[..3].iter())
            {
                *destination =
                    ((*source as u16 * a + *destination as u16 * inverse + 127) / 255) as u8;
            }
            self.pixels[offset + 3] = 255;
        }
    }

    fn fill_rect(
        &mut self,
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
        color: [u8; 4],
        alpha: u8,
    ) {
        for y in top.max(0)..bottom.min(self.height as i32) {
            for x in left.max(0)..right.min(self.width as i32) {
                self.pixel(x, y, color, alpha);
            }
        }
    }

    fn disc(&mut self, center: AnnotationPoint, radius: i32, color: [u8; 4], alpha: u8) {
        for y in -radius..=radius {
            for x in -radius..=radius {
                if x * x + y * y <= radius * radius {
                    self.pixel(center.x + x, center.y + y, color, alpha);
                }
            }
        }
    }

    fn line(
        &mut self,
        start: AnnotationPoint,
        end: AnnotationPoint,
        thickness: i32,
        color: [u8; 4],
        alpha: u8,
    ) {
        let dx = end.x - start.x;
        let dy = end.y - start.y;
        let steps = dx.abs().max(dy.abs()).max(1);
        let radius = (thickness - 1) / 2;
        for step in 0..=steps {
            let x = start.x + dx * step / steps;
            let y = start.y + dy * step / steps;
            self.disc(AnnotationPoint::new(x, y), radius, color, alpha);
        }
    }

    fn filled_triangle(
        &mut self,
        a: AnnotationPoint,
        b: AnnotationPoint,
        c: AnnotationPoint,
        color: [u8; 4],
    ) {
        let min_x = a.x.min(b.x).min(c.x);
        let max_x = a.x.max(b.x).max(c.x);
        let min_y = a.y.min(b.y).min(c.y);
        let max_y = a.y.max(b.y).max(c.y);
        let edge = |p: AnnotationPoint, q: AnnotationPoint, x: i32, y: i32| {
            (x - p.x) as i64 * (q.y - p.y) as i64 - (y - p.y) as i64 * (q.x - p.x) as i64
        };
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let e1 = edge(a, b, x, y);
                let e2 = edge(b, c, x, y);
                let e3 = edge(c, a, x, y);
                if (e1 >= 0 && e2 >= 0 && e3 >= 0) || (e1 <= 0 && e2 <= 0 && e3 <= 0) {
                    self.pixel(x, y, color, 255);
                }
            }
        }
    }

    fn pixelate(&mut self, start: AnnotationPoint, end: AnnotationPoint, block: usize) {
        let (left, top, right, bottom) = ordered_rect(start, end);
        let left = left.max(0) as usize;
        let top = top.max(0) as usize;
        let right = (right + 1).min(self.width as i32).max(0) as usize;
        let bottom = (bottom + 1).min(self.height as i32).max(0) as usize;
        for by in (top..bottom).step_by(block) {
            for bx in (left..right).step_by(block) {
                let block_right = (bx + block).min(right);
                let block_bottom = (by + block).min(bottom);
                let mut sums = [0_u64; 3];
                let count = ((block_right - bx) * (block_bottom - by)) as u64;
                if count == 0 {
                    continue;
                }
                for y in by..block_bottom {
                    for x in bx..block_right {
                        let offset = y * self.stride + x * 4;
                        for (sum, value) in
                            sums.iter_mut().zip(self.pixels[offset..offset + 3].iter())
                        {
                            *sum += *value as u64;
                        }
                    }
                }
                let color = [
                    (sums[0] / count) as u8,
                    (sums[1] / count) as u8,
                    (sums[2] / count) as u8,
                    255,
                ];
                self.fill_rect(
                    bx as i32,
                    by as i32,
                    block_right as i32,
                    block_bottom as i32,
                    color,
                    255,
                );
            }
        }
    }

    fn marker(&mut self, center: AnnotationPoint, number: u32) {
        let radius = 13;
        self.disc(center, radius, BLUE, 255);
        for angle in 0..360 {
            let radians = (angle as f32).to_radians();
            self.disc(
                AnnotationPoint::new(
                    center.x + (radians.cos() * radius as f32).round() as i32,
                    center.y + (radians.sin() * radius as f32).round() as i32,
                ),
                1,
                WHITE,
                255,
            );
        }
        let label = number.to_string();
        let width = label.len() as i32 * 6;
        self.text(
            AnnotationPoint::new(center.x - width / 2, center.y - 6),
            &label,
            13,
        );
    }

    #[cfg(target_os = "windows")]
    fn text(&mut self, position: AnnotationPoint, text: &str, size: u32) {
        windows_text::draw(self, position, text, size);
    }

    #[cfg(not(target_os = "windows"))]
    fn text(&mut self, position: AnnotationPoint, text: &str, size: u32) {
        // Tiny deterministic fallback used by tests and non-Windows builds.
        let scale = (size / 8).max(1) as i32;
        for (index, byte) in text.bytes().enumerate() {
            for row in 0..7_i32 {
                for column in 0..5_i32 {
                    let bit = ((byte.rotate_left(row as u32) >> column) & 1) != 0;
                    if bit {
                        self.fill_rect(
                            position.x + index as i32 * 6 * scale + column * scale,
                            position.y + row * scale,
                            position.x + index as i32 * 6 * scale + (column + 1) * scale,
                            position.y + (row + 1) * scale,
                            WHITE,
                            255,
                        );
                    }
                }
            }
        }
    }

    fn make_opaque(&mut self) {
        for y in 0..self.height {
            for x in 0..self.width {
                self.pixels[y * self.stride + x * 4 + 3] = 255;
            }
        }
    }
}

fn ordered_rect(a: AnnotationPoint, b: AnnotationPoint) -> (i32, i32, i32, i32) {
    (a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y))
}

#[cfg(target_os = "windows")]
mod windows_text {
    use super::{AnnotationPoint, Canvas};
    use std::ptr;
    use windows::{
        Win32::{
            Foundation::COLORREF,
            Graphics::Gdi::{
                BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS,
                CreateCompatibleDC, CreateDIBSection, CreateFontW, DEFAULT_CHARSET, DIB_RGB_COLORS,
                DeleteDC, DeleteObject, HGDIOBJ, OUT_DEFAULT_PRECIS, SelectObject, SetBkMode,
                SetTextColor, TRANSPARENT, TextOutW,
            },
        },
        core::w,
    };

    pub(super) fn draw(canvas: &mut Canvas<'_>, position: AnnotationPoint, text: &str, size: u32) {
        // Safety: every GDI handle is checked before use, selected objects are restored before
        // deletion, and all raw copies are bounded by the top-down 32bpp DIB allocation.
        unsafe { draw_unchecked(canvas, position, text, size) }
    }

    unsafe fn draw_unchecked(
        canvas: &mut Canvas<'_>,
        position: AnnotationPoint,
        text: &str,
        size: u32,
    ) {
        if canvas.width == 0 || canvas.height == 0 || text.is_empty() {
            return;
        }

        let tight_stride = canvas.width * 4;
        let byte_len = tight_stride * canvas.height;
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: canvas.width as i32,
                biHeight: -(canvas.height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                biSizeImage: byte_len as u32,
                ..Default::default()
            },
            ..Default::default()
        };

        let dc = unsafe { CreateCompatibleDC(None) };
        if dc.is_invalid() {
            return;
        }
        let mut bits = ptr::null_mut();
        let Ok(bitmap) =
            (unsafe { CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0) })
        else {
            let _ = unsafe { DeleteDC(dc) };
            return;
        };
        if bits.is_null() {
            let _ = unsafe { DeleteObject(HGDIOBJ(bitmap.0)) };
            let _ = unsafe { DeleteDC(dc) };
            return;
        }

        for row in 0..canvas.height {
            unsafe {
                ptr::copy_nonoverlapping(
                    canvas.pixels.as_ptr().add(row * canvas.stride),
                    (bits as *mut u8).add(row * tight_stride),
                    tight_stride,
                );
            }
        }

        let old_bitmap = unsafe { SelectObject(dc, HGDIOBJ(bitmap.0)) };
        let font = unsafe {
            CreateFontW(
                -(size as i32),
                0,
                0,
                0,
                700,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                0,
                w!("Segoe UI"),
            )
        };
        let old_font = if font.is_invalid() {
            HGDIOBJ::default()
        } else {
            unsafe { SelectObject(dc, HGDIOBJ(font.0)) }
        };
        let wide: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            let _ = SetBkMode(dc, TRANSPARENT);
            SetTextColor(dc, COLORREF(0x0000_0000));
            let _ = TextOutW(dc, position.x + 1, position.y + 1, &wide);
            SetTextColor(dc, COLORREF(0x00ff_ffff));
            let _ = TextOutW(dc, position.x, position.y, &wide);
        }

        for row in 0..canvas.height {
            unsafe {
                ptr::copy_nonoverlapping(
                    (bits as *const u8).add(row * tight_stride),
                    canvas.pixels.as_mut_ptr().add(row * canvas.stride),
                    tight_stride,
                );
            }
        }
        unsafe {
            if !old_font.is_invalid() {
                SelectObject(dc, old_font);
            }
            if !font.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(font.0));
            }
            if !old_bitmap.is_invalid() {
                SelectObject(dc, old_bitmap);
            }
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
            let _ = DeleteDC(dc);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(width: u32, height: u32) -> BgraFrame {
        let stride = width as usize * 4;
        BgraFrame {
            origin_x: 0,
            origin_y: 0,
            width,
            height,
            stride,
            pixels: vec![10_u8; stride * height as usize].into(),
        }
    }

    fn pixel(frame: &BgraFrame, x: usize, y: usize) -> [u8; 4] {
        let offset = y * frame.stride + x * 4;
        frame.pixels[offset..offset + 4].try_into().unwrap()
    }

    #[test]
    fn undo_preserves_sequential_marker_numbers() {
        let mut document = AnnotationDocument::new();
        assert_eq!(document.add_marker(AnnotationPoint::new(4, 4)), 1);
        assert_eq!(document.add_marker(AnnotationPoint::new(8, 8)), 2);
        assert!(matches!(
            document.undo(),
            Some(AnnotationItem::Marker { number: 2, .. })
        ));
        assert_eq!(document.add_marker(AnnotationPoint::new(12, 12)), 3);
        document.clear();
        assert_eq!(document.add_marker(AnnotationPoint::new(4, 4)), 1);
    }

    #[test]
    fn primitives_mutate_representative_pixels_and_keep_source_unchanged() {
        let source = frame(40, 30);
        let mut document = AnnotationDocument::new();
        document.add(AnnotationItem::Rectangle {
            start: AnnotationPoint::new(2, 2),
            end: AnnotationPoint::new(20, 15),
            thickness: 3,
        });
        document.add(AnnotationItem::Highlight {
            start: AnnotationPoint::new(24, 3),
            end: AnnotationPoint::new(35, 12),
        });
        document.add(AnnotationItem::Arrow {
            start: AnnotationPoint::new(3, 24),
            end: AnnotationPoint::new(20, 24),
            thickness: 5,
        });
        let rendered = document.render(&source);

        assert_eq!(pixel(&source, 2, 2), [10, 10, 10, 10]);
        assert_eq!(pixel(&rendered, 2, 2), RED);
        assert_ne!(pixel(&rendered, 28, 7), pixel(&source, 28, 7));
        assert_eq!(pixel(&rendered, 10, 24), RED);
        assert!(
            rendered
                .pixels
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| pixel[3] == 255)
        );
    }

    #[test]
    fn redaction_replaces_each_block_with_its_average() {
        let mut source = frame(4, 4);
        let pixels = Arc::make_mut(&mut source.pixels);
        pixels[0..4].copy_from_slice(&[0, 0, 0, 7]);
        pixels[4..8].copy_from_slice(&[100, 100, 100, 7]);
        pixels[16..20].copy_from_slice(&[200, 200, 200, 7]);
        pixels[20..24].copy_from_slice(&[100, 100, 100, 7]);
        let mut document = AnnotationDocument::new();
        document.add(AnnotationItem::Redact {
            start: AnnotationPoint::new(0, 0),
            end: AnnotationPoint::new(1, 1),
            block_size: 2,
        });
        let rendered = document.render(&source);
        assert_eq!(pixel(&rendered, 0, 0), [100, 100, 100, 255]);
        assert_eq!(pixel(&rendered, 1, 1), [100, 100, 100, 255]);
    }
}
