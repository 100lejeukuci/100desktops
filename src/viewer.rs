//! Viewer window: decoded NV12 frames are scaled/converted to the swap chain by the GPU video processor.

use crate::decoder::DecodedFrame;
use anyhow::{Context, Result};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Interface, w};

pub struct Viewer {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    swapchain: IDXGISwapChain1,
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    width: u32,
    height: u32,
    pub screenshot: Option<String>,
}

/// Input seen by the window, in client-area pixels; turned into protocol events by the client.
#[derive(Clone, Copy, Debug)]
pub enum RawInput {
    Move { x: i32, y: i32 },
    /// button: 0 left, 1 right, 2 middle, 3 X1, 4 X2
    Button { button: u8, down: bool, x: i32, y: i32 },
    Key { scan: u16, extended: bool, down: bool },
    Wheel { delta: i16, horizontal: bool },
    ReleaseAll,
}

thread_local! {
    // The window procedure runs on the thread that pumps messages, so a thread-local queue is enough.
    static INPUT_QUEUE: std::cell::RefCell<Vec<RawInput>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn queue(input: RawInput) {
    INPUT_QUEUE.with(|q| q.borrow_mut().push(input));
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        let x = (lp.0 & 0xFFFF) as i16 as i32;
        let y = ((lp.0 >> 16) & 0xFFFF) as i16 as i32;
        let high_word = ((wp.0 >> 16) & 0xFFFF) as u16;
        const ANY_BUTTON: usize = 0x0073; // MK_L/R/M/X1/X2BUTTON
        match msg {
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                queue(RawInput::Move { x, y });
                LRESULT(0)
            }
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN => {
                let button = match msg {
                    WM_LBUTTONDOWN => 0,
                    WM_RBUTTONDOWN => 1,
                    WM_MBUTTONDOWN => 2,
                    _ if high_word == 1 => 3,
                    _ => 4,
                };
                SetCapture(hwnd); // keep receiving the drag when the pointer leaves the window
                queue(RawInput::Button { button, down: true, x, y });
                LRESULT(if msg == WM_XBUTTONDOWN { 1 } else { 0 })
            }
            WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP | WM_XBUTTONUP => {
                let button = match msg {
                    WM_LBUTTONUP => 0,
                    WM_RBUTTONUP => 1,
                    WM_MBUTTONUP => 2,
                    _ if high_word == 1 => 3,
                    _ => 4,
                };
                queue(RawInput::Button { button, down: false, x, y });
                if wp.0 & ANY_BUTTON == 0 {
                    let _ = ReleaseCapture();
                }
                LRESULT(if msg == WM_XBUTTONUP { 1 } else { 0 })
            }
            WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
                queue(RawInput::Wheel { delta: high_word as i16, horizontal: msg == WM_MOUSEHWHEEL });
                LRESULT(0)
            }
            WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP => {
                queue(RawInput::Key {
                    scan: ((lp.0 >> 16) & 0xFF) as u16,
                    extended: (lp.0 >> 24) & 1 != 0,
                    down: msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN,
                });
                // Swallowed on purpose: Alt+F4, F10 and Alt menus belong to the remote machine, not this window.
                LRESULT(0)
            }
            WM_SYSCHAR => LRESULT(0),
            WM_KILLFOCUS => {
                queue(RawInput::ReleaseAll);
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

/// D3D11 device for a machine that only decodes and displays (no capture adapter involved).
pub fn create_device() -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    unsafe {
        let mut device = None;
        let mut context = None;
        D3D11CreateDevice(
            None,
            windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE,
            windows::Win32::Foundation::HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )?;
        Ok((device.context("D3D11 device")?, context.context("D3D11 context")?))
    }
}

impl Viewer {
    pub fn new(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        src_w: u32,
        src_h: u32,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        unsafe {
            let hinst = GetModuleHandleW(None)?;
            let class = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: hinst.into(),
                lpszClassName: w!("RemoteDeskViewer"),
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                ..Default::default()
            };
            RegisterClassW(&class);
            let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
            let mut rect = RECT { left: 0, top: 0, right: width as i32, bottom: height as i32 };
            AdjustWindowRect(&mut rect, style, false)?;
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("RemoteDeskViewer"),
                w!("Remote Desk viewer"),
                style | WS_VISIBLE,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                rect.right - rect.left,
                rect.bottom - rect.top,
                None,
                None,
                Some(hinst.into()),
                None,
            )?;

            let dxgi_device: IDXGIDevice = device.cast()?;
            let factory: IDXGIFactory2 = dxgi_device.GetAdapter()?.GetParent()?;
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: width,
                Height: height,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                Flags: DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING.0 as u32,
                ..Default::default()
            };
            let swapchain = factory.CreateSwapChainForHwnd(device, hwnd, &desc, None, None)?;

            let video_device: ID3D11VideoDevice = device.cast()?;
            let video_context: ID3D11VideoContext = context.cast()?;
            let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputWidth: src_w,
                InputHeight: src_h,
                OutputWidth: width,
                OutputHeight: height,
                Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
                ..Default::default()
            };
            let enumerator = video_device.CreateVideoProcessorEnumerator(&content)?;
            let processor = video_device.CreateVideoProcessor(&enumerator, 0)?;

            Ok(Self {
                device: device.clone(),
                context: context.clone(),
                swapchain,
                video_device,
                video_context,
                enumerator,
                processor,
                width,
                height,
                screenshot: None,
            })
        }
    }

    /// Input collected by the window since the last call.
    pub fn take_input(&self) -> Vec<RawInput> {
        INPUT_QUEUE.with(|q| std::mem::take(&mut *q.borrow_mut()))
    }

    /// Process window messages. Returns false once the window was closed.
    pub fn pump(&self) -> bool {
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    return false;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            true
        }
    }

    pub fn present(&mut self, frame: &DecodedFrame) -> Result<()> {
        unsafe {
            let backbuffer: ID3D11Texture2D = self.swapchain.GetBuffer(0)?;
            let in_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                FourCC: 0,
                ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPIV { MipSlice: 0, ArraySlice: frame.subresource },
                },
            };
            let mut in_view = None;
            self.video_device.CreateVideoProcessorInputView(&frame.texture, &self.enumerator, &in_desc, Some(&mut in_view))?;
            let out_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 } },
            };
            let mut out_view = None;
            self.video_device.CreateVideoProcessorOutputView(&backbuffer, &self.enumerator, &out_desc, Some(&mut out_view))?;

            let stream = D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: true.into(),
                pInputSurface: std::mem::ManuallyDrop::new(in_view),
                ..Default::default()
            };
            let streams = [stream];
            let blt = self.video_context.VideoProcessorBlt(&self.processor, &out_view.context("out view")?, 0, &streams);
            let [stream] = streams;
            drop(std::mem::ManuallyDrop::into_inner(stream.pInputSurface));
            blt?;

            if let Some(path) = self.screenshot.take() {
                self.save_bmp(&backbuffer, &path)?;
            }
            self.swapchain.Present(0, DXGI_PRESENT_ALLOW_TEARING).ok()?;
            Ok(())
        }
    }

    /// Debug aid: dump the current back buffer as a BMP so the picture can be checked.
    fn save_bmp(&self, tex: &ID3D11Texture2D, path: &str) -> Result<()> {
        unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            tex.GetDesc(&mut desc);
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            desc.MiscFlags = 0;
            let mut staging = None;
            self.device.CreateTexture2D(&desc, None, Some(&mut staging))?;
            let staging = staging.context("staging texture")?;
            self.context.CopyResource(&staging, tex);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let (w, h) = (self.width as usize, self.height as usize);
            let mut out = Vec::with_capacity(54 + w * h * 4);
            out.extend_from_slice(b"BM");
            out.extend_from_slice(&((54 + w * h * 4) as u32).to_le_bytes());
            out.extend_from_slice(&[0; 4]);
            out.extend_from_slice(&54u32.to_le_bytes());
            out.extend_from_slice(&40u32.to_le_bytes());
            out.extend_from_slice(&(w as i32).to_le_bytes());
            out.extend_from_slice(&(h as i32).to_le_bytes());
            out.extend_from_slice(&1u16.to_le_bytes());
            out.extend_from_slice(&32u16.to_le_bytes());
            out.extend_from_slice(&[0; 24]);
            for y in (0..h).rev() {
                let row = std::slice::from_raw_parts((mapped.pData as *const u8).add(y * mapped.RowPitch as usize), w * 4);
                out.extend_from_slice(row);
            }
            self.context.Unmap(&staging, 0);
            std::fs::write(path, out)?;
            Ok(())
        }
    }
}
