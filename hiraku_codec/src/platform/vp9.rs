//! VP9 registry configuration accepted by the current 8-bit planar bridge.
use crate::{CodecError, VideoDecoderConfig};

pub(super) fn configuration(config: &VideoDecoderConfig) -> Result<[u8; 12], CodecError> {
    config.codec.validate()?;
    let crate::Codec::Vp9(v) = &config.codec else {
        return Err(CodecError::Unsupported("expected VP9".into()));
    };
    if config.description.is_some() {
        return Err(CodecError::Configuration(
            "VP9 description must be absent".into(),
        ));
    }
    let (profile, level, depth) = (v.profile, v.level, v.bit_depth);
    let (chroma, primaries, transfer, matrix, full) =
        v.color.as_ref().map_or((1, 1, 1, 1, 0), |c| {
            (
                c.chroma_subsampling,
                c.primaries,
                c.transfer,
                c.matrix,
                u8::from(c.full_range),
            )
        });
    // ISO vpcC FullBox payload, not a WebCodecs description (VP9 has none).
    Ok([
        1,
        0,
        0,
        0,
        profile,
        level,
        (depth << 4) | (chroma << 1) | full,
        primaries,
        transfer,
        matrix,
        0,
        0,
    ])
}
