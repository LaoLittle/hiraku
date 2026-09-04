use std::{
    collections::VecDeque,
    ffi::{c_char, c_void},
    ptr,
    slice,
    sync::Arc,
    time::Duration,
};

use crate::{TransferFunction, VideoFrame, VideoPixels, YuvColorTransform};

#[link(name = "VideoToolbox", kind = "framework")]
unsafe extern "C" {
    fn VTIsHardwareDecodeSupported(codec_type: OSType) -> bool;
}

pub(super) fn av1_hardware_decode_supported() -> bool {
    unsafe {
        VTIsHardwareDecodeSupported(K_CM_VIDEO_CODEC_TYPE_AV1)
    }
}

/// Synchronous VideoToolbox AV1 decoder.
///
/// All CoreFoundation/CoreMedia/CoreVideo/VideoToolbox references are private to this module.
/// `decode` and `finish` return owned `VideoFrame`s.
pub(super) struct VideoToolboxDecoder {
    session: VTDecompressionSessionRef,
    format_description: CMVideoFormatDescriptionRef,
    callback_context: Box<CallbackContext>,
    output_format: OutputPixelFormat,
}

impl VideoToolboxDecoder {
    pub(super) fn new(width: u32, height: u32, av1c: &[u8]) -> Result<Self, String> {
        let format_description = create_av1_format_description(width, height, av1c)?;
        let mut callback_context = Box::new(CallbackContext::default());
        let callback_ref_con = (&mut *callback_context as *mut CallbackContext).cast();

        let result = create_decoder(format_description, callback_ref_con);
        let (session, output_format) = match result {
            Ok(value) => value,
            Err(error) => {
                unsafe { cf_release(format_description.cast_const()) };
                return Err(error);
            }
        };

        Ok(Self {
            session,
            format_description,
            callback_context,
            output_format,
        })
    }

    pub(super) fn output_format_name(&self) -> &'static str {
        self.output_format.name()
    }

    /// Submit one compressed AV1 sample.
    ///
    /// A vector is returned rather than a single frame because VideoToolbox is allowed to emit more
    /// than one output callback while processing a sample. With the current synchronous/low-latency
    /// path this is normally exactly one frame.
    pub(super) fn decode(
        &mut self,
        packet: &[u8],
        pts: i64,
        duration: i64,
        time_base_numer: u32,
        time_base_denom: u32,
    ) -> Result<Vec<VideoFrame>, String> {
        self.callback_context.prepare();

        let pts = timestamp_to_cm_time(pts, time_base_numer, time_base_denom)?;
        let duration = timestamp_to_cm_time(duration, time_base_numer, time_base_denom)?;
        let sample = create_sample_buffer(packet, self.format_description, pts, duration)?;

        let mut info_flags: VTDecodeInfoFlags = 0;
        let status = unsafe {
            VTDecompressionSessionDecodeFrame(
                self.session,
                sample,
                0, // synchronous; intentionally no async/temporal-processing flags
                ptr::null_mut(),
                &mut info_flags,
            )
        };

        unsafe { cf_release(sample.cast_const()) };
        check_status(status, "VTDecompressionSessionDecodeFrame")?;
        self.callback_context.take_result()
    }

    /// Drain any delayed output before end-of-stream.
    pub(super) fn finish(&mut self) -> Result<Vec<VideoFrame>, String> {
        self.callback_context.prepare();

        check_status(
            unsafe { VTDecompressionSessionFinishDelayedFrames(self.session) },
            "VTDecompressionSessionFinishDelayedFrames",
        )?;
        check_status(
            unsafe { VTDecompressionSessionWaitForAsynchronousFrames(self.session) },
            "VTDecompressionSessionWaitForAsynchronousFrames",
        )?;

        self.callback_context.take_result()
    }
}

impl Drop for VideoToolboxDecoder {
    fn drop(&mut self) {
        unsafe {
            VTDecompressionSessionInvalidate(self.session);
            cf_release(self.session.cast_const());
            cf_release(self.format_description.cast_const());
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OutputPixelFormat {
    Nv12,
    I420,
}

impl OutputPixelFormat {
    const fn ostype(self) -> OSType {
        match self {
            Self::Nv12 => K_CV_PIXEL_FORMAT_TYPE_420YPCBCR8_BIPLANAR_VIDEO_RANGE,
            Self::I420 => K_CV_PIXEL_FORMAT_TYPE_420YPCBCR8_PLANAR,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Nv12 => "NV12 / 420v",
            Self::I420 => "I420 / y420",
        }
    }
}

#[derive(Default)]
struct CallbackContext {
    frames: VecDeque<VideoFrame>,
    error: Option<String>,
}

impl CallbackContext {
    fn prepare(&mut self) {
        self.frames.clear();
        self.error = None;
    }

    fn take_result(&mut self) -> Result<Vec<VideoFrame>, String> {
        if let Some(error) = self.error.take() {
            self.frames.clear();
            return Err(error);
        }
        Ok(self.frames.drain(..).collect())
    }
}

unsafe extern "C" fn output_callback(
    decompression_output_ref_con: *mut c_void,
    _source_frame_ref_con: *mut c_void,
    status: OSStatus,
    _info_flags: VTDecodeInfoFlags,
    image_buffer: CVImageBufferRef,
    presentation_time_stamp: CMTime,
    _presentation_duration: CMTime,
) {
    if decompression_output_ref_con.is_null() {
        return;
    }

    // SAFETY: the pointer is created from the Box<CallbackContext> owned by
    // VideoToolboxDecoder and the decoder uses synchronous decompression. The Box lives until after
    // the VT session has been invalidated.
    let context = unsafe { &mut *decompression_output_ref_con.cast::<CallbackContext>() };

    if status != NO_ERR {
        context.error = Some(format!("VideoToolbox output callback failed: OSStatus {status}"));
        return;
    }
    if image_buffer.is_null() {
        context.error = Some("VideoToolbox output callback returned a null image buffer".into());
        return;
    }

    let pixel_buffer = image_buffer as CVPixelBufferRef;
    match unsafe { copy_pixel_buffer(pixel_buffer, presentation_time_stamp) } {
        Ok(frame) => context.frames.push_back(frame),
        Err(error) => context.error = Some(error),
    }
}

unsafe fn copy_pixel_buffer(
    pixel_buffer: CVPixelBufferRef,
    presentation_time_stamp: CMTime,
) -> Result<VideoFrame, String> {
    let pixel_format = unsafe { CVPixelBufferGetPixelFormatType(pixel_buffer) };
    let width = usize_to_u32(unsafe { CVPixelBufferGetWidth(pixel_buffer) }, "frame width")?;
    let height = usize_to_u32(unsafe { CVPixelBufferGetHeight(pixel_buffer) }, "frame height")?;
    let plane_count = unsafe { CVPixelBufferGetPlaneCount(pixel_buffer) };
    let timestamp = cm_time_to_duration(presentation_time_stamp)?;

    let limited_range = match pixel_format {
        K_CV_PIXEL_FORMAT_TYPE_420YPCBCR8_BIPLANAR_VIDEO_RANGE
        | K_CV_PIXEL_FORMAT_TYPE_420YPCBCR8_PLANAR => true,
        _ => {
            return Err(format!(
                "unsupported VideoToolbox output pixel format {} (0x{pixel_format:08x})",
                fourcc(pixel_format)
            ));
        }
    };

    let (kr, kb) = unsafe { pixel_buffer_luma_coefficients(pixel_buffer) }?;
    let transfer = unsafe { pixel_buffer_transfer(pixel_buffer) }?;
    let color_transform = YuvColorTransform::from_luma_coefficients(kr, kb, limited_range);

    check_cv_return(
        unsafe { CVPixelBufferLockBaseAddress(pixel_buffer, K_CV_PIXEL_BUFFER_LOCK_READ_ONLY) },
        "CVPixelBufferLockBaseAddress",
    )?;

    let result = match pixel_format {
        K_CV_PIXEL_FORMAT_TYPE_420YPCBCR8_BIPLANAR_VIDEO_RANGE => {
            unsafe { copy_nv12(pixel_buffer, timestamp, width, height, color_transform, transfer, plane_count) }
        }
        K_CV_PIXEL_FORMAT_TYPE_420YPCBCR8_PLANAR => {
            unsafe { copy_i420(pixel_buffer, timestamp, width, height, color_transform, transfer, plane_count) }
        }
        _ => unreachable!(),
    };

    let unlock = unsafe {
        CVPixelBufferUnlockBaseAddress(pixel_buffer, K_CV_PIXEL_BUFFER_LOCK_READ_ONLY)
    };
    if unlock != K_CV_RETURN_SUCCESS {
        return Err(format!(
            "CVPixelBufferUnlockBaseAddress failed: CVReturn {unlock}"
        ));
    }

    result
}

unsafe fn copy_nv12(
    pixel_buffer: CVPixelBufferRef,
    timestamp: Duration,
    width: u32,
    height: u32,
    color_transform: YuvColorTransform,
    transfer: TransferFunction,
    plane_count: usize,
) -> Result<VideoFrame, String> {
    if plane_count != 2 {
        return Err(format!(
            "invalid NV12 CVPixelBuffer: expected 2 planes, got {plane_count}"
        ));
    }

    let chroma_width = usize_to_u32(
        unsafe { CVPixelBufferGetWidthOfPlane(pixel_buffer, 1) },
        "NV12 chroma width",
    )?;
    let chroma_height = usize_to_u32(
        unsafe { CVPixelBufferGetHeightOfPlane(pixel_buffer, 1) },
        "NV12 chroma height",
    )?;
    let y_stride = unsafe { CVPixelBufferGetBytesPerRowOfPlane(pixel_buffer, 0) };
    let uv_stride = unsafe { CVPixelBufferGetBytesPerRowOfPlane(pixel_buffer, 1) };
    let y_ptr = unsafe { CVPixelBufferGetBaseAddressOfPlane(pixel_buffer, 0) }.cast::<u8>();
    let uv_ptr = unsafe { CVPixelBufferGetBaseAddressOfPlane(pixel_buffer, 1) }.cast::<u8>();
    if y_ptr.is_null() || uv_ptr.is_null() {
        return Err("NV12 CVPixelBuffer has a null plane base address".into());
    }

    let y_len = checked_plane_len(y_stride, height, "NV12 Y plane")?;
    let uv_len = checked_plane_len(uv_stride, chroma_height, "NV12 UV plane")?;
    let uv_offset = y_len;
    let total_len = y_len
        .checked_add(uv_len)
        .ok_or_else(|| "NV12 frame allocation size overflow".to_string())?;

    let mut storage = Vec::with_capacity(total_len);
    storage.extend_from_slice(unsafe { slice::from_raw_parts(y_ptr, y_len) });
    storage.extend_from_slice(unsafe { slice::from_raw_parts(uv_ptr, uv_len) });

    Ok(VideoFrame {
        timestamp,
        width,
        height,
        chroma_width,
        chroma_height,
        color_transform,
        transfer,
        pixels: VideoPixels::Nv12Strided {
            planes: Arc::from(storage),
            uv_offset,
            y_stride: usize_to_u32(y_stride, "NV12 Y stride")?,
            uv_stride: usize_to_u32(uv_stride, "NV12 UV stride")?,
        },
    })
}

unsafe fn copy_i420(
    pixel_buffer: CVPixelBufferRef,
    timestamp: Duration,
    width: u32,
    height: u32,
    color_transform: YuvColorTransform,
    transfer: TransferFunction,
    plane_count: usize,
) -> Result<VideoFrame, String> {
    if plane_count != 3 {
        return Err(format!(
            "invalid I420 CVPixelBuffer: expected 3 planes, got {plane_count}"
        ));
    }

    let chroma_width = usize_to_u32(
        unsafe { CVPixelBufferGetWidthOfPlane(pixel_buffer, 1) },
        "I420 chroma width",
    )?;
    let chroma_height = usize_to_u32(
        unsafe { CVPixelBufferGetHeightOfPlane(pixel_buffer, 1) },
        "I420 chroma height",
    )?;
    let v_width = unsafe { CVPixelBufferGetWidthOfPlane(pixel_buffer, 2) };
    let v_height = unsafe { CVPixelBufferGetHeightOfPlane(pixel_buffer, 2) };
    if v_width != chroma_width as usize || v_height != chroma_height as usize {
        return Err(format!(
            "invalid I420 CVPixelBuffer: U plane is {chroma_width}x{chroma_height}, V plane is {v_width}x{v_height}"
        ));
    }

    let y_stride = unsafe { CVPixelBufferGetBytesPerRowOfPlane(pixel_buffer, 0) };
    let u_stride = unsafe { CVPixelBufferGetBytesPerRowOfPlane(pixel_buffer, 1) };
    let v_stride = unsafe { CVPixelBufferGetBytesPerRowOfPlane(pixel_buffer, 2) };
    if u_stride != v_stride {
        return Err(format!(
            "unsupported I420 CVPixelBuffer: U stride {u_stride} differs from V stride {v_stride}"
        ));
    }

    let y_ptr = unsafe { CVPixelBufferGetBaseAddressOfPlane(pixel_buffer, 0) }.cast::<u8>();
    let u_ptr = unsafe { CVPixelBufferGetBaseAddressOfPlane(pixel_buffer, 1) }.cast::<u8>();
    let v_ptr = unsafe { CVPixelBufferGetBaseAddressOfPlane(pixel_buffer, 2) }.cast::<u8>();
    if y_ptr.is_null() || u_ptr.is_null() || v_ptr.is_null() {
        return Err("I420 CVPixelBuffer has a null plane base address".into());
    }

    let y_len = checked_plane_len(y_stride, height, "I420 Y plane")?;
    let u_len = checked_plane_len(u_stride, chroma_height, "I420 U plane")?;
    let v_len = checked_plane_len(v_stride, chroma_height, "I420 V plane")?;
    let u_offset = y_len;
    let v_offset = y_len
        .checked_add(u_len)
        .ok_or_else(|| "I420 V offset overflow".to_string())?;
    let total_len = v_offset
        .checked_add(v_len)
        .ok_or_else(|| "I420 frame allocation size overflow".to_string())?;

    let mut storage = Vec::with_capacity(total_len);
    storage.extend_from_slice(unsafe { slice::from_raw_parts(y_ptr, y_len) });
    storage.extend_from_slice(unsafe { slice::from_raw_parts(u_ptr, u_len) });
    storage.extend_from_slice(unsafe { slice::from_raw_parts(v_ptr, v_len) });

    Ok(VideoFrame {
        timestamp,
        width,
        height,
        chroma_width,
        chroma_height,
        color_transform,
        transfer,
        pixels: VideoPixels::I420Strided {
            planes: Arc::from(storage),
            u_offset,
            v_offset,
            y_stride: usize_to_u32(y_stride, "I420 Y stride")?,
            chroma_stride: usize_to_u32(u_stride, "I420 chroma stride")?,
        },
    })
}

unsafe fn pixel_buffer_luma_coefficients(
    pixel_buffer: CVPixelBufferRef,
) -> Result<(f32, f32), String> {
    let value = unsafe { copy_attachment(pixel_buffer, kCVImageBufferYCbCrMatrixKey) };
    let Some(value) = value else {
        return Ok((0.2126, 0.0722)); // unspecified -> BT.709, same policy as native backend
    };

    let result = if unsafe { CFEqual(value, kCVImageBufferYCbCrMatrix_ITU_R_601_4) } != 0 {
        Ok((0.299, 0.114))
    } else if unsafe { CFEqual(value, kCVImageBufferYCbCrMatrix_SMPTE_240M_1995) } != 0 {
        Ok((0.2122, 0.0865))
    } else if unsafe { CFEqual(value, kCVImageBufferYCbCrMatrix_ITU_R_2020) } != 0 {
        Ok((0.2627, 0.0593))
    } else {
        // 709, P3 matrix values not explicitly handled, or unknown -> BT.709.
        Ok((0.2126, 0.0722))
    };

    unsafe { cf_release(value) };
    result
}

unsafe fn pixel_buffer_transfer(pixel_buffer: CVPixelBufferRef) -> Result<TransferFunction, String> {
    let value = unsafe { copy_attachment(pixel_buffer, kCVImageBufferTransferFunctionKey) };
    let Some(value) = value else {
        return Ok(TransferFunction::Bt1886);
    };

    let result = if unsafe { CFEqual(value, kCVImageBufferTransferFunction_sRGB) } != 0 {
        Ok(TransferFunction::Srgb)
    } else if unsafe { CFEqual(value, kCVImageBufferTransferFunction_SMPTE_ST_2084_PQ) } != 0 {
        Err("unsupported VideoToolbox transfer function: SMPTE ST 2084 PQ (HDR tone mapping is not implemented)".into())
    } else if unsafe { CFEqual(value, kCVImageBufferTransferFunction_ITU_R_2100_HLG) } != 0 {
        Err("unsupported VideoToolbox transfer function: ITU-R BT.2100 HLG (HDR tone mapping is not implemented)".into())
    } else if unsafe { CFEqual(value, kCVImageBufferTransferFunction_SMPTE_ST_428_1) } != 0 {
        Err("unsupported VideoToolbox transfer function: SMPTE ST 428-1 (HDR/log transfer is not implemented)".into())
    } else {
        // ITU-R 709/2020, SMPTE 240M, unspecified, and other SDR video curves follow the
        // existing Hiraku native-backend policy and use the BT.1886 display transfer.
        Ok(TransferFunction::Bt1886)
    };

    unsafe { cf_release(value) };
    result
}

unsafe fn copy_attachment(pixel_buffer: CVPixelBufferRef, key: CFStringRef) -> Option<CFTypeRef> {
    let mut mode: CVAttachmentMode = 0;
    let value = unsafe { CVBufferCopyAttachment(pixel_buffer.cast(), key, &mut mode) };
    (!value.is_null()).then_some(value)
}

fn create_decoder(
    format_description: CMVideoFormatDescriptionRef,
    callback_ref_con: *mut c_void,
) -> Result<(VTDecompressionSessionRef, OutputPixelFormat), String> {
    let probe = create_hardware_decoder_session(format_description, None, callback_ref_con)?;
    let performance_order = copy_performance_ordered_pixel_formats(probe)?;
    unsafe {
        VTDecompressionSessionInvalidate(probe);
        cf_release(probe.cast_const());
    }

    if let Some(formats) = performance_order {
        if let Some(selected) = formats.iter().copied().find_map(hiraku_pixel_format) {
            let session = create_hardware_decoder_session(
                format_description,
                Some(selected),
                callback_ref_con,
            )?;
            return Ok((session, selected));
        }

        let listed = formats
            .into_iter()
            .map(fourcc)
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "VideoToolbox hardware AV1 decoder exposes no Hiraku-compatible output format; performance list: [{listed}]"
        ));
    }

    // The performance list is optional. In its absence there is no API-provided speed ordering.
    // Probe the conventional Apple fast path first, then planar I420. Keep the first successful
    // session rather than destroying it and creating a third session.
    let mut errors = Vec::new();
    for candidate in [OutputPixelFormat::Nv12, OutputPixelFormat::I420] {
        match create_hardware_decoder_session(
            format_description,
            Some(candidate),
            callback_ref_con,
        ) {
            Ok(session) => return Ok((session, candidate)),
            Err(error) => errors.push(format!("{}: {error}", candidate.name())),
        }
    }

    Err(format!(
        "failed to create VideoToolbox hardware AV1 decoder with NV12 or I420 output: {}",
        errors.join("; ")
    ))
}

fn hiraku_pixel_format(format: OSType) -> Option<OutputPixelFormat> {
    match format {
        K_CV_PIXEL_FORMAT_TYPE_420YPCBCR8_BIPLANAR_VIDEO_RANGE => Some(OutputPixelFormat::Nv12),
        K_CV_PIXEL_FORMAT_TYPE_420YPCBCR8_PLANAR => Some(OutputPixelFormat::I420),
        _ => None,
    }
}

fn copy_performance_ordered_pixel_formats(
    session: VTDecompressionSessionRef,
) -> Result<Option<Vec<OSType>>, String> {
    let mut value: CFTypeRef = ptr::null();
    let status = unsafe {
        VTSessionCopyProperty(
            session,
            kVTDecompressionPropertyKey_SupportedPixelFormatsOrderedByPerformance,
            kCFAllocatorDefault,
            (&mut value as *mut CFTypeRef).cast(),
        )
    };

    // Apple documents this property as optional. Failure or a null value means no performance list
    // is available, so compatibility probing is required.
    if status != NO_ERR || value.is_null() {
        if !value.is_null() {
            unsafe { cf_release(value) };
        }
        return Ok(None);
    }

    let array = value as CFArrayRef;
    let count = unsafe { CFArrayGetCount(array) };
    let mut formats = Vec::with_capacity(count.max(0) as usize);
    for index in 0..count {
        let number = unsafe { CFArrayGetValueAtIndex(array, index) } as CFNumberRef;
        if number.is_null() {
            continue;
        }
        let mut raw: i32 = 0;
        let ok = unsafe {
            CFNumberGetValue(
                number,
                K_CF_NUMBER_SINT32_TYPE,
                (&mut raw as *mut i32).cast(),
            )
        };
        if ok != 0 {
            formats.push(raw as OSType);
        }
    }
    unsafe { cf_release(value) };
    Ok(Some(formats))
}

fn create_hardware_decoder_session(
    format_description: CMVideoFormatDescriptionRef,
    output_format: Option<OutputPixelFormat>,
    callback_ref_con: *mut c_void,
) -> Result<VTDecompressionSessionRef, String> {
    let decoder_spec = cf_dictionary(&[(
        unsafe { kVTVideoDecoderSpecification_RequireHardwareAcceleratedVideoDecoder },
        unsafe { kCFBooleanTrue },
    )])?;

    let mut pixel_format_number: CFNumberRef = ptr::null();
    let mut image_attributes: CFDictionaryRef = ptr::null();

    if let Some(output_format) = output_format {
        let pixel_format = output_format.ostype() as i32;
        pixel_format_number = unsafe {
            CFNumberCreate(
                kCFAllocatorDefault,
                K_CF_NUMBER_SINT32_TYPE,
                (&pixel_format as *const i32).cast(),
            )
        };
        if pixel_format_number.is_null() {
            unsafe { cf_release(decoder_spec) };
            return Err("CFNumberCreate(pixel format) failed".into());
        }
        image_attributes = match cf_dictionary(&[(
            unsafe { kCVPixelBufferPixelFormatTypeKey },
            pixel_format_number,
        )]) {
            Ok(value) => value,
            Err(error) => {
                unsafe {
                    cf_release(pixel_format_number);
                    cf_release(decoder_spec);
                }
                return Err(error);
            }
        };
    }

    let callback = VTDecompressionOutputCallbackRecord {
        decompression_output_callback: Some(output_callback),
        decompression_output_ref_con: callback_ref_con,
    };
    let mut session = ptr::null_mut();
    let status = unsafe {
        VTDecompressionSessionCreate(
            kCFAllocatorDefault,
            format_description,
            decoder_spec,
            image_attributes,
            &callback,
            &mut session,
        )
    };

    unsafe {
        cf_release(image_attributes);
        cf_release(pixel_format_number);
        cf_release(decoder_spec);
    }

    check_status(status, "VTDecompressionSessionCreate")?;
    if session.is_null() {
        return Err("VTDecompressionSessionCreate returned null".into());
    }
    Ok(session)
}

fn create_av1_format_description(
    width: u32,
    height: u32,
    av1c: &[u8],
) -> Result<CMVideoFormatDescriptionRef, String> {
    let av1c_data = unsafe {
        CFDataCreate(
            kCFAllocatorDefault,
            av1c.as_ptr(),
            isize::try_from(av1c.len()).map_err(|_| "av1C is too large".to_string())?,
        )
    };
    if av1c_data.is_null() {
        return Err("CFDataCreate(av1C) failed".into());
    }

    let av1c_key = match cf_string("av1C") {
        Ok(value) => value,
        Err(error) => {
            unsafe { cf_release(av1c_data) };
            return Err(error);
        }
    };
    let atoms = match cf_dictionary(&[(av1c_key, av1c_data)]) {
        Ok(value) => value,
        Err(error) => {
            unsafe {
                cf_release(av1c_key);
                cf_release(av1c_data);
            }
            return Err(error);
        }
    };
    let extensions = match cf_dictionary(&[(
        unsafe { kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms },
        atoms,
    )]) {
        Ok(value) => value,
        Err(error) => {
            unsafe {
                cf_release(atoms);
                cf_release(av1c_key);
                cf_release(av1c_data);
            }
            return Err(error);
        }
    };

    let mut format_description = ptr::null_mut();
    let status = unsafe {
        CMVideoFormatDescriptionCreate(
            kCFAllocatorDefault,
            K_CM_VIDEO_CODEC_TYPE_AV1,
            i32::try_from(width).map_err(|_| "video width exceeds i32".to_string())?,
            i32::try_from(height).map_err(|_| "video height exceeds i32".to_string())?,
            extensions,
            &mut format_description,
        )
    };

    unsafe {
        cf_release(extensions);
        cf_release(atoms);
        cf_release(av1c_key);
        cf_release(av1c_data);
    }

    check_status(status, "CMVideoFormatDescriptionCreate")?;
    if format_description.is_null() {
        return Err("CMVideoFormatDescriptionCreate returned null".into());
    }
    Ok(format_description)
}

fn create_sample_buffer(
    packet: &[u8],
    format_description: CMVideoFormatDescriptionRef,
    pts: CMTime,
    duration: CMTime,
) -> Result<CMSampleBufferRef, String> {
    let mut block_buffer = ptr::null_mut();
    let status = unsafe {
        CMBlockBufferCreateWithMemoryBlock(
            kCFAllocatorDefault,
            ptr::null_mut(),
            packet.len(),
            kCFAllocatorDefault,
            ptr::null(),
            0,
            packet.len(),
            0,
            &mut block_buffer,
        )
    };
    check_status(status, "CMBlockBufferCreateWithMemoryBlock")?;
    if block_buffer.is_null() {
        return Err("CMBlockBufferCreateWithMemoryBlock returned null".into());
    }

    let status = unsafe {
        CMBlockBufferReplaceDataBytes(packet.as_ptr().cast(), block_buffer, 0, packet.len())
    };
    if let Err(error) = check_status(status, "CMBlockBufferReplaceDataBytes") {
        unsafe { cf_release(block_buffer.cast_const()) };
        return Err(error);
    }

    let timing = CMSampleTimingInfo {
        duration,
        presentation_time_stamp: pts,
        decode_time_stamp: CMTime::INVALID,
    };
    let sample_size = packet.len();
    let mut sample_buffer = ptr::null_mut();
    let status = unsafe {
        CMSampleBufferCreateReady(
            kCFAllocatorDefault,
            block_buffer,
            format_description,
            1,
            1,
            &timing,
            1,
            &sample_size,
            &mut sample_buffer,
        )
    };
    unsafe { cf_release(block_buffer.cast_const()) };

    check_status(status, "CMSampleBufferCreateReady")?;
    if sample_buffer.is_null() {
        return Err("CMSampleBufferCreateReady returned null".into());
    }
    Ok(sample_buffer)
}

fn cf_string(value: &str) -> Result<CFStringRef, String> {
    let mut bytes = Vec::with_capacity(value.len() + 1);
    bytes.extend_from_slice(value.as_bytes());
    bytes.push(0);
    let result = unsafe {
        CFStringCreateWithCString(
            kCFAllocatorDefault,
            bytes.as_ptr().cast(),
            K_CF_STRING_ENCODING_UTF8,
        )
    };
    (!result.is_null())
        .then_some(result)
        .ok_or_else(|| format!("CFStringCreateWithCString failed for {value:?}"))
}

fn cf_dictionary(entries: &[(CFTypeRef, CFTypeRef)]) -> Result<CFDictionaryRef, String> {
    let keys = entries.iter().map(|(key, _)| *key).collect::<Vec<_>>();
    let values = entries.iter().map(|(_, value)| *value).collect::<Vec<_>>();
    let dictionary = unsafe {
        CFDictionaryCreate(
            kCFAllocatorDefault,
            keys.as_ptr(),
            values.as_ptr(),
            isize::try_from(entries.len()).map_err(|_| "CFDictionary is too large".to_string())?,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        )
    };
    (!dictionary.is_null())
        .then_some(dictionary)
        .ok_or_else(|| "CFDictionaryCreate failed".into())
}

unsafe fn cf_release(value: CFTypeRef) {
    if !value.is_null() {
        unsafe { CFRelease(value) };
    }
}

fn timestamp_to_cm_time(timestamp: i64, numer: u32, denom: u32) -> Result<CMTime, String> {
    if denom == 0 {
        return Err("invalid Symphonia time base denominator".into());
    }
    let value = timestamp
        .checked_mul(i64::from(numer))
        .ok_or_else(|| "timestamp overflow".to_string())?;
    let timescale = i32::try_from(denom)
        .map_err(|_| "time base denominator exceeds CoreMedia CMTime range".to_string())?;
    Ok(CMTime::new(value, timescale))
}

fn cm_time_to_duration(time: CMTime) -> Result<Duration, String> {
    if time.flags & K_CM_TIME_FLAGS_VALID == 0 || time.timescale <= 0 {
        return Err("VideoToolbox returned an invalid presentation timestamp".into());
    }
    let seconds = time.value as f64 / f64::from(time.timescale);
    if !seconds.is_finite() {
        return Err("VideoToolbox returned a non-finite presentation timestamp".into());
    }
    Ok(Duration::from_secs_f64(seconds.max(0.0)))
}

fn checked_plane_len(stride: usize, height: u32, name: &str) -> Result<usize, String> {
    stride
        .checked_mul(usize::try_from(height).map_err(|_| format!("{name} height overflow"))?)
        .ok_or_else(|| format!("{name} size overflow"))
}

fn usize_to_u32(value: usize, name: &str) -> Result<u32, String> {
    u32::try_from(value).map_err(|_| format!("{name} exceeds u32"))
}

fn check_status(status: OSStatus, operation: &str) -> Result<(), String> {
    if status == NO_ERR {
        Ok(())
    } else {
        Err(format!("{operation} failed: OSStatus {status}"))
    }
}

fn check_cv_return(status: CVReturn, operation: &str) -> Result<(), String> {
    if status == K_CV_RETURN_SUCCESS {
        Ok(())
    } else {
        Err(format!("{operation} failed: CVReturn {status}"))
    }
}

fn fourcc(value: OSType) -> String {
    let bytes = value.to_be_bytes();
    if bytes.iter().all(|byte| byte.is_ascii_graphic()) {
        String::from_utf8_lossy(&bytes).into_owned()
    } else {
        format!("0x{value:08x}")
    }
}

type OSStatus = i32;
type CVReturn = i32;
type OSType = u32;
type CFIndex = isize;
type CFTypeRef = *const c_void;
type CFAllocatorRef = *const c_void;
type CFStringRef = *const c_void;
type CFDataRef = *const c_void;
type CFDictionaryRef = *const c_void;
type CFNumberRef = *const c_void;
type CFArrayRef = *const c_void;
type CVAttachmentMode = u32;

type CMBlockBufferRef = *mut c_void;
type CMSampleBufferRef = *mut c_void;
type CMVideoFormatDescriptionRef = *mut c_void;
type VTDecompressionSessionRef = *mut c_void;
type CVImageBufferRef = *mut c_void;
type CVPixelBufferRef = *mut c_void;
type CVBufferRef = *mut c_void;

type VTDecodeFrameFlags = u32;
type VTDecodeInfoFlags = u32;

const NO_ERR: OSStatus = 0;
const K_CV_RETURN_SUCCESS: CVReturn = 0;
const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const K_CF_NUMBER_SINT32_TYPE: i32 = 3;
const K_CM_TIME_FLAGS_VALID: u32 = 1;
const K_CV_PIXEL_BUFFER_LOCK_READ_ONLY: u64 = 1;

const K_CM_VIDEO_CODEC_TYPE_AV1: OSType = u32::from_be_bytes(*b"av01");
const K_CV_PIXEL_FORMAT_TYPE_420YPCBCR8_BIPLANAR_VIDEO_RANGE: OSType =
    u32::from_be_bytes(*b"420v");
const K_CV_PIXEL_FORMAT_TYPE_420YPCBCR8_PLANAR: OSType = u32::from_be_bytes(*b"y420");

#[repr(C)]
#[derive(Clone, Copy)]
struct CMTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

impl CMTime {
    const INVALID: Self = Self {
        value: 0,
        timescale: 0,
        flags: 0,
        epoch: 0,
    };

    const fn new(value: i64, timescale: i32) -> Self {
        Self {
            value,
            timescale,
            flags: K_CM_TIME_FLAGS_VALID,
            epoch: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CMSampleTimingInfo {
    duration: CMTime,
    presentation_time_stamp: CMTime,
    decode_time_stamp: CMTime,
}

type VTDecompressionOutputCallback = unsafe extern "C" fn(
    decompression_output_ref_con: *mut c_void,
    source_frame_ref_con: *mut c_void,
    status: OSStatus,
    info_flags: VTDecodeInfoFlags,
    image_buffer: CVImageBufferRef,
    presentation_time_stamp: CMTime,
    presentation_duration: CMTime,
);

#[repr(C)]
struct VTDecompressionOutputCallbackRecord {
    decompression_output_callback: Option<VTDecompressionOutputCallback>,
    decompression_output_ref_con: *mut c_void,
}

#[repr(C)]
struct CFDictionaryKeyCallBacks {
    version: CFIndex,
    retain: *const c_void,
    release: *const c_void,
    copy_description: *const c_void,
    equal: *const c_void,
    hash: *const c_void,
}

#[repr(C)]
struct CFDictionaryValueCallBacks {
    version: CFIndex,
    retain: *const c_void,
    release: *const c_void,
    copy_description: *const c_void,
    equal: *const c_void,
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFAllocatorDefault: CFAllocatorRef;
    static kCFBooleanTrue: CFTypeRef;
    static kCFTypeDictionaryKeyCallBacks: CFDictionaryKeyCallBacks;
    static kCFTypeDictionaryValueCallBacks: CFDictionaryValueCallBacks;

    fn CFRelease(cf: CFTypeRef);
    fn CFEqual(cf1: CFTypeRef, cf2: CFTypeRef) -> u8;
    fn CFStringCreateWithCString(
        allocator: CFAllocatorRef,
        c_str: *const c_char,
        encoding: u32,
    ) -> CFStringRef;
    fn CFDataCreate(allocator: CFAllocatorRef, bytes: *const u8, length: CFIndex) -> CFDataRef;
    fn CFNumberCreate(
        allocator: CFAllocatorRef,
        the_type: i32,
        value_ptr: *const c_void,
    ) -> CFNumberRef;
    fn CFNumberGetValue(number: CFNumberRef, the_type: i32, value_ptr: *mut c_void) -> u8;
    fn CFArrayGetCount(array: CFArrayRef) -> CFIndex;
    fn CFArrayGetValueAtIndex(array: CFArrayRef, index: CFIndex) -> *const c_void;
    fn CFDictionaryCreate(
        allocator: CFAllocatorRef,
        keys: *const *const c_void,
        values: *const *const c_void,
        num_values: CFIndex,
        key_callbacks: *const CFDictionaryKeyCallBacks,
        value_callbacks: *const CFDictionaryValueCallBacks,
    ) -> CFDictionaryRef;
}

#[link(name = "CoreMedia", kind = "framework")]
unsafe extern "C" {
    static kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms: CFStringRef;

    fn CMVideoFormatDescriptionCreate(
        allocator: CFAllocatorRef,
        codec_type: OSType,
        width: i32,
        height: i32,
        extensions: CFDictionaryRef,
        format_description_out: *mut CMVideoFormatDescriptionRef,
    ) -> OSStatus;
    fn CMBlockBufferCreateWithMemoryBlock(
        structure_allocator: CFAllocatorRef,
        memory_block: *mut c_void,
        block_length: usize,
        block_allocator: CFAllocatorRef,
        custom_block_source: *const c_void,
        offset_to_data: usize,
        data_length: usize,
        flags: u32,
        block_buffer_out: *mut CMBlockBufferRef,
    ) -> OSStatus;
    fn CMBlockBufferReplaceDataBytes(
        source_bytes: *const c_void,
        destination_buffer: CMBlockBufferRef,
        offset_into_destination: usize,
        data_length: usize,
    ) -> OSStatus;
    fn CMSampleBufferCreateReady(
        allocator: CFAllocatorRef,
        data_buffer: CMBlockBufferRef,
        format_description: CMVideoFormatDescriptionRef,
        num_samples: isize,
        num_sample_timing_entries: isize,
        sample_timing_array: *const CMSampleTimingInfo,
        num_sample_size_entries: isize,
        sample_size_array: *const usize,
        sample_buffer_out: *mut CMSampleBufferRef,
    ) -> OSStatus;
}

#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    static kCVPixelBufferPixelFormatTypeKey: CFStringRef;

    static kCVImageBufferYCbCrMatrixKey: CFStringRef;
    static kCVImageBufferYCbCrMatrix_ITU_R_601_4: CFStringRef;
    static kCVImageBufferYCbCrMatrix_ITU_R_2020: CFStringRef;
    static kCVImageBufferYCbCrMatrix_SMPTE_240M_1995: CFStringRef;

    static kCVImageBufferTransferFunctionKey: CFStringRef;
    static kCVImageBufferTransferFunction_sRGB: CFStringRef;
    static kCVImageBufferTransferFunction_SMPTE_ST_428_1: CFStringRef;
    static kCVImageBufferTransferFunction_ITU_R_2100_HLG: CFStringRef;
    static kCVImageBufferTransferFunction_SMPTE_ST_2084_PQ: CFStringRef;

    fn CVBufferCopyAttachment(
        buffer: CVBufferRef,
        key: CFStringRef,
        attachment_mode: *mut CVAttachmentMode,
    ) -> CFTypeRef;
    fn CVPixelBufferGetPixelFormatType(pixel_buffer: CVPixelBufferRef) -> OSType;
    fn CVPixelBufferGetWidth(pixel_buffer: CVPixelBufferRef) -> usize;
    fn CVPixelBufferGetHeight(pixel_buffer: CVPixelBufferRef) -> usize;
    fn CVPixelBufferGetPlaneCount(pixel_buffer: CVPixelBufferRef) -> usize;
    fn CVPixelBufferGetWidthOfPlane(pixel_buffer: CVPixelBufferRef, plane_index: usize) -> usize;
    fn CVPixelBufferGetHeightOfPlane(pixel_buffer: CVPixelBufferRef, plane_index: usize) -> usize;
    fn CVPixelBufferGetBytesPerRowOfPlane(
        pixel_buffer: CVPixelBufferRef,
        plane_index: usize,
    ) -> usize;
    fn CVPixelBufferLockBaseAddress(pixel_buffer: CVPixelBufferRef, lock_flags: u64) -> CVReturn;
    fn CVPixelBufferUnlockBaseAddress(pixel_buffer: CVPixelBufferRef, unlock_flags: u64) -> CVReturn;
    fn CVPixelBufferGetBaseAddressOfPlane(
        pixel_buffer: CVPixelBufferRef,
        plane_index: usize,
    ) -> *mut c_void;
}

#[link(name = "VideoToolbox", kind = "framework")]
unsafe extern "C" {
    static kVTVideoDecoderSpecification_RequireHardwareAcceleratedVideoDecoder: CFStringRef;
    static kVTDecompressionPropertyKey_SupportedPixelFormatsOrderedByPerformance: CFStringRef;

    fn VTSessionCopyProperty(
        session: VTDecompressionSessionRef,
        property_key: CFStringRef,
        allocator: CFAllocatorRef,
        property_value_out: *mut c_void,
    ) -> OSStatus;
    fn VTDecompressionSessionCreate(
        allocator: CFAllocatorRef,
        video_format_description: CMVideoFormatDescriptionRef,
        video_decoder_specification: CFDictionaryRef,
        destination_image_buffer_attributes: CFDictionaryRef,
        output_callback: *const VTDecompressionOutputCallbackRecord,
        decompression_session_out: *mut VTDecompressionSessionRef,
    ) -> OSStatus;
    fn VTDecompressionSessionDecodeFrame(
        session: VTDecompressionSessionRef,
        sample_buffer: CMSampleBufferRef,
        decode_flags: VTDecodeFrameFlags,
        source_frame_ref_con: *mut c_void,
        info_flags_out: *mut VTDecodeInfoFlags,
    ) -> OSStatus;
    fn VTDecompressionSessionFinishDelayedFrames(session: VTDecompressionSessionRef) -> OSStatus;
    fn VTDecompressionSessionWaitForAsynchronousFrames(
        session: VTDecompressionSessionRef,
    ) -> OSStatus;
    fn VTDecompressionSessionInvalidate(session: VTDecompressionSessionRef);
}
