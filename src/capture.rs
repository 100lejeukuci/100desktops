//! Screen capture via DXGI Desktop Duplication. Frames stay on the GPU as D3D11 textures.

use anyhow::{Context, Result, bail};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Dxgi::*;
use windows::core::Interface;

pub struct Capturer {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    duplication: IDXGIOutputDuplication,
    pub width: u32,
    pub height: u32,
}

pub enum Frame {
    /// New picture copied into the texture returned by `Capturer::frame_texture`.
    New,
    /// Nothing changed on screen within the timeout.
    Unchanged,
}

impl Capturer {
    pub fn new() -> Result<Self> {
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
            let adapter = factory.EnumAdapters1(0).context("no GPU adapter")?;
            let output = adapter.EnumOutputs(0).context("no monitor on adapter 0")?;
            let rect = output.GetDesc()?.DesktopCoordinates;
            let width = (rect.right - rect.left) as u32;
            let height = (rect.bottom - rect.top) as u32;
            let output1: IDXGIOutput1 = output.cast()?;

            let mut device = None;
            let mut context = None;
            D3D11CreateDevice(
                &adapter.cast::<IDXGIAdapter>()?,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
            let device = device.context("D3D11 device missing")?;
            let context = context.context("D3D11 context missing")?;
            let duplication = output1.DuplicateOutput(&device)?;

            Ok(Self { device, context, duplication, width, height })
        }
    }

    /// Texture that matches the desktop size, usable as copy destination.
    pub fn frame_texture(&self) -> Result<ID3D11Texture2D> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: self.width,
            Height: self.height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
            ..Default::default()
        };
        let mut tex = None;
        unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut tex))? };
        tex.context("texture creation failed")
    }

    /// Wait up to `timeout_ms` for a new desktop frame and copy it into `dest`.
    pub fn next_frame(&self, dest: &ID3D11Texture2D, timeout_ms: u32) -> Result<Frame> {
        unsafe {
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut resource: Option<IDXGIResource> = None;
            match self.duplication.AcquireNextFrame(timeout_ms, &mut info, &mut resource) {
                Ok(()) => {}
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(Frame::Unchanged),
                Err(e) => bail!("AcquireNextFrame failed: {e}"),
            }
            let result = (|| -> Result<Frame> {
                // Mouse-only updates carry no new picture.
                if info.LastPresentTime == 0 {
                    return Ok(Frame::Unchanged);
                }
                let tex: ID3D11Texture2D = resource.context("no resource")?.cast()?;
                self.context.CopyResource(dest, &tex);
                Ok(Frame::New)
            })();
            self.duplication.ReleaseFrame()?;
            result
        }
    }
}
