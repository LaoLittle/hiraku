use crate::{Codec, CodecError, VideoDecoderConfig};
use std::borrow::Cow;

#[allow(
    dead_code,
    reason = "platform adapters consume different configuration fields"
)]
pub(super) struct NalInput {
    pub length_size: Option<usize>,
    pub parameter_sets: Vec<Vec<u8>>,
    pub bit_depth: Option<u8>,
}
fn invalid() -> CodecError {
    CodecError::Configuration("invalid AVC/HEVC decoder configuration record or NAL lengths".into())
}

/// Annex B start codes are not part of the NAL payload.
pub(super) fn annex_b_units(bytes: &[u8]) -> Result<Vec<&[u8]>, CodecError> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= bytes.len() {
        let n = if bytes[i..].starts_with(&[0, 0, 0, 1]) {
            4
        } else if bytes[i..].starts_with(&[0, 0, 1]) {
            3
        } else {
            i += 1;
            continue;
        };
        starts.push((i, i + n));
        i += n;
    }
    let Some((first, _)) = starts.first() else {
        return Err(invalid());
    };
    if bytes[..*first].iter().any(|b| *b != 0) {
        return Err(invalid());
    }
    let mut result = Vec::new();
    for (index, (_, start)) in starts.iter().enumerate() {
        let mut end = starts.get(index + 1).map_or(bytes.len(), |v| v.0);
        while end > *start && bytes[end - 1] == 0 {
            end -= 1;
        }
        if end == *start {
            return Err(invalid());
        }
        result.push(&bytes[*start..end]);
    }
    Ok(result)
}
struct Reader<'a>(&'a [u8]);
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], CodecError> {
        let (head, tail) = self.0.split_at_checked(n).ok_or_else(invalid)?;
        self.0 = tail;
        Ok(head)
    }
    fn byte(&mut self) -> Result<u8, CodecError> {
        Ok(self.take(1)?[0])
    }
    fn short(&mut self) -> Result<usize, CodecError> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]) as usize)
    }
    fn sets(&mut self, count: usize, result: &mut Vec<Vec<u8>>) -> Result<(), CodecError> {
        for _ in 0..count {
            let n = self.short()?;
            if n == 0 {
                return Err(invalid());
            }
            result.push(self.take(n)?.to_vec());
        }
        Ok(())
    }
}
impl NalInput {
    pub fn new(config: &VideoDecoderConfig) -> Result<Option<Self>, CodecError> {
        let hevc = match config.codec {
            Codec::Avc(_) => false,
            Codec::Hevc(_) => true,
            _ => return Ok(None),
        };
        let Some(data) = config.description.as_deref() else {
            return Ok(Some(Self {
                length_size: None,
                parameter_sets: Vec::new(),
                bit_depth: None,
            }));
        };
        let mut r = Reader(data);
        let mut sets = Vec::new();
        let (length, depth) = if hevc {
            let header = r.take(23)?;
            if header[0] != 1 {
                return Err(invalid());
            }
            for _ in 0..header[22] {
                r.byte()?;
                let n = r.short()?;
                r.sets(n, &mut sets)?;
            }
            if !r.0.is_empty() {
                return Err(invalid());
            }
            ((header[21] & 3) as usize + 1, Some(8 + (header[17] & 7)))
        } else {
            let header = r.take(6)?;
            if header[0] != 1 {
                return Err(invalid());
            }
            r.sets((header[5] & 31) as usize, &mut sets)?;
            let count = r.byte()? as usize;
            r.sets(count, &mut sets)?;
            let mut depth = Some(8);
            // High-profile optional extension includes bit depth and SPS ext.
            if !r.0.is_empty() && matches!(header[1], 100 | 110 | 122 | 144) {
                let ext = r.take(4)?;
                depth = Some(8 + (ext[1] & 7));
                r.sets(ext[3] as usize, &mut sets)?;
            }
            if !r.0.is_empty() {
                return Err(invalid());
            }
            ((header[4] & 3) as usize + 1, depth)
        };
        if length == 3 || sets.is_empty() {
            return Err(invalid());
        }
        Ok(Some(Self {
            length_size: Some(length),
            parameter_sets: sets,
            bit_depth: depth,
        }))
    }
    #[allow(
        dead_code,
        reason = "VideoToolbox consumes length-prefixed input; other adapters consume Annex B"
    )]
    pub fn annex_b<'a>(&self, bytes: &'a [u8], key: bool) -> Result<Cow<'a, [u8]>, CodecError> {
        let Some(length) = self.length_size else {
            annex_b_units(bytes)?;
            return Ok(Cow::Borrowed(bytes));
        };
        let mut result = Vec::new();
        if key {
            for set in &self.parameter_sets {
                result.extend_from_slice(&[0, 0, 0, 1]);
                result.extend_from_slice(set);
            }
        }
        let mut r = Reader(bytes);
        while !r.0.is_empty() {
            let size = r
                .take(length)?
                .iter()
                .fold(0usize, |n, b| (n << 8) | usize::from(*b));
            if size == 0 {
                return Err(invalid());
            }
            let nal = r.take(size)?;
            result.extend_from_slice(&[0, 0, 0, 1]);
            result.extend_from_slice(nal);
        }
        Ok(Cow::Owned(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn annex_b_handles_start_codes_and_rejects_invalid_prefixes() {
        assert_eq!(
            annex_b_units(&[0, 0, 0, 1, 103, 1, 0, 0, 1, 104, 2, 0]).expect("units"),
            vec![&[103, 1][..], &[104, 2][..]]
        );
        for bad in [&[][..], &[1, 2, 3], &[0, 0, 1], &[5, 0, 0, 1, 9]] {
            assert!(annex_b_units(bad).is_err());
        }
    }
    #[test]
    fn hevc_record_preserves_nal_length_and_bit_depth() {
        let mut c = VideoDecoderConfig::new("hvc1.2.4.L120.B0", 16, 16).expect("config");
        let mut data = vec![0; 23];
        data[0] = 1;
        data[17] = 2;
        data[21] = 3;
        data[22] = 1;
        data.extend_from_slice(&[32, 0, 1, 0, 2, 64, 1]);
        c.description = Some(data.into());
        let nal = NalInput::new(&c).expect("hvcC").expect("NAL");
        assert_eq!(nal.bit_depth, Some(10));
        assert_eq!(nal.length_size, Some(4));
        assert_eq!(nal.parameter_sets, vec![vec![64, 1]]);
        assert!(nal.annex_b(&[0, 0, 0, 0], false).is_err());
    }
    #[test]
    fn configuration_and_access_units_are_distinct() {
        let mut c = VideoDecoderConfig::new("avc1.42001E", 16, 16).expect("config");
        c.description = Some(vec![1, 66, 0, 30, 255, 225, 0, 2, 103, 1, 1, 0, 2, 104, 2].into());
        let n = NalInput::new(&c).expect("record").expect("NAL");
        assert_eq!(
            n.annex_b(&[0, 0, 0, 2, 101, 3], true)
                .expect("packet")
                .as_ref(),
            &[0, 0, 0, 1, 103, 1, 0, 0, 0, 1, 104, 2, 0, 0, 0, 1, 101, 3]
        );
        assert!(n.annex_b(&[0, 0, 0, 10, 101], false).is_err());
        for i in 0..c.description.as_ref().expect("description").len() {
            let mut truncated = c.clone();
            truncated.description = Some(c.description.as_ref().expect("description")[..i].into());
            assert!(NalInput::new(&truncated).is_err());
        }
    }
}
