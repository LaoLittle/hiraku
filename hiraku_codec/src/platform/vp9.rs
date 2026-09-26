//! VP9 registry configuration accepted by the current 8-bit planar bridge.
use crate::{CodecError, VideoDecoderConfig};

pub(super) fn configuration(config: &VideoDecoderConfig) -> Result<[u8; 12], CodecError> {
    let fields: Vec<_> = config.codec.0.split('.').collect();
    let unsupported =
        || CodecError::Unsupported(format!("VP9 8-bit 4:2:0 configuration: {}", config.codec.0));
    if !matches!(fields.len(), 4 | 9)
        || fields[0] != "vp09"
        || config.description.is_some()
        || fields[1..]
            .iter()
            .any(|f| f.len() != 2 || !f.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(unsupported());
    }
    let numbers: Vec<u8> = fields[1..]
        .iter()
        .map(|f| f.parse().map_err(|_| unsupported()))
        .collect::<Result<_, _>>()?;
    let (profile, level, depth) = (numbers[0], numbers[1], numbers[2]);
    if profile != 0
        || depth != 8
        || !matches!(
            level,
            10 | 11 | 20 | 21 | 30 | 31 | 40 | 41 | 50 | 51 | 52 | 60 | 61 | 62
        )
    {
        return Err(unsupported());
    }
    let (chroma, primaries, transfer, matrix, full) = if numbers.len() == 8 {
        (numbers[3], numbers[4], numbers[5], numbers[6], numbers[7])
    } else {
        (1, 1, 1, 1, 0)
    };
    if chroma > 1
        || full > 1
        || !matches!(primaries, 1 | 2 | 5 | 6 | 9)
        || !matches!(transfer, 1 | 2 | 4 | 5 | 6 | 8 | 13 | 14 | 15)
        || !matches!(matrix, 1 | 2 | 5 | 6 | 9)
    {
        return Err(unsupported());
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registry_defaults_and_full_range_are_preserved() {
        let config = VideoDecoderConfig::new("vp09.00.10.08", 64, 64);
        assert_eq!(
            configuration(&config).expect("valid"),
            [1, 0, 0, 0, 0, 10, 130, 1, 1, 1, 0, 0]
        );
        let config = VideoDecoderConfig::new("vp09.00.10.08.01.01.13.01.01", 64, 64);
        let record = configuration(&config).expect("full range");
        assert_eq!(record[6], 131);
        assert_eq!(record[8], 13);
    }
    #[test]
    fn unsupported_profiles_and_malformed_strings_do_not_panic() {
        for codec in [
            "vp9",
            "vp09.00",
            "vp09.02.10.10",
            "vp09.00.99.08",
            "vp09.00.10.08.01.01.16.01.00",
        ] {
            assert!(configuration(&VideoDecoderConfig::new(codec, 64, 64)).is_err());
        }
    }
}
