//! Pure validation helpers, also tested on hosts without libva.
use crate::CodecError;

pub(super) fn configuration_obus(description: Option<&[u8]>) -> Result<Vec<u8>, CodecError> {
    let Some(bytes) = description.filter(|bytes| !bytes.is_empty()) else {
        return Ok(Vec::new());
    };
    if bytes.len() < 4 || bytes[0] != 0x81 || bytes[3] & 0xe0 != 0 {
        return Err(CodecError::Configuration(
            "invalid AV1CodecConfigurationRecord".into(),
        ));
    }
    Ok(bytes[4..].to_vec())
}

pub(super) fn consumed(input: &[u8], count: usize) -> Result<&[u8], CodecError> {
    if count == 0 || count > input.len() {
        return Err(CodecError::Operation(
            "VA-API parser made no progress or consumed beyond the chunk".into(),
        ));
    }
    Ok(&input[count..])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn config_header_is_not_submitted_as_bitstream() {
        assert_eq!(
            configuration_obus(Some(&[0x81, 0, 0x0c, 0, 0x12, 0])).expect("valid av1C"),
            [0x12, 0]
        );
        assert!(
            configuration_obus(None)
                .expect("in-band configuration")
                .is_empty()
        );
        for bytes in [&[0x81][..], &[1, 0, 0, 0], &[0x81, 0, 0, 0xe0]] {
            assert!(configuration_obus(Some(bytes)).is_err());
        }
    }
    #[test]
    fn partial_input_consumption_is_checked() {
        assert_eq!(consumed(&[1, 2, 3], 1).expect("partial OBU"), [2, 3]);
        assert!(consumed(&[1], 0).is_err());
        assert!(consumed(&[1], 2).is_err());
    }
}
