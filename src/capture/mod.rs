#[cfg(target_os = "windows")]
pub mod dxgi;
#[cfg(target_os = "windows")]
mod gdi;
pub mod region;

use std::sync::Arc;

use region::PixelRect;

#[derive(Clone, Debug)]
pub struct BgraFrame {
    pub origin_x: i32,
    pub origin_y: i32,
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub pixels: Arc<[u8]>,
}

impl BgraFrame {
    pub fn bounds(&self) -> PixelRect {
        PixelRect {
            left: self.origin_x,
            top: self.origin_y,
            right: self.origin_x + self.width as i32,
            bottom: self.origin_y + self.height as i32,
        }
    }

    pub fn crop(&self, rect: PixelRect) -> Result<Self, String> {
        let rect = rect.clamp_to(self.bounds());
        if rect.is_empty() {
            return Err("selection is empty".into());
        }

        let width = rect.width();
        let height = rect.height();
        let stride = width as usize * 4;
        let mut pixels = vec![0_u8; stride * height as usize];
        let source_x = (rect.left - self.origin_x) as usize;
        let source_y = (rect.top - self.origin_y) as usize;

        for row in 0..height as usize {
            let src = (source_y + row) * self.stride + source_x * 4;
            let dst = row * stride;
            pixels[dst..dst + stride].copy_from_slice(&self.pixels[src..src + stride]);
        }

        Ok(Self {
            origin_x: rect.left,
            origin_y: rect.top,
            width,
            height,
            stride,
            pixels: pixels.into(),
        })
    }

    #[cfg(target_os = "windows")]
    pub fn to_egui_image(&self) -> egui::ColorImage {
        let mut rgba = Vec::with_capacity(self.width as usize * self.height as usize * 4);
        for row in 0..self.height as usize {
            let row_start = row * self.stride;
            let (pixels, remainder) =
                self.pixels[row_start..row_start + self.width as usize * 4].as_chunks::<4>();
            debug_assert!(remainder.is_empty());
            for [blue, green, red, _alpha] in pixels {
                rgba.extend_from_slice(&[*red, *green, *blue, 255]);
            }
        }
        egui::ColorImage::from_rgba_unmultiplied([self.width as usize, self.height as usize], &rgba)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crops_negative_origin_frame() {
        let pixels: Arc<[u8]> = (0_u8..32).collect::<Vec<_>>().into();
        let frame = BgraFrame {
            origin_x: -2,
            origin_y: -1,
            width: 4,
            height: 2,
            stride: 16,
            pixels,
        };
        let cropped = frame
            .crop(PixelRect {
                left: -1,
                top: -1,
                right: 1,
                bottom: 1,
            })
            .unwrap();
        assert_eq!((cropped.width, cropped.height, cropped.stride), (2, 2, 8));
        assert_eq!(
            &*cropped.pixels,
            &[4, 5, 6, 7, 8, 9, 10, 11, 20, 21, 22, 23, 24, 25, 26, 27]
        );
    }
}
