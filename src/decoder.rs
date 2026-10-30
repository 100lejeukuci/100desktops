//! H.264 decoding through the Media Foundation decoder MFT with a D3D11 device manager (GPU decode, NV12 textures out).

use anyhow::{Context, Result};
use std::mem::ManuallyDrop;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Variant::VARIANT;
use windows::core::Interface;

pub struct DecodedFrame {
    pub texture: ID3D11Texture2D,
    /// Slice inside `texture` (decoder output is a texture array).
    pub subresource: u32,
    _sample: IMFSample,
}

pub struct Decoder {
    transform: IMFTransform,
    _dxgi_manager: IMFDXGIDeviceManager,
    pts: i64,
}

impl Decoder {
    pub fn new(device: &ID3D11Device) -> Result<Self> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, MFSTARTUP_FULL)?;
            let _ = device.cast::<ID3D11Multithread>()?.SetMultithreadProtected(true);

            let transform: IMFTransform = CoCreateInstance(&CLSID_MSH264DecoderMFT, None, CLSCTX_INPROC_SERVER)?;
            let attrs = transform.GetAttributes()?;
            let _ = attrs.SetUINT32(&MF_LOW_LATENCY, 1);
            let _ = attrs.SetUINT32(&MF_SA_D3D11_AWARE, 1);
            let _ = attrs.SetUINT32(&MF_SA_D3D11_BINDFLAGS, D3D11_BIND_SHADER_RESOURCE.0 as u32);

            if let Ok(codec) = transform.cast::<ICodecAPI>() {
                // Without this the decoder holds back frames waiting for possible reordering.
                if let Err(e) = codec.SetValue(&CODECAPI_AVLowLatencyMode, &VARIANT::from(1u32)) {
                    eprintln!("decoder low-latency mode not applied: {e}");
                }
            }

            let mut token = 0u32;
            let mut manager = None;
            MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
            let manager = manager.context("DXGI manager")?;
            manager.ResetDevice(device, token)?;
            transform.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, manager.as_raw() as usize)?;

            let in_type = MFCreateMediaType()?;
            in_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            in_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            in_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            transform.SetInputType(0, &in_type, 0)?;

            let mut dec = Self { transform, _dxgi_manager: manager, pts: 0 };
            let _ = dec.select_output_type(); // may only succeed once the first SPS has been seen
            dec.transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            dec.transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
            Ok(dec)
        }
    }

    fn select_output_type(&mut self) -> Result<()> {
        unsafe {
            for i in 0.. {
                let t = self.transform.GetOutputAvailableType(0, i)?;
                if t.GetGUID(&MF_MT_SUBTYPE)? == MFVideoFormat_NV12 {
                    self.transform.SetOutputType(0, &t, 0)?;
                    return Ok(());
                }
            }
            unreachable!()
        }
    }

    /// Feed one Annex-B access unit, return every frame the decoder finished.
    pub fn decode(&mut self, data: &[u8]) -> Result<Vec<DecodedFrame>> {
        unsafe {
            // A trailing access-unit delimiter tells the decoder this frame is complete, so it need not
            // wait for the first NAL of the next frame before emitting it (that wait costs a full frame).
            const AUD: [u8; 6] = [0, 0, 0, 1, 0x09, 0xF0];
            let len = data.len() + AUD.len();
            let buffer = MFCreateMemoryBuffer(len as u32)?;
            let (mut ptr, mut max) = (std::ptr::null_mut(), 0u32);
            buffer.Lock(&mut ptr, Some(&mut max), None)?;
            std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len());
            std::ptr::copy_nonoverlapping(AUD.as_ptr(), ptr.add(data.len()), AUD.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(len as u32)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(self.pts)?;
            self.pts += 166_667;

            let mut frames = Vec::new();
            loop {
                match self.transform.ProcessInput(0, &sample, 0) {
                    Ok(()) => break,
                    Err(e) if e.code() == MF_E_NOTACCEPTING => self.drain(&mut frames)?,
                    Err(e) => return Err(e.into()),
                }
            }
            self.drain(&mut frames)?;
            Ok(frames)
        }
    }

    fn drain(&mut self, frames: &mut Vec<DecodedFrame>) -> Result<()> {
        unsafe {
            loop {
                let mut out = [MFT_OUTPUT_DATA_BUFFER::default()];
                let mut status = 0u32;
                match self.transform.ProcessOutput(0, &mut out, &mut status) {
                    Ok(()) => {
                        let _events = ManuallyDrop::take(&mut out[0].pEvents);
                        let sample = ManuallyDrop::take(&mut out[0].pSample).context("decoder returned no sample")?;
                        let dxgi: IMFDXGIBuffer = sample.GetBufferByIndex(0)?.cast()?;
                        let mut raw = std::ptr::null_mut();
                        dxgi.GetResource(&ID3D11Texture2D::IID, &mut raw)?;
                        let texture = ID3D11Texture2D::from_raw(raw);
                        let subresource = dxgi.GetSubresourceIndex()?;
                        frames.push(DecodedFrame { texture, subresource, _sample: sample });
                    }
                    Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(()),
                    Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => self.select_output_type()?,
                    Err(e) => return Err(e.into()),
                }
            }
        }
    }
}
