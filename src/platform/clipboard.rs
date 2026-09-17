use std::{mem::size_of, ptr::copy_nonoverlapping, thread, time::Duration};

use windows::Win32::{
    Foundation::{GlobalFree, HANDLE},
    Graphics::Gdi::{BI_BITFIELDS, BITMAPV5HEADER},
    System::{
        DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData},
        Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock},
        Ole::CF_DIBV5,
    },
    UI::ColorSystem::LCS_sRGB,
};

use crate::capture::BgraFrame;

pub fn copy_bgra_to_clipboard(frame: &BgraFrame) -> Result<(), String> {
    let image_size = frame.width as usize * frame.height as usize * 4;
    let allocation_size = size_of::<BITMAPV5HEADER>() + image_size;

    // Safety: the movable allocation remains locked only while initialized. Ownership transfers
    // to the clipboard only after SetClipboardData succeeds; otherwise it is freed locally.
    unsafe {
        let allocation = GlobalAlloc(GMEM_MOVEABLE, allocation_size)
            .map_err(|error| format!("clipboard allocation failed: {error}"))?;
        let memory = GlobalLock(allocation);
        if memory.is_null() {
            let _ = GlobalFree(Some(allocation));
            return Err("clipboard allocation could not be locked".into());
        }

        let header = BITMAPV5HEADER {
            bV5Size: size_of::<BITMAPV5HEADER>() as u32,
            bV5Width: frame.width as i32,
            bV5Height: -(frame.height as i32),
            bV5Planes: 1,
            bV5BitCount: 32,
            bV5Compression: BI_BITFIELDS,
            bV5SizeImage: image_size as u32,
            bV5RedMask: 0x00ff_0000,
            bV5GreenMask: 0x0000_ff00,
            bV5BlueMask: 0x0000_00ff,
            bV5AlphaMask: 0xff00_0000,
            bV5CSType: LCS_sRGB.0 as u32,
            ..Default::default()
        };
        memory.cast::<BITMAPV5HEADER>().write(header);

        let destination = memory.cast::<u8>().add(size_of::<BITMAPV5HEADER>());
        let row_bytes = frame.width as usize * 4;
        for row in 0..frame.height as usize {
            copy_nonoverlapping(
                frame.pixels.as_ptr().add(row * frame.stride),
                destination.add(row * row_bytes),
                row_bytes,
            );
        }
        let _ = GlobalUnlock(allocation);

        let mut opened = false;
        for _ in 0..8 {
            if OpenClipboard(None).is_ok() {
                opened = true;
                break;
            }
            thread::sleep(Duration::from_millis(2));
        }
        if !opened {
            let _ = GlobalFree(Some(allocation));
            return Err("clipboard is busy".into());
        }

        let result = (|| -> windows::core::Result<()> {
            EmptyClipboard()?;
            SetClipboardData(CF_DIBV5.0 as u32, Some(HANDLE(allocation.0)))?;
            Ok(())
        })();
        let close_result = CloseClipboard();

        if let Err(error) = result {
            let _ = GlobalFree(Some(allocation));
            return Err(format!("clipboard copy failed: {error}"));
        }
        close_result.map_err(|error| format!("clipboard close failed: {error}"))?;
    }

    Ok(())
}
