use super::Result;
use basis_universal::{BasisTextureFormat, ColorSpace, Compressor, CompressorParams};

pub const UASTC_LEVEL: u32 = 3;

/// Bound intra-image parallelism, respecting Cargo's available job budget.
pub fn encoder_threads() -> u32 {
    cpu_budget().clamp(1, 8) as u32
}

pub(crate) fn cpu_budget() -> usize {
    let hardware = std::thread::available_parallelism().map_or(1, |count| count.get());
    let jobs = std::env::var("NUM_JOBS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|jobs| *jobs > 0)
        .unwrap_or(hardware);
    hardware.min(jobs).max(1)
}

/// Encode one RGBA image and wrap its UASTC blocks in unsupercompressed KTX2.
/// The pinned encoder exposes .basis only, so container writing is host-only.
pub fn encode_rgba(bytes: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
    encode_rgba_with_threads(bytes, width, height, encoder_threads())
}

pub fn encode_rgba_with_threads(
    bytes: &[u8],
    width: u32,
    height: u32,
    threads: u32,
) -> Result<Vec<u8>> {
    if !(1..=64).contains(&threads) {
        return Err("encoder threads must be in 1..=64".into());
    }
    if width == 0
        || height == 0
        || width > 16384
        || height > 16384
        || bytes.len() != width as usize * height as usize * 4
    {
        return Err("invalid RGBA texture dimensions or byte count".into());
    }
    let mut params = CompressorParams::new();
    params.set_basis_format(BasisTextureFormat::UASTC4x4);
    params.set_uastc_quality_level(UASTC_LEVEL);
    params.set_color_space(ColorSpace::Srgb);
    params.set_generate_mipmaps(false);
    params.set_print_status_to_stdout(false);
    params.source_image_mut(0).init(bytes, width, height, 4);
    let mut compressor = Compressor::new(threads);
    // SAFETY: exactly one nonempty RGBA8 image with validated dimensions and
    // length; params and pixels remain alive throughout the synchronous encode.
    unsafe {
        if !compressor.init(&params) {
            return Err("UASTC encoder initialization failed".into());
        }
        compressor
            .process()
            .map_err(|error| format!("UASTC encode failed: {error:?}"))?;
    }
    let basis = compressor.basis_file();
    let read_u32 = |offset: usize| -> Result<u32> {
        Ok(u32::from_le_bytes(
            basis
                .get(offset..offset + 4)
                .ok_or("truncated encoder output")?
                .try_into()?,
        ))
    };
    // basis_file_header.m_slice_desc_file_ofs and basis_slice_desc layout,
    // pinned by basis-universal 0.3.1. This writer accepts only one LDR slice.
    if basis.get(14..21) != Some(&[1, 0, 0, 1, 0, 0, 1][..]) {
        return Err("unexpected UASTC slice layout".into());
    }
    let descriptor = read_u32(65)? as usize;
    let offset = read_u32(descriptor + 13)? as usize;
    let size = read_u32(descriptor + 17)? as usize;
    let blocks = basis
        .get(offset..offset.checked_add(size).ok_or("slice overflow")?)
        .ok_or("invalid encoder slice")?;
    if size != width.div_ceil(4) as usize * height.div_ceil(4) as usize * 16 {
        return Err("unexpected UASTC block length".into());
    }
    // Header (80), one level index (24), Khronos basic DFD (44), padding to 16.
    let mut result = vec![0u8; 160];
    result[..12].copy_from_slice(b"\xabKTX 20\xbb\r\n\x1a\n");
    let put = |out: &mut [u8], at: usize, value: u32| {
        out[at..at + 4].copy_from_slice(&value.to_le_bytes())
    };
    put(&mut result, 16, 1); // typeSize
    put(&mut result, 20, width);
    put(&mut result, 24, height);
    put(&mut result, 36, 1); // faceCount
    put(&mut result, 40, 1); // levelCount; supercompression = 0
    put(&mut result, 48, 104);
    put(&mut result, 52, 44);
    for (at, value) in [(80, 160u64), (88, size as u64), (96, size as u64)] {
        result[at..at + 8].copy_from_slice(&value.to_le_bytes());
    }
    put(&mut result, 104, 44); // total DFD size
    put(&mut result, 112, 2 | (40 << 16)); // version 1.3, descriptor block size
    put(&mut result, 116, 166 | (1 << 8) | (2 << 16)); // UASTC, BT709 primaries, sRGB transfer
    put(&mut result, 120, 3 | (3 << 8)); // 4x4 block
    put(&mut result, 124, 16); // bytesPlane0
    put(&mut result, 132, (127 << 16) | (3 << 24)); // 128-bit RGBA sample
    put(&mut result, 144, u32::MAX); // sampleUpper
    result.extend_from_slice(blocks);
    Ok(result)
}
