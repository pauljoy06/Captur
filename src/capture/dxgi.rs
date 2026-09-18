use std::{cell::RefCell, slice, sync::Arc};

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
                CreateDXGIFactory1, DXGI_ERROR_NOT_FOUND, DXGI_ERROR_WAIT_TIMEOUT,
                DXGI_OUTDUPL_FRAME_INFO, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput1,
                IDXGIOutputDuplication, IDXGIResource,
            },
        },
    },
    core::Interface,
};

use super::{BgraFrame, region::PixelRect};

thread_local! {
    static CAPTURE_ENGINE: RefCell<Option<CaptureEngine>> = const { RefCell::new(None) };
}

pub fn initialize() -> Result<(), String> {
    CAPTURE_ENGINE.with(|engine| {
        let mut engine = engine.borrow_mut();
        if engine.is_none() {
            *engine = Some(unsafe { CaptureEngine::new() }.map_err(capture_error)?);
        }
        Ok(())
    })
}

pub fn capture_desktop() -> Result<BgraFrame, String> {
    // Safety: all COM interfaces are owned RAII values. Mapped pointers are copied before Unmap,
    // and every acquired duplication frame is released before the interface is dropped.
    CAPTURE_ENGINE.with(|engine| {
        let mut engine = engine.borrow_mut();
        if engine.is_none() {
            *engine = Some(unsafe { CaptureEngine::new() }.map_err(capture_error)?);
        }

        let first = unsafe {
            engine
                .as_mut()
                .expect("capture engine initialized")
                .capture()
        };
        match first {
            Ok(frame) => Ok(frame),
            Err(_) => {
                *engine = None;
                *engine = Some(unsafe { CaptureEngine::new() }.map_err(capture_error)?);
                unsafe { engine.as_mut().expect("capture engine rebuilt").capture() }
                    .map_err(capture_error)
            }
        }
    })
}

fn capture_error(error: windows::core::Error) -> String {
    format!("desktop duplication failed: {error}")
}

struct CaptureEngine {
    outputs: Vec<OutputDuplicator>,
    bounds: PixelRect,
}

impl CaptureEngine {
    unsafe fn new() -> windows::core::Result<Self> {
        let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1()? };
        let mut outputs = Vec::new();
        let mut adapter_index = 0;

        loop {
            let adapter = match unsafe { factory.EnumAdapters1(adapter_index) } {
                Ok(adapter) => adapter,
                Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(error) => return Err(error),
            };
            outputs.extend(unsafe { create_adapter_outputs(&adapter)? });
            adapter_index += 1;
        }

        if outputs.is_empty() {
            return Err(windows::core::Error::new(
                windows::core::HRESULT(0x8000_4005_u32 as i32),
                "no attached desktop outputs were available",
            ));
        }

        let bounds = outputs.iter().fold(
            PixelRect {
                left: i32::MAX,
                top: i32::MAX,
                right: i32::MIN,
                bottom: i32::MIN,
            },
            |mut bounds, output| {
                bounds.left = bounds.left.min(output.rect.left);
                bounds.top = bounds.top.min(output.rect.top);
                bounds.right = bounds.right.max(output.rect.right);
                bounds.bottom = bounds.bottom.max(output.rect.bottom);
                bounds
            },
        );
        Ok(Self { outputs, bounds })
    }

    unsafe fn capture(&mut self) -> windows::core::Result<BgraFrame> {
        let mut captures = Vec::with_capacity(self.outputs.len());
        for output in &mut self.outputs {
            captures.push(unsafe { output.capture()? });
        }

        let width = self.bounds.width();
        let height = self.bounds.height();
        let stride = width as usize * 4;
        let mut pixels = vec![0_u8; stride * height as usize];

        for capture in captures {
            let x = (capture.rect.left - self.bounds.left) as usize;
            let y = (capture.rect.top - self.bounds.top) as usize;
            for row in 0..capture.height as usize {
                let src = row * capture.stride;
                let dst = (y + row) * stride + x * 4;
                let len = capture.width as usize * 4;
                pixels[dst..dst + len].copy_from_slice(&capture.pixels[src..src + len]);
            }
        }

        Ok(BgraFrame {
            origin_x: self.bounds.left,
            origin_y: self.bounds.top,
            width,
            height,
            stride,
            pixels: Arc::from(pixels),
        })
    }
}

#[derive(Clone)]
struct OutputCapture {
    rect: PixelRect,
    width: u32,
    height: u32,
    stride: usize,
    pixels: Vec<u8>,
}

struct OutputDuplicator {
    rect: PixelRect,
    rotation: windows::Win32::Graphics::Dxgi::Common::DXGI_MODE_ROTATION,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    duplication: IDXGIOutputDuplication,
    latest: Option<OutputCapture>,
}

impl OutputDuplicator {
    unsafe fn capture(&mut self) -> windows::core::Result<OutputCapture> {
        let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut desktop_resource: Option<IDXGIResource> = None;
        let timeout_ms = if self.latest.is_some() { 32 } else { 500 };
        if let Err(error) = unsafe {
            self.duplication
                .AcquireNextFrame(timeout_ms, &mut frame_info, &mut desktop_resource)
        } {
            if error.code() == DXGI_ERROR_WAIT_TIMEOUT
                && let Some(latest) = self.latest.clone()
            {
                return Ok(latest);
            }
            return Err(error);
        }

        let capture_result = (|| unsafe {
            let resource = desktop_resource.ok_or_else(|| {
                windows::core::Error::new(
                    windows::core::HRESULT(0x8000_4005_u32 as i32),
                    "desktop duplication returned no resource",
                )
            })?;
            let texture: ID3D11Texture2D = resource.cast()?;
            read_texture(
                &self.device,
                &self.context,
                &texture,
                self.rotation,
                self.rect,
            )
        })();
        let release_result = unsafe { self.duplication.ReleaseFrame() };
        let capture = capture_result?;
        release_result?;
        self.latest = Some(capture.clone());
        Ok(capture)
    }
}

unsafe fn create_adapter_outputs(
    adapter: &IDXGIAdapter1,
) -> windows::core::Result<Vec<OutputDuplicator>> {
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
        captures.push(OutputDuplicator {
            rect: PixelRect {
                left: desc.DesktopCoordinates.left,
                top: desc.DesktopCoordinates.top,
                right: desc.DesktopCoordinates.right,
                bottom: desc.DesktopCoordinates.bottom,
            },
            rotation: desc.Rotation,
            device: device.clone(),
            context: context.clone(),
            duplication: unsafe { output1.DuplicateOutput(&device)? },
            latest: None,
        });
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
