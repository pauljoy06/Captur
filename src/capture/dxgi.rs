use std::{slice, sync::Arc};

use windows::{
    Win32::{
        Foundation::HMODULE,
        Graphics::{
            Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0},
            Direct3D11::{
                D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
                D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
                D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext,
                ID3D11Texture2D,
            },
            Dxgi::{
                Common::{
                    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_MODE_ROTATION_IDENTITY,
                    DXGI_MODE_ROTATION_ROTATE90, DXGI_MODE_ROTATION_ROTATE180,
                    DXGI_MODE_ROTATION_ROTATE270,
                },
                CreateDXGIFactory1, DXGI_ERROR_NOT_FOUND, DXGI_OUTDUPL_FRAME_INFO, IDXGIAdapter1,
                IDXGIFactory1, IDXGIOutput1, IDXGIResource,
            },
        },
    },
    core::Interface,
};

use super::{BgraFrame, region::PixelRect};

pub fn capture_desktop() -> Result<BgraFrame, String> {
    // Safety: all COM interfaces are owned RAII values. Mapped pointers are copied before Unmap,
    // and every acquired duplication frame is released before the interface is dropped.
    unsafe {
        capture_desktop_inner().map_err(|error| format!("desktop duplication failed: {error}"))
    }
}

unsafe fn capture_desktop_inner() -> windows::core::Result<BgraFrame> {
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1()? };
    let mut captures = Vec::new();
    let mut adapter_index = 0;

    loop {
        let adapter = match unsafe { factory.EnumAdapters1(adapter_index) } {
            Ok(adapter) => adapter,
            Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(error) => return Err(error),
        };
        captures.extend(unsafe { capture_adapter_outputs(&adapter)? });
        adapter_index += 1;
    }

    if captures.is_empty() {
        return Err(windows::core::Error::new(
            windows::core::HRESULT(0x8000_4005_u32 as i32),
            "no attached desktop outputs were available",
        ));
    }

    let bounds = captures.iter().fold(
        PixelRect {
            left: i32::MAX,
            top: i32::MAX,
            right: i32::MIN,
            bottom: i32::MIN,
        },
        |mut bounds, capture| {
            bounds.left = bounds.left.min(capture.rect.left);
            bounds.top = bounds.top.min(capture.rect.top);
            bounds.right = bounds.right.max(capture.rect.right);
            bounds.bottom = bounds.bottom.max(capture.rect.bottom);
            bounds
        },
    );

    let width = bounds.width();
    let height = bounds.height();
    let stride = width as usize * 4;
    let mut pixels = vec![0_u8; stride * height as usize];

    for capture in captures {
        let x = (capture.rect.left - bounds.left) as usize;
        let y = (capture.rect.top - bounds.top) as usize;
        for row in 0..capture.height as usize {
            let src = row * capture.stride;
            let dst = (y + row) * stride + x * 4;
            let len = capture.width as usize * 4;
            pixels[dst..dst + len].copy_from_slice(&capture.pixels[src..src + len]);
        }
    }

    Ok(BgraFrame {
        origin_x: bounds.left,
        origin_y: bounds.top,
        width,
        height,
        stride,
        pixels: Arc::from(pixels),
    })
}

struct OutputCapture {
    rect: PixelRect,
    width: u32,
    height: u32,
    stride: usize,
    pixels: Vec<u8>,
}

unsafe fn capture_adapter_outputs(
    adapter: &IDXGIAdapter1,
) -> windows::core::Result<Vec<OutputCapture>> {
    let (device, context) = unsafe { create_device(adapter)? };
    let mut captures = Vec::new();
    let mut output_index = 0;

    loop {
        let output = match unsafe { adapter.EnumOutputs(output_index) } {
            Ok(output) => output,
            Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(error) => return Err(error),
        };
        output_index += 1;

        let desc = unsafe { output.GetDesc()? };
        if !desc.AttachedToDesktop.as_bool() {
            continue;
        }

        let output1: IDXGIOutput1 = output.cast()?;
        let duplication = unsafe { output1.DuplicateOutput(&device)? };
        let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut desktop_resource: Option<IDXGIResource> = None;
        unsafe { duplication.AcquireNextFrame(500, &mut frame_info, &mut desktop_resource)? };

        let capture_result = (|| unsafe {
            let resource = desktop_resource.ok_or_else(|| {
                windows::core::Error::new(
                    windows::core::HRESULT(0x8000_4005_u32 as i32),
                    "desktop duplication returned no resource",
                )
            })?;
            let texture: ID3D11Texture2D = resource.cast()?;
            read_texture(
                &device,
                &context,
                &texture,
                desc.Rotation,
                PixelRect {
                    left: desc.DesktopCoordinates.left,
                    top: desc.DesktopCoordinates.top,
                    right: desc.DesktopCoordinates.right,
                    bottom: desc.DesktopCoordinates.bottom,
                },
            )
        })();

        let release_result = unsafe { duplication.ReleaseFrame() };
        captures.push(capture_result?);
        release_result?;
    }

    Ok(captures)
}

unsafe fn create_device(
    adapter: &IDXGIAdapter1,
) -> windows::core::Result<(ID3D11Device, ID3D11DeviceContext)> {
    let mut device = None;
    let mut context = None;
    unsafe {
        D3D11CreateDevice(
            adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&[D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )?;
    }
    Ok((device.unwrap(), context.unwrap()))
}

unsafe fn read_texture(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    source: &ID3D11Texture2D,
    rotation: windows::Win32::Graphics::Dxgi::Common::DXGI_MODE_ROTATION,
    rect: PixelRect,
) -> windows::core::Result<OutputCapture> {
    let mut source_desc = D3D11_TEXTURE2D_DESC::default();
    unsafe { source.GetDesc(&mut source_desc) };
    if source_desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
        return Err(windows::core::Error::new(
            windows::core::HRESULT(0x8000_4005_u32 as i32),
            "unexpected desktop duplication pixel format",
        ));
    }

    let staging_desc = D3D11_TEXTURE2D_DESC {
        Width: source_desc.Width,
        Height: source_desc.Height,
        MipLevels: 1,
        ArraySize: 1,
        Format: source_desc.Format,
        SampleDesc: source_desc.SampleDesc,
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut staging = None;
    unsafe { device.CreateTexture2D(&staging_desc, None, Some(&mut staging))? };
    let staging = staging.unwrap();
    unsafe { context.CopyResource(&staging, source) };

    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    unsafe { context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))? };
    let source_bytes = unsafe {
        slice::from_raw_parts(
            mapped.pData.cast::<u8>(),
            mapped.RowPitch as usize * source_desc.Height as usize,
        )
    };

    let destination_width = rect.width();
    let destination_height = rect.height();
    let stride = destination_width as usize * 4;
    let mut pixels = vec![0_u8; stride * destination_height as usize];
    copy_or_rotate(
        source_bytes,
        mapped.RowPitch as usize,
        source_desc.Width,
        source_desc.Height,
        &mut pixels,
        stride,
        destination_width,
        destination_height,
        rotation,
    );
    unsafe { context.Unmap(&staging, 0) };

    Ok(OutputCapture {
        rect,
        width: destination_width,
        height: destination_height,
        stride,
        pixels,
    })
}

#[allow(clippy::too_many_arguments)]
fn copy_or_rotate(
    source: &[u8],
    source_stride: usize,
    source_width: u32,
    source_height: u32,
    destination: &mut [u8],
    destination_stride: usize,
    destination_width: u32,
    destination_height: u32,
    rotation: windows::Win32::Graphics::Dxgi::Common::DXGI_MODE_ROTATION,
) {
    for y in 0..destination_height {
        for x in 0..destination_width {
            let (sx, sy) = if rotation == DXGI_MODE_ROTATION_ROTATE90 {
                (y, source_height.saturating_sub(1).saturating_sub(x))
            } else if rotation == DXGI_MODE_ROTATION_ROTATE180 {
                (
                    source_width.saturating_sub(1).saturating_sub(x),
                    source_height.saturating_sub(1).saturating_sub(y),
                )
            } else if rotation == DXGI_MODE_ROTATION_ROTATE270 {
                (source_width.saturating_sub(1).saturating_sub(y), x)
            } else {
                debug_assert!(rotation == DXGI_MODE_ROTATION_IDENTITY);
                (x, y)
            };

            if sx < source_width && sy < source_height {
                let src = sy as usize * source_stride + sx as usize * 4;
                let dst = y as usize * destination_stride + x as usize * 4;
                destination[dst..dst + 4].copy_from_slice(&source[src..src + 4]);
            }
        }
    }
}
