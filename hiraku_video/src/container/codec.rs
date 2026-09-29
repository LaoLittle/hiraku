//! Translate Matroska codec-private records to WebCodecs configurations.
use super::MediaError;
use hiraku_codec::{AvcCodec, Codec, HevcCodec, VideoDecoderConfig, Vp9Codec};
use std::sync::Arc;
use symphonia::core::codecs::video::{
    VideoCodecParameters,
    well_known::{self as ids, extra_data::*},
};

pub(super) fn video_config(
    vp: &VideoCodecParameters,
    width: u32,
    height: u32,
) -> Result<VideoDecoderConfig, MediaError> {
    let record_id = match vp.codec {
        ids::CODEC_ID_AV1 => Some(VIDEO_EXTRA_DATA_ID_AV1_DECODER_CONFIG),
        ids::CODEC_ID_H264 => Some(VIDEO_EXTRA_DATA_ID_AVC_DECODER_CONFIG),
        ids::CODEC_ID_HEVC => Some(VIDEO_EXTRA_DATA_ID_HEVC_DECODER_CONFIG),
        ids::CODEC_ID_VP9 => Some(VIDEO_EXTRA_DATA_ID_VP9_DECODER_CONFIG),
        ids::CODEC_ID_VP8 => None,
        other => return Err(MediaError::UnsupportedVideo(format!("{other:?}"))),
    };
    // Other additions, such as Dolby Vision metadata, are not codec descriptions.
    let description: Option<Arc<[u8]>> = record_id
        .and_then(|id| vp.extra_data.iter().find(|d| d.id == id))
        .map(|d| Arc::from(d.data.as_ref()));
    let invalid = || {
        MediaError::Container(format!(
            "invalid or missing codec-private record for {:?}",
            vp.codec
        ))
    };
    let codec = match vp.codec {
        ids::CODEC_ID_H264 => {
            let d = description
                .as_deref()
                .filter(|d| d.len() >= 7 && d[0] == 1)
                .ok_or_else(invalid)?;
            Codec::Avc(AvcCodec {
                in_band: false,
                profile: d[1],
                compatibility: d[2],
                level: d[3],
            })
        }
        ids::CODEC_ID_HEVC => {
            let d = description
                .as_deref()
                .filter(|d| d.len() >= 23 && d[0] == 1)
                .ok_or_else(invalid)?;
            Codec::Hevc(HevcCodec {
                in_band: false,
                profile_space: d[1] >> 6,
                profile: d[1] & 31,
                compatibility: u32::from_be_bytes([d[2], d[3], d[4], d[5]]).reverse_bits(),
                high_tier: d[1] & 32 != 0,
                level: d[12],
                constraints: d[6..12].try_into().map_err(|_| invalid())?,
            })
        }
        ids::CODEC_ID_AV1 => {
            let string = if let Some(d) = description.as_deref() {
                if d.len() < 4 || d[0] != 0x81 {
                    return Err(invalid());
                }
                format!(
                    "av01.{}.{:02}{}.{:02}",
                    d[1] >> 5,
                    d[1] & 31,
                    if d[2] & 128 != 0 { 'H' } else { 'M' },
                    if d[2] & 64 == 0 {
                        8
                    } else if d[2] & 32 == 0 {
                        10
                    } else {
                        12
                    }
                )
            } else {
                "av01.0.04M.08".into()
            };
            string
                .parse()
                .map_err(|e: hiraku_codec::CodecError| MediaError::Container(e.to_string()))?
        }
        ids::CODEC_ID_VP8 => Codec::Vp8,
        ids::CODEC_ID_VP9 => {
            // Matroska VP9 CodecPrivate is a TLV list, not an ISO vpcC record.
            let mut v = Vp9Codec {
                profile: 0,
                level: 10,
                bit_depth: 8,
                color: None,
            };
            let mut rest = description.as_deref().unwrap_or_default();
            while !rest.is_empty() {
                if rest.len() < 2 {
                    return Err(invalid());
                }
                let (id, len) = (rest[0], usize::from(rest[1]));
                rest = &rest[2..];
                let (value, next) = rest.split_at_checked(len).ok_or_else(invalid)?;
                rest = next;
                if matches!(id, 1..=4) && len != 1 {
                    return Err(invalid());
                }
                match id {
                    1 => v.profile = value[0],
                    2 => v.level = value[0],
                    3 => v.bit_depth = value[0],
                    4 => {
                        v.color = Some(hiraku_codec::Vp9Color {
                            chroma_subsampling: value[0],
                            primaries: 1,
                            transfer: 1,
                            matrix: 1,
                            full_range: false,
                        })
                    }
                    _ => {}
                }
            }
            Codec::Vp9(v)
        }
        _ => return Err(MediaError::UnsupportedVideo(format!("{:?}", vp.codec))),
    };
    codec
        .validate()
        .map_err(|e| MediaError::Container(e.to_string()))?;
    let mut config = VideoDecoderConfig::new(codec, width, height)
        .map_err(|e| MediaError::Container(e.to_string()))?;
    // WebCodecs VP8/VP9 do not take a description.
    if !matches!(vp.codec, ids::CODEC_ID_VP8 | ids::CODEC_ID_VP9) {
        config.description = description;
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use symphonia::core::codecs::video::VideoExtraData;
    #[test]
    fn avc_record_maps_profile_and_preserves_description() {
        let bytes = vec![1, 100, 0, 40, 255, 225, 0, 2, 103, 1, 1, 0, 2, 104, 1];
        let mut p = VideoCodecParameters {
            codec: ids::CODEC_ID_H264,
            ..Default::default()
        };
        p.extra_data.push(VideoExtraData {
            id: VIDEO_EXTRA_DATA_ID_AVC_DECODER_CONFIG,
            data: bytes.clone().into(),
        });
        let c = video_config(&p, 64, 32).expect("AVC mapping");
        assert_eq!(c.codec.to_string(), "avc1.640028");
        assert_eq!(c.description.as_deref(), Some(bytes.as_slice()));
    }
    #[test]
    fn hevc_compatibility_flags_reverse_registry_bit_order() {
        let mut data = vec![0; 23];
        data[0] = 1;
        data[1] = 1;
        data[2] = 0x60;
        data[6] = 0xb0;
        data[12] = 93;
        let mut p = VideoCodecParameters {
            codec: ids::CODEC_ID_HEVC,
            ..Default::default()
        };
        p.extra_data.push(VideoExtraData {
            id: VIDEO_EXTRA_DATA_ID_HEVC_DECODER_CONFIG,
            data: data.into(),
        });
        assert_eq!(
            video_config(&p, 64, 32)
                .expect("HEVC mapping")
                .codec
                .to_string(),
            "hvc1.1.6.L93.B0"
        );
    }
    #[test]
    fn vp9_private_data_is_not_forwarded_as_description() {
        let mut p = VideoCodecParameters {
            codec: ids::CODEC_ID_VP9,
            ..Default::default()
        };
        p.extra_data.push(VideoExtraData {
            id: VIDEO_EXTRA_DATA_ID_VP9_DECODER_CONFIG,
            data: vec![1, 1, 2, 2, 1, 31, 3, 1, 10].into(),
        });
        let c = video_config(&p, 64, 32).expect("VP9 mapping");
        assert_eq!(c.codec.to_string(), "vp09.02.31.10");
        assert!(c.description.is_none());
    }
    #[test]
    fn missing_avc_record_is_not_mislabelled_as_av1() {
        let p = VideoCodecParameters {
            codec: ids::CODEC_ID_H264,
            ..Default::default()
        };
        assert!(video_config(&p, 64, 32).is_err());
    }
}
