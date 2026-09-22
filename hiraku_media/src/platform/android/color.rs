//! MediaFormat uses 0 for unspecified, not BT.709 or SDR. Resolve absent
//! metadata against the AV1 codec string before using explicit SDR defaults.
use crate::{CodecError, TransferFunction, YuvColorTransform};

pub(super) fn resolve(
    standard: Option<i32>,
    transfer: Option<i32>,
    range: Option<i32>,
    codec: &str,
) -> Result<(YuvColorTransform, TransferFunction), CodecError> {
    let fields: Vec<_> = codec.split('.').collect();
    let extended = fields.len() == 10 && fields[0] == "av01";
    let matrix = if extended { fields[8] } else { "01" };
    let (kr, kb) = match standard.filter(|value| *value != 0) {
        Some(1) => (0.2126, 0.0722),
        Some(2 | 4) => (0.299, 0.114),
        Some(6) => (0.2627, 0.0593),
        None => match matrix {
            "01" | "02" => (0.2126, 0.0722),
            "05" | "06" => (0.299, 0.114),
            "09" => (0.2627, 0.0593),
            _ => {
                return Err(CodecError::Unsupported(format!(
                    "AV1 matrix coefficients {matrix}"
                )));
            }
        },
        Some(value) => {
            return Err(CodecError::Unsupported(format!(
                "MediaCodec color standard {value}"
            )));
        }
    };
    let transfer = match transfer.filter(|value| *value != 0) {
        Some(1) => TransferFunction::Linear,
        // Android's native ColorAspects / Media3 use 2 for sRGB. It is not
        // the CICP value 2 (unspecified), and is not an HDR transfer.
        Some(2) => TransferFunction::Srgb,
        Some(3) => TransferFunction::Bt1886,
        None => match if extended { fields[7] } else { "01" } {
            "01" | "02" | "06" | "14" | "15" => TransferFunction::Bt1886,
            "04" => TransferFunction::Gamma22,
            "05" => TransferFunction::Gamma28,
            "08" => TransferFunction::Linear,
            "13" => TransferFunction::Srgb,
            value => {
                return Err(CodecError::Unsupported(format!(
                    "AV1 transfer {value}; HDR is not supported"
                )));
            }
        },
        Some(value @ (6 | 7)) => {
            return Err(CodecError::Unsupported(format!(
                "MediaCodec transfer {value}; HDR is not supported"
            )));
        }
        Some(value) => {
            return Err(CodecError::Unsupported(format!(
                "unknown MediaCodec transfer {value}"
            )));
        }
    };
    let limited = match range.filter(|value| *value != 0) {
        Some(1) => false,
        Some(2) => true,
        None if extended => match fields[9] {
            "0" => true,
            "1" => false,
            value => return Err(CodecError::Unsupported(format!("AV1 range {value}"))),
        },
        None => true,
        Some(value) => return Err(CodecError::Unsupported(format!("MediaCodec range {value}"))),
    };
    Ok((
        YuvColorTransform::from_luma_coefficients(kr, kb, limited),
        transfer,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_srgb_output_preserves_full_range_and_overrides_stream_transfer() {
        for codec in ["av01.0.04M.08", "av01.0.04M.08.0.110.01.01.01.0"] {
            let (matrix, transfer) = resolve(Some(1), Some(2), Some(1), codec)
                .expect("MediaCodec sRGB full-range output");
            assert_eq!(transfer, TransferFunction::Srgb);
            assert_eq!(matrix.luma, [1.0, 0.0]);
        }
    }

    #[test]
    fn hdr_and_unknown_transfers_are_reported_separately() {
        for transfer in [6, 7] {
            let error = resolve(None, Some(transfer), None, "av01.0.04M.08")
                .expect_err("HDR remains unsupported");
            assert!(error.to_string().contains("HDR is not supported"));
        }
        let error = resolve(None, Some(99), None, "av01.0.04M.08")
            .expect_err("unknown transfer remains unsupported");
        assert!(error.to_string().contains("unknown MediaCodec transfer 99"));
    }
    #[test]
    fn decoded_full_range_overrides_limited_stream_metadata() {
        let codec = "av01.0.04M.08.0.110.01.13.01.0";
        let (full, _) = resolve(Some(1), None, Some(1), codec).expect("full output");
        assert_eq!(full.luma, [1.0, 0.0]);
        let (limited, _) = resolve(Some(1), None, Some(2), codec).expect("limited output");
        for (sample, expected) in [(16.0 / 255.0, 0.0), (235.0 / 255.0, 1.0)] {
            assert!((sample * limited.luma[0] + limited.luma[1] - expected).abs() < 1e-6);
        }
    }
    #[test]
    fn unspecified_uses_stream_metadata_and_explicit_values_override_it() {
        let codec = "av01.0.04M.08.0.110.01.13.06.1";
        let (matrix, transfer) =
            resolve(Some(0), Some(0), Some(0), codec).expect("stream fallback");
        assert_eq!(transfer, TransferFunction::Srgb);
        assert_eq!(
            matrix.row_r,
            YuvColorTransform::from_luma_coefficients(0.299, 0.114, false).row_r
        );
        let (explicit, transfer) =
            resolve(Some(1), Some(3), Some(2), codec).expect("explicit format");
        assert_eq!(transfer, TransferFunction::Bt1886);
        assert_eq!(
            explicit.row_r,
            YuvColorTransform::from_luma_coefficients(0.2126, 0.0722, true).row_r
        );
        assert!(resolve(None, None, None, "av01.0.04M.08.0.110.09.16.09.0").is_err());
        assert!(resolve(Some(99), None, None, codec).is_err());
        assert!(resolve(None, None, None, "av01.0.04M.08").is_ok());
    }
}
