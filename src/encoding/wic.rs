use std::path::Path;

use windows::{
    Win32::{
        Foundation::GENERIC_WRITE,
        Graphics::Imaging::{
            CLSID_WICImagingFactory, GUID_ContainerFormatPng, GUID_WICPixelFormat32bppBGRA,
            IWICImagingFactory, WICBitmapEncoderNoCache,
        },
        System::Com::{
            CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
            CoUninitialize,
        },
    },
    core::HSTRING,
};

use crate::capture::BgraFrame;

pub fn encode_png(frame: &BgraFrame, path: &Path) -> Result<(), String> {
    // Safety: this function is intended for a dedicated worker thread. COM is initialized and
    // uninitialized on that same thread, and every WIC interface is managed by COM RAII wrappers.
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED)
            .ok()
            .map_err(|error| format!("WIC COM initialization failed: {error}"))?;
        let result = encode_png_inner(frame, path);
        CoUninitialize();
        result.map_err(|error| format!("WIC PNG encoding failed: {error}"))
    }
}

unsafe fn encode_png_inner(frame: &BgraFrame, path: &Path) -> windows::core::Result<()> {
    let factory: IWICImagingFactory =
        unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)? };
    let stream = unsafe { factory.CreateStream()? };
    let filename = HSTRING::from(path.as_os_str().to_string_lossy().as_ref());
    unsafe { stream.InitializeFromFilename(&filename, GENERIC_WRITE.0)? };

    let encoder = unsafe { factory.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())? };
    unsafe { encoder.Initialize(&stream, WICBitmapEncoderNoCache)? };

    let mut frame_encoder = None;
    let mut options = None;
    unsafe { encoder.CreateNewFrame(&mut frame_encoder, &mut options)? };
    let frame_encoder = frame_encoder.expect("WIC did not return a frame encoder");
    unsafe {
        frame_encoder.Initialize(options.as_ref())?;
        frame_encoder.SetSize(frame.width, frame.height)?;
    }
    let mut pixel_format = GUID_WICPixelFormat32bppBGRA;
    unsafe { frame_encoder.SetPixelFormat(&mut pixel_format)? };
    if pixel_format != GUID_WICPixelFormat32bppBGRA {
        return Err(windows::core::Error::new(
            windows::core::HRESULT(0x8000_4005_u32 as i32),
            "WIC PNG encoder rejected 32bpp BGRA",
        ));
    }

    unsafe {
        frame_encoder.WritePixels(frame.height, frame.stride as u32, &frame.pixels)?;
        frame_encoder.Commit()?;
        encoder.Commit()?;
    }
    Ok(())
}
