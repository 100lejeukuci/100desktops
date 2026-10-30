//! Low-latency H.264 encoding on the GPU: BGRA -> NV12 (D3D11 video processor) -> Media Foundation hardware MFT.

use anyhow::{Context, Result, bail};
use std::mem::ManuallyDrop;
use std::time::{Duration, Instant};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_NV12, DXGI_SAMPLE_DESC};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Variant::VARIANT;
use windows::core::{GUID, Interface};

pub struct Packet {
    pub data: Vec<u8>,
    pub keyframe: bool,
    /// Time from handing the frame to the encoder until this packet came out.
    pub latency: Duration,
}

pub struct Encoder {
    transform: IMFTransform,
    codec: ICodecAPI,
    events: IMFMediaEventGenerator,
    _dxgi_manager: IMFDXGIDeviceManager,
    video_context: ID3D11VideoContext,
    processor: ID3D11VideoProcessor,
    in_view: ID3D11VideoProcessorInputView,
    out_view: ID3D11VideoProcessorOutputView,
    nv12: ID3D11Texture2D,
    need_input: u32,
    frame_index: i64,
    frame_duration: i64,
    submitted: std::collections::VecDeque<Instant>,
}

fn pack(hi: u32, lo: u32) -> u64 {
    ((hi as u64) << 32) | lo as u64
}

impl Encoder {
    pub fn new(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        source: &ID3D11Texture2D,
        width: u32,
        height: u32,
        fps: u32,
        bitrate: u32,
    ) -> Result<Self> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, MFSTARTUP_FULL)?;
            let _ = device.cast::<ID3D11Multithread>()?.SetMultithreadProtected(true);

            // --- BGRA -> NV12 on the GPU
            let nv12 = {
                let desc = D3D11_TEXTURE2D_DESC {
                    Width: width,
                    Height: height,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_NV12,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
                    ..Default::default()
                };
                let mut t = None;
                device.CreateTexture2D(&desc, None, Some(&mut t))?;
                t.context("NV12 texture")?
            };
            let video_device: ID3D11VideoDevice = device.cast()?;
            let video_context: ID3D11VideoContext = context.cast()?;
            let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputWidth: width,
                InputHeight: height,
                OutputWidth: width,
                OutputHeight: height,
                Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
                ..Default::default()
            };
            let enumerator = video_device.CreateVideoProcessorEnumerator(&content)?;
            let processor = video_device.CreateVideoProcessor(&enumerator, 0)?;
            let in_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                FourCC: 0,
                ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPIV { MipSlice: 0, ArraySlice: 0 },
                },
            };
            let mut in_view = None;
            video_device.CreateVideoProcessorInputView(source, &enumerator, &in_desc, Some(&mut in_view))?;
            let out_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
                },
            };
            let mut out_view = None;
            video_device.CreateVideoProcessorOutputView(&nv12, &enumerator, &out_desc, Some(&mut out_view))?;

            // --- hardware H.264 encoder MFT
            let in_info = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_NV12 };
            let out_info = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_H264 };
            let mut activates: *mut Option<IMFActivate> = std::ptr::null_mut();
            let mut count = 0u32;
            MFTEnumEx(
                MFT_CATEGORY_VIDEO_ENCODER,
                MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
                Some(&in_info),
                Some(&out_info),
                &mut activates,
                &mut count,
            )?;
            if count == 0 {
                bail!("no hardware H.264 encoder found");
            }
            let first = (*activates).clone().context("null activate")?;
            for i in 1..count as usize {
                std::ptr::drop_in_place(activates.add(i));
            }
            std::ptr::drop_in_place(activates);
            CoTaskMemFree(Some(activates as *const _));
            let transform: IMFTransform = first.ActivateObject()?;

            let attrs = transform.GetAttributes()?;
            attrs.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)?;
            let _ = attrs.SetUINT32(&MF_LOW_LATENCY, 1);

            let mut token = 0u32;
            let mut manager = None;
            MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
            let manager = manager.context("DXGI manager")?;
            manager.ResetDevice(device, token)?;
            transform.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, manager.as_raw() as usize)?;

            // Rate control / latency knobs. Failures are tolerated: not every driver exposes every knob.
            let codec: ICodecAPI = transform.cast()?;
            let set = |key: &GUID, v: VARIANT| {
                if let Err(e) = codec.SetValue(key, &v) {
                    eprintln!("codec option {key:?} not applied: {e}");
                }
            };
            set(&CODECAPI_AVLowLatencyMode, VARIANT::from(true));
            set(&CODECAPI_AVEncCommonRateControlMode, VARIANT::from(eAVEncCommonRateControlMode_CBR.0 as u32));
            set(&CODECAPI_AVEncCommonMeanBitRate, VARIANT::from(bitrate));
            set(&CODECAPI_AVEncMPVDefaultBPictureCount, VARIANT::from(0u32));
            set(&CODECAPI_AVEncMPVGOPSize, VARIANT::from(fps * 4));

            let out_type = MFCreateMediaType()?;
            out_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            out_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            out_type.SetUINT32(&MF_MT_AVG_BITRATE, bitrate)?;
            out_type.SetUINT64(&MF_MT_FRAME_SIZE, pack(width, height))?;
            out_type.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1))?;
            out_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
            out_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            out_type.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Main.0 as u32)?;
            transform.SetOutputType(0, &out_type, 0)?;

            let in_type = MFCreateMediaType()?;
            in_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            in_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
            in_type.SetUINT64(&MF_MT_FRAME_SIZE, pack(width, height))?;
            in_type.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1))?;
            in_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
            in_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            transform.SetInputType(0, &in_type, 0)?;

            transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0)?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;

            let events: IMFMediaEventGenerator = transform.cast()?;
            Ok(Self {
                transform,
                codec,
                events,
                _dxgi_manager: manager,
                video_context,
                processor,
                in_view: in_view.context("input view")?,
                out_view: out_view.context("output view")?,
                nv12,
                need_input: 0,
                frame_index: 0,
                frame_duration: 10_000_000 / fps as i64,
                submitted: Default::default(),
            })
        }
    }

    /// Make the next encoded frame an IDR keyframe (used when a viewer lost data).
    pub fn force_keyframe(&self) -> Result<()> {
        unsafe { self.codec.SetValue(&CODECAPI_AVEncVideoForceKeyFrame, &VARIANT::from(1u32))? };
        Ok(())
    }

    /// Convert and submit the current contents of the source texture, then return every packet that is ready.
    pub fn encode(&mut self) -> Result<Vec<Packet>> {
        unsafe {
            let started = Instant::now();
            let stream = D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: true.into(),
                pInputSurface: ManuallyDrop::new(Some(self.in_view.clone())),
                ..Default::default()
            };
            let blt = self.video_context.VideoProcessorBlt(&self.processor, &self.out_view, 0, &[stream]);
            blt?;

            let mut packets = Vec::new();
            // Wait for the encoder to ask for input.
            while self.need_input == 0 {
                self.pump_event(true, &mut packets)?;
            }

            let buffer = MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, &self.nv12, 0, false)?;
            let len = buffer.cast::<IMF2DBuffer>()?.GetContiguousLength()?;
            buffer.SetCurrentLength(len)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(self.frame_index * self.frame_duration)?;
            sample.SetSampleDuration(self.frame_duration)?;
            self.frame_index += 1;
            self.submitted.push_back(started);
            self.transform.ProcessInput(0, &sample, 0)?;
            self.need_input -= 1;

            // Give the encoder a short window to deliver this frame; whatever is not ready arrives next call.
            let deadline = Instant::now() + Duration::from_millis(30);
            while packets.is_empty() && Instant::now() < deadline {
                self.pump_event(false, &mut packets)?;
            }
            Ok(packets)
        }
    }

    /// Handle one transform event. With `block` it waits for an event, otherwise it returns if none is pending.
    fn pump_event(&mut self, block: bool, packets: &mut Vec<Packet>) -> Result<()> {
        unsafe {
            let flags = if block { MF_EVENT_FLAG_NONE } else { MF_EVENT_FLAG_NO_WAIT };
            let event = match self.events.GetEvent(flags) {
                Ok(e) => e,
                Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => return Ok(()),
                Err(e) => return Err(e.into()),
            };
            let kind = event.GetType()?;
            if kind == METransformNeedInput.0 as u32 {
                self.need_input += 1;
            } else if kind == METransformHaveOutput.0 as u32 {
                if let Some(p) = self.read_output()? {
                    packets.push(p);
                }
            }
            Ok(())
        }
    }

    fn read_output(&mut self) -> Result<Option<Packet>> {
        unsafe {
            let mut out = [MFT_OUTPUT_DATA_BUFFER::default()];
            let mut status = 0u32;
            match self.transform.ProcessOutput(0, &mut out, &mut status) {
                Ok(()) => {}
                Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(None),
                Err(e) => return Err(e.into()),
            }
            let sample = ManuallyDrop::take(&mut out[0].pSample).context("encoder returned no sample")?;
            let _events = ManuallyDrop::take(&mut out[0].pEvents);
            let buffer = sample.ConvertToContiguousBuffer()?;
            let (mut ptr, mut max, mut cur) = (std::ptr::null_mut(), 0u32, 0u32);
            buffer.Lock(&mut ptr, Some(&mut max), Some(&mut cur))?;
            let data = std::slice::from_raw_parts(ptr, cur as usize).to_vec();
            buffer.Unlock()?;
            let keyframe = sample.GetUINT32(&MFSampleExtension_CleanPoint).unwrap_or(0) != 0;
            let latency = self.submitted.pop_front().map(|t| t.elapsed()).unwrap_or_default();
            Ok(Some(Packet { data, keyframe, latency }))
        }
    }
}
