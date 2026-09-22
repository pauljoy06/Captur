use std::{mem::size_of, sync::Arc};

use windows::Win32::{
    Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleBitmap,
        CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, HBITMAP, HDC,
        HGDIOBJ, ReleaseDC, SRCCOPY, SelectObject,
    },
    UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN,
    },
};

use super::BgraFrame;

pub(super) fn capture_desktop() -> Result<BgraFrame, String> {
    // Safety: every acquired GDI handle is wrapped in a guard, the selected bitmap is restored
    // before it is read or deleted, and GetDIBits writes into a correctly sized BGRA buffer.
    unsafe { capture_desktop_inner() }
}

unsafe fn capture_desktop_inner() -> Result<BgraFrame, String> {
    let origin_x = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
    let origin_y = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
    let width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
    let height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
    if width <= 0 || height <= 0 {
        return Err(format!(
            "Windows reported invalid virtual desktop dimensions {width} x {height}"
        ));
    }

    let screen_dc = ScreenDc(unsafe { GetDC(None) });
    if screen_dc.0.0.is_null() {
        return Err(last_error("GetDC"));
    }

    let memory_dc = MemoryDc(unsafe { CreateCompatibleDC(Some(screen_dc.0)) });
    if memory_dc.0.0.is_null() {
        return Err(last_error("CreateCompatibleDC"));
    }

    let bitmap = Bitmap(unsafe { CreateCompatibleBitmap(screen_dc.0, width, height) });
    if bitmap.0.0.is_null() {
        return Err(last_error("CreateCompatibleBitmap"));
    }

    let previous = unsafe { SelectObject(memory_dc.0, HGDIOBJ(bitmap.0.0)) };
    if previous.0.is_null() {
        return Err(last_error("SelectObject"));
    }
    let selected = SelectedObject {
        dc: memory_dc.0,
        previous,
    };

    unsafe {
        BitBlt(
            memory_dc.0,
            0,
            0,
            width,
            height,
            Some(screen_dc.0),
            origin_x,
            origin_y,
            SRCCOPY | CAPTUREBLT,
        )
        .map_err(|error| format!("BitBlt failed: {error}"))?;
    }

    // GetDIBits requires that the bitmap is not selected into a device context.
    drop(selected);

    let width = width as u32;
    let height = height as u32;
    let stride = width as usize * 4;
    let byte_count = stride
        .checked_mul(height as usize)
        .ok_or_else(|| "virtual desktop is too large to capture".to_owned())?;
    let mut pixels = vec![0_u8; byte_count];
    let mut bitmap_info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            // A negative height requests top-down rows, matching BgraFrame's layout.
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            biSizeImage: byte_count as u32,
            ..Default::default()
        },
        ..Default::default()
    };
    let copied_rows = unsafe {
        GetDIBits(
            memory_dc.0,
            bitmap.0,
            0,
            height,
            Some(pixels.as_mut_ptr().cast()),
            &mut bitmap_info,
            DIB_RGB_COLORS,
        )
    };
    if copied_rows != height as i32 {
        return Err(if copied_rows == 0 {
            last_error("GetDIBits")
        } else {
            format!("GetDIBits copied {copied_rows} of {height} rows")
        });
    }

    // Screen-compatible bitmaps do not provide meaningful alpha. Captur's clipboard DIB does,
    // so make every captured pixel fully opaque instead of producing a transparent screenshot.
    for alpha in pixels.iter_mut().skip(3).step_by(4) {
        *alpha = 255;
    }

    Ok(BgraFrame {
        origin_x,
        origin_y,
        width,
        height,
        stride,
        pixels: Arc::from(pixels),
    })
}

fn last_error(operation: &str) -> String {
    format!(
        "{operation} failed: {}",
        windows::core::Error::from_thread()
    )
}

struct ScreenDc(HDC);

impl Drop for ScreenDc {
    fn drop(&mut self) {
        if !self.0.0.is_null() {
            unsafe {
                let _ = ReleaseDC(None, self.0);
            }
        }
    }
}

struct MemoryDc(HDC);

impl Drop for MemoryDc {
    fn drop(&mut self) {
        if !self.0.0.is_null() {
            unsafe {
                let _ = DeleteDC(self.0);
            }
        }
    }
}

struct Bitmap(HBITMAP);

impl Drop for Bitmap {
    fn drop(&mut self) {
        if !self.0.0.is_null() {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(self.0.0));
            }
        }
    }
}

struct SelectedObject {
    dc: HDC,
    previous: HGDIOBJ,
}

impl Drop for SelectedObject {
    fn drop(&mut self) {
        unsafe {
            let _ = SelectObject(self.dc, self.previous);
        }
    }
}
