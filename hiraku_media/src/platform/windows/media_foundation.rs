//! AV1 MFT adapter. COM objects never leave the decoder worker thread.
use std::{
    mem::ManuallyDrop,
    ptr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use windows::{
    Win32::{
        Foundation::E_NOTIMPL,
        Media::MediaFoundation::*,
        System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize},
    },
    core::{Error, Interface, Result},
};

use crate::{TransferFunction, VideoFrame, VideoPixels, YuvColorTransform};

// Runtime is declared last so all COM interfaces are released before MFShutdown.
pub(in crate::platform) struct MediaFoundationDecoder {
    core: Transform,
    _runtime: Runtime,
}

struct Runtime;

impl Runtime {
    fn new() -> Result<Self> {
        // SAFETY: this is a dedicated worker; every successful initialization is
        // paired with shutdown on this same thread.
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
            if let Err(error) = MFStartup(MF_VERSION, MFSTARTUP_FULL) {
                CoUninitialize();
                return Err(error);
            }
        }
        Ok(Self)
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        unsafe {
            let _ = MFShutdown();
            CoUninitialize();
        }
    }
}

struct Activations {
    pointer: *mut Option<IMFActivate>,
    count: u32,
}

impl Activations {
    fn enumerate(flags: MFT_ENUM_FLAG) -> Result<Self> {
        let input = MFT_REGISTER_TYPE_INFO {
            guidMajorType: MFMediaType_Video,
            guidSubtype: MFVideoFormat_AV1,
        };
        let mut result = Self {
            pointer: ptr::null_mut(),
            count: 0,
        };
        unsafe {
            MFTEnumEx(
                MFT_CATEGORY_VIDEO_DECODER,
                flags,
                Some(&input),
                None,
                &mut result.pointer,
                &mut result.count,
            )?;
        }
        Ok(result)
    }

    fn entries(&self) -> &[Option<IMFActivate>] {
        if self.count == 0 || self.pointer.is_null() {
            &[]
        } else {
            // MFTEnumEx owns a count-element COM-task allocation.
            unsafe { std::slice::from_raw_parts(self.pointer, self.count as usize) }
        }
    }
}

impl Drop for Activations {
    fn drop(&mut self) {
        if !self.pointer.is_null() {
            unsafe {
                ptr::drop_in_place(ptr::slice_from_raw_parts_mut(
                    self.pointer,
                    self.count as usize,
                ));
                CoTaskMemFree(Some(self.pointer.cast()));
            }
        }
    }
}

struct Transform {
    transform: IMFTransform,
    activation: IMFActivate,
    events: Option<IMFMediaEventGenerator>,
    input: u32,
    output: u32,
    input_requests: usize,
    drained: bool,
    width: u32,
    height: u32,
}

impl Drop for Transform {
    fn drop(&mut self) {
        unsafe {
            let _ = self.transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
            let _ = self
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
            let _ = self.activation.ShutdownObject();
        }
    }
}

impl MediaFoundationDecoder {
    pub(in crate::platform) fn new(width: u32, height: u32) -> std::result::Result<Self, String> {
        let runtime =
            Runtime::new().map_err(|e| format!("Media Foundation startup failed: {e}"))?;
        let mut last_error = String::from("no installed AV1 MFT");
        // Prefer hardware MFTs accepting system-memory samples, then installed
        // synchronous/asynchronous decoders. GPU-surface-only MFTs are rejected
        // during negotiation; the caller can then use rav1d.
        for flags in [
            MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
            MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_ASYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER,
        ] {
            let candidates = Activations::enumerate(flags)
                .map_err(|e| format!("AV1 MFT enumeration failed: {e}"))?;
            for activation in candidates.entries().iter().flatten() {
                match Transform::new(activation, width, height) {
                    Ok(core) => {
                        return Ok(Self {
                            core,
                            _runtime: runtime,
                        });
                    }
                    Err(error) => {
                        last_error = error.to_string();
                        unsafe {
                            let _ = activation.ShutdownObject();
                        }
                    }
                }
            }
        }
        Err(format!(
            "no usable Media Foundation AV1 decoder: {last_error}"
        ))
    }

    pub(in crate::platform) fn decode(
        &mut self,
        packet: &[u8],
        pts: i64,
        duration: i64,
        numer: u32,
        denom: u32,
        cancellation: &AtomicBool,
    ) -> std::result::Result<Vec<VideoFrame>, String> {
        let pts = ticks(pts, numer, denom)?;
        let duration = ticks(duration, numer, denom)?;
        self.core
            .submit(packet, pts, duration, cancellation)
            .map_err(|e| format!("Media Foundation AV1 decode failed: {e}"))
    }

    pub(in crate::platform) fn finish(
        &mut self,
        cancellation: &AtomicBool,
    ) -> std::result::Result<Vec<VideoFrame>, String> {
        self.core
            .finish(cancellation)
            .map_err(|e| format!("Media Foundation drain failed: {e}"))
    }
}

impl Transform {
    fn new(activation: &IMFActivate, width: u32, height: u32) -> Result<Self> {
        unsafe {
            let transform: IMFTransform = activation.ActivateObject()?;
            let events = if let Ok(attributes) = transform.GetAttributes() {
                if attributes.GetUINT32(&MF_TRANSFORM_ASYNC).unwrap_or(0) != 0 {
                    attributes.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)?;
                    Some(transform.cast()?)
                } else {
                    None
                }
            } else {
                None
            };
            let mut inputs = 0;
            let mut outputs = 0;
            transform.GetStreamCount(&mut inputs, &mut outputs)?;
            if inputs != 1 || outputs != 1 {
                return Err(failure("AV1 decoder must expose one input and one output"));
            }
            let mut input = [0];
            let mut output = [0];
            if let Err(error) = transform.GetStreamIDs(&mut input, &mut output)
                && error.code() != E_NOTIMPL
            {
                return Err(error);
            }
            let media_type = MFCreateMediaType()?;
            media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            media_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_AV1)?;
            media_type.SetUINT64(
                &MF_MT_FRAME_SIZE,
                (u64::from(width) << 32) | u64::from(height),
            )?;
            transform.SetInputType(input[0], &media_type, 0)?;
            let mut decoder = Self {
                transform,
                activation: activation.clone(),
                events,
                input: input[0],
                output: output[0],
                input_requests: 0,
                drained: false,
                width,
                height,
            };
            decoder.negotiate_output()?;
            decoder
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            decoder
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
            Ok(decoder)
        }
    }

    fn negotiate_output(&mut self) -> Result<()> {
        unsafe {
            for index in 0..256 {
                let media_type = match self.transform.GetOutputAvailableType(self.output, index) {
                    Ok(value) => value,
                    Err(e) if e.code() == MF_E_NO_MORE_TYPES => break,
                    Err(e) => return Err(e),
                };
                if media_type.GetGUID(&MF_MT_SUBTYPE)? != MFVideoFormat_NV12 {
                    continue;
                }
                if self
                    .transform
                    .SetOutputType(self.output, &media_type, 0)
                    .is_ok()
                {
                    let size = media_type.GetUINT64(&MF_MT_FRAME_SIZE)?;
                    self.width = (size >> 32) as u32;
                    self.height = size as u32;
                    if self.width == 0
                        || self.height == 0
                        || self.width % 2 != 0
                        || self.height % 2 != 0
                    {
                        return Err(failure("NV12 output requires nonzero even dimensions"));
                    }
                    return Ok(());
                }
            }
        }
        Err(failure("AV1 MFT has no supported 8-bit NV12 output"))
    }

    fn submit(
        &mut self,
        packet: &[u8],
        pts: i64,
        duration: i64,
        cancel: &AtomicBool,
    ) -> Result<Vec<VideoFrame>> {
        let mut frames = Vec::new();
        let sample = unsafe {
            let sample = MFCreateSample()?;
            let len =
                u32::try_from(packet.len()).map_err(|_| failure("AV1 packet is too large"))?;
            let buffer = MFCreateMemoryBuffer(len)?;
            let mut pointer = ptr::null_mut();
            buffer.Lock(&mut pointer, None, None)?;
            // MF allocated len bytes. No fallible operations occur before Unlock.
            ptr::copy_nonoverlapping(packet.as_ptr(), pointer, packet.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(len)?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(pts)?;
            sample.SetSampleDuration(duration.max(0))?;
            sample
        };
        if self.events.is_some() {
            self.wait_events(cancel, &mut frames, false)?;
            unsafe {
                self.transform.ProcessInput(self.input, &sample, 0)?;
            }
            self.input_requests -= 1;
            self.poll_events(&mut frames)?;
        } else {
            match unsafe { self.transform.ProcessInput(self.input, &sample, 0) } {
                Err(e) if e.code() == MF_E_NOTACCEPTING => {
                    self.read_available(&mut frames)?;
                    unsafe {
                        self.transform.ProcessInput(self.input, &sample, 0)?;
                    }
                }
                result => result?,
            }
            self.read_available(&mut frames)?;
        }
        Ok(frames)
    }

    fn finish(&mut self, cancel: &AtomicBool) -> Result<Vec<VideoFrame>> {
        let mut frames = Vec::new();
        unsafe {
            self.transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, self.input as usize)?;
            self.transform
                .ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0)?;
        }
        if self.events.is_some() {
            self.wait_events(cancel, &mut frames, true)?;
        } else {
            self.read_available(&mut frames)?;
        }
        self.drained = false;
        self.input_requests = 0;
        unsafe {
            self.transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
        }
        Ok(frames)
    }

    fn wait_events(
        &mut self,
        cancel: &AtomicBool,
        frames: &mut Vec<VideoFrame>,
        draining: bool,
    ) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(failure("decode cancelled"));
            }
            self.poll_events(frames)?;
            if (draining && self.drained) || (!draining && self.input_requests > 0) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(failure("AV1 MFT event wait timed out"));
            }
            // Only the dedicated decoder worker sleeps, never an ECS system.
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn poll_events(&mut self, frames: &mut Vec<VideoFrame>) -> Result<()> {
        let Some(events) = self.events.clone() else {
            return Ok(());
        };
        loop {
            let event = match unsafe { events.GetEvent(MF_EVENT_FLAG_NO_WAIT) } {
                Ok(event) => event,
                Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => return Ok(()),
                Err(e) => return Err(e),
            };
            unsafe {
                event.GetStatus()?.ok()?;
            }
            match unsafe { event.GetType()? } as i32 {
                kind if kind == METransformNeedInput.0 => self.input_requests += 1,
                kind if kind == METransformHaveOutput.0 => {
                    if let Some(frame) = self.read_output()? {
                        frames.push(frame);
                    }
                }
                kind if kind == METransformDrainComplete.0 => self.drained = true,
                _ => {}
            }
        }
    }

    fn read_available(&mut self, frames: &mut Vec<VideoFrame>) -> Result<()> {
        while let Some(frame) = self.read_output()? {
            frames.push(frame);
        }
        Ok(())
    }

    fn read_output(&mut self) -> Result<Option<VideoFrame>> {
        for _ in 0..16 {
            unsafe {
                let info = self.transform.GetOutputStreamInfo(self.output)?;
                let provides_samples = info.dwFlags
                    & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0
                        | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0) as u32
                    != 0;
                let sample = if provides_samples {
                    None
                } else {
                    let sample = MFCreateSample()?;
                    let buffer = MFCreateAlignedMemoryBuffer(
                        info.cbSize,
                        info.cbAlignment.saturating_sub(1),
                    )?;
                    sample.AddBuffer(&buffer)?;
                    Some(sample)
                };
                let mut output = MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: self.output,
                    pSample: ManuallyDrop::new(sample),
                    ..Default::default()
                };
                let mut status = 0;
                let result =
                    self.transform
                        .ProcessOutput(0, std::slice::from_mut(&mut output), &mut status);
                // windows-rs uses ManuallyDrop for ABI structs: release both on
                // every HRESULT path, including stream-change and error paths.
                let sample = ManuallyDrop::take(&mut output.pSample);
                drop(ManuallyDrop::take(&mut output.pEvents));
                match result {
                    Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(None),
                    Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                        self.negotiate_output()?;
                        continue;
                    }
                    Err(e) => return Err(e),
                    Ok(()) => {
                        if output.dwStatus & MFT_OUTPUT_DATA_BUFFER_NO_SAMPLE.0 as u32
                            == MFT_OUTPUT_DATA_BUFFER_NO_SAMPLE.0 as u32
                        {
                            return Ok(None);
                        }
                        return sample.map(|sample| self.copy_frame(&sample)).transpose();
                    }
                }
            }
        }
        Err(failure(
            "AV1 MFT repeatedly changed output format without producing a frame",
        ))
    }

    fn copy_frame(&self, sample: &IMFSample) -> Result<VideoFrame> {
        let media_type = unsafe { self.transform.GetOutputCurrentType(self.output)? };
        copy_nv12(sample, &media_type, self.width, self.height)
    }
}

fn copy_nv12(
    sample: &IMFSample,
    media_type: &IMFMediaType,
    width: u32,
    height: u32,
) -> Result<VideoFrame> {
    unsafe {
        let buffer = sample.ConvertToContiguousBuffer()?;
        let (planes, stride) = if let Ok(buffer2d) = buffer.cast::<IMF2DBuffer>() {
            // ContiguousCopyTo removes the surface pitch according to the
            // media subtype; it also handles MFT-owned 2D buffers.
            let mut bytes = vec![0; buffer2d.GetContiguousLength()? as usize];
            buffer2d.ContiguousCopyTo(&mut bytes)?;
            (bytes, width)
        } else {
            let stride = media_type.GetUINT32(&MF_MT_DEFAULT_STRIDE).unwrap_or(width);
            if (stride as i32) <= 0 || stride < width {
                return Err(failure("unsupported NV12 row stride"));
            }
            let mut pointer = ptr::null_mut();
            let mut length = 0;
            buffer.Lock(&mut pointer, None, Some(&mut length))?;
            let bytes = if length == 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(pointer, length as usize).to_vec()
            };
            buffer.Unlock()?;
            (bytes, stride)
        };
        let uv_offset = (stride as usize)
            .checked_mul(height as usize)
            .ok_or_else(|| failure("NV12 plane offset overflow"))?;
        let expected = uv_offset
            .checked_add(uv_offset / 2)
            .ok_or_else(|| failure("NV12 buffer size overflow"))?;
        if planes.len() < expected {
            return Err(failure("AV1 MFT returned a truncated NV12 buffer"));
        }
        let matrix = media_type.GetUINT32(&MF_MT_YUV_MATRIX).unwrap_or(1);
        let (kr, kb) = match matrix {
            2 => (0.299, 0.114),
            3 => (0.2122, 0.0865),
            4 | 5 => (0.2627, 0.0593),
            _ => (0.2126, 0.0722),
        };
        let transfer = match media_type.GetUINT32(&MF_MT_TRANSFER_FUNCTION).unwrap_or(5) {
            0 | 5 | 6 | 11 | 12 | 13 => TransferFunction::Bt1886,
            1 => TransferFunction::Linear,
            4 => TransferFunction::Gamma22,
            7 => TransferFunction::Srgb,
            8 => TransferFunction::Gamma28,
            _ => {
                return Err(failure(
                    "unsupported AV1 transfer function: HDR/log tone mapping is not implemented",
                ));
            }
        };
        let limited = media_type
            .GetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE)
            .unwrap_or(2)
            != 1;
        let timestamp = sample.GetSampleTime()? / 10;
        Ok(VideoFrame {
            timestamp,
            width: width,
            height: height,
            chroma_width: width / 2,
            chroma_height: height / 2,
            color_transform: YuvColorTransform::from_luma_coefficients(kr, kb, limited),
            transfer,
            pixels: VideoPixels::Nv12Strided {
                planes: Arc::from(planes),
                uv_offset,
                y_stride: stride,
                uv_stride: stride,
            },
        })
    }
}

fn ticks(value: i64, numer: u32, denom: u32) -> std::result::Result<i64, String> {
    if denom == 0 {
        return Err("AV1 time base denominator is zero".into());
    }
    i64::try_from(i128::from(value) * i128::from(numer) * 10_000_000 / i128::from(denom))
        .map_err(|_| "AV1 timestamp exceeds Media Foundation's 100 ns clock range".into())
}

fn failure(message: &str) -> Error {
    Error::new(windows::Win32::Foundation::E_FAIL, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_preserve_signed_preroll_and_reject_overflow() {
        assert_eq!(ticks(-7, 1, 1000).expect("valid preroll"), -70_000);
        assert_eq!(
            ticks(90_000, 1, 90_000).expect("valid time base"),
            10_000_000
        );
        assert!(ticks(1, 1, 0).is_err());
        assert!(ticks(i64::MAX, u32::MAX, 1).is_err());
    }

    fn sample(bytes: &[u8]) -> Result<IMFSample> {
        unsafe {
            let buffer = MFCreateMemoryBuffer(bytes.len() as u32)?;
            let mut pointer = ptr::null_mut();
            buffer.Lock(&mut pointer, None, None)?;
            ptr::copy_nonoverlapping(bytes.as_ptr(), pointer, bytes.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(bytes.len() as u32)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(10_000_000)?;
            Ok(sample)
        }
    }

    #[test]
    fn nv12_preserves_padding_and_rejects_truncated_samples() -> Result<()> {
        let _runtime = Runtime::new()?;
        unsafe {
            let media_type = MFCreateMediaType()?;
            media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            media_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
            media_type.SetUINT32(&MF_MT_DEFAULT_STRIDE, 4)?;
            media_type.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, 1)?;
            media_type.SetUINT32(&MF_MT_TRANSFER_FUNCTION, 7)?;
            // 2x2 image in a four-byte row pitch, followed by interleaved UV.
            let bytes = [16, 32, 0, 0, 48, 64, 0, 0, 128, 128, 0, 0];
            let frame = copy_nv12(&sample(&bytes)?, &media_type, 2, 2)?;
            assert_eq!(frame.timestamp, 1_000_000);
            assert_eq!(frame.transfer, TransferFunction::Srgb);
            assert_eq!(frame.color_transform.row_r[0], 1.0);
            let VideoPixels::Nv12Strided {
                planes,
                uv_offset,
                y_stride,
                uv_stride,
            } = frame.pixels
            else {
                panic!("expected an NV12 frame");
            };
            assert_eq!(&*planes, &bytes);
            assert_eq!((uv_offset, y_stride, uv_stride), (8, 4, 4));
            assert!(copy_nv12(&sample(&bytes[..8])?, &media_type, 2, 2).is_err());
            media_type.SetUINT32(&MF_MT_DEFAULT_STRIDE, (-4_i32) as u32)?;
            assert!(copy_nv12(&sample(&bytes)?, &media_type, 2, 2).is_err());
        }
        Ok(())
    }
}
