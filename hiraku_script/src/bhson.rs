//! Binary Hiraku Script Object Notation, sharing HSON's data model and serde.
//!
//! Version 1: `BHSON\0`, followed by a little-endian u16 version (1), then one
//! tagged value. Tags: 0=null, 1=false, 2=true, 3=zigzag i64, 4=u64, 5=f64 LE,
//! 6=UTF-8 string, 7=array, 8=map. Integers, lengths and element counts use
//! canonical unsigned LEB128. Map keys are untagged length-prefixed strings,
//! strictly ascending by Rust string order. Arrays represent both serde lists
//! and tuples, just as in HSON. No executable code or platform addresses occur
//! in the wire format. Trailing bytes, unknown versions/tags, non-finite floats
//! and non-canonical encodings are errors.
use crate::hson::{self, HsonValue};
use serde::{Serialize, de::DeserializeOwned};
use std::{collections::BTreeMap, fmt};

pub const MAGIC: &[u8; 6] = b"BHSON\0";
pub const VERSION: u16 = 1;
const HEADER_LEN: usize = 8;

/// Budgets are checked before allocating from untrusted length/count fields.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_bytes: usize,
    pub max_depth: usize,
    pub max_values: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_bytes: 64 * 1024 * 1024,
            max_depth: 128,
            max_values: 1_000_000,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BhsonError {
    pub offset: usize,
    pub message: String,
}

impl fmt::Display for BhsonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "BHSON at byte {}: {}", self.offset, self.message)
    }
}
impl std::error::Error for BhsonError {}

fn error(offset: usize, message: impl Into<String>) -> BhsonError {
    BhsonError {
        offset,
        message: message.into(),
    }
}

pub fn to_vec<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, BhsonError> {
    encode(&hson::to_value(value).map_err(|cause| error(0, cause.to_string()))?)
}

pub fn from_slice<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, BhsonError> {
    hson::from_value(parse(bytes)?).map_err(|cause| error(HEADER_LEN, cause.to_string()))
}

pub fn encode(value: &HsonValue) -> Result<Vec<u8>, BhsonError> {
    encode_with_limits(value, Limits::default())
}

pub fn encode_with_limits(value: &HsonValue, limits: Limits) -> Result<Vec<u8>, BhsonError> {
    let mut encoder = Encoder {
        bytes: Vec::new(),
        limits,
        values: 0,
    };
    encoder.put(MAGIC)?;
    encoder.put(&VERSION.to_le_bytes())?;
    encoder.value(value, 0)?;
    Ok(encoder.bytes)
}

pub fn parse(bytes: &[u8]) -> Result<HsonValue, BhsonError> {
    parse_with_limits(bytes, Limits::default())
}

pub fn parse_with_limits(bytes: &[u8], limits: Limits) -> Result<HsonValue, BhsonError> {
    if bytes.len() > limits.max_bytes {
        return Err(error(0, "document exceeds byte budget"));
    }
    let mut decoder = Decoder {
        bytes,
        offset: 0,
        limits,
        values: 0,
    };
    if decoder.take(MAGIC.len())? != MAGIC {
        return Err(error(0, "incorrect magic"));
    }
    let version = decoder.take(2)?;
    if u16::from_le_bytes([version[0], version[1]]) != VERSION {
        return Err(error(MAGIC.len(), "unsupported format version"));
    }
    let value = decoder.value(0)?;
    if decoder.offset != bytes.len() {
        return Err(error(decoder.offset, "trailing data"));
    }
    Ok(value)
}

struct Encoder {
    bytes: Vec<u8>,
    limits: Limits,
    values: usize,
}
impl Encoder {
    fn put(&mut self, bytes: &[u8]) -> Result<(), BhsonError> {
        if bytes.len() > self.limits.max_bytes.saturating_sub(self.bytes.len()) {
            return Err(error(self.bytes.len(), "document exceeds byte budget"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }
    fn uint(&mut self, mut value: u64) -> Result<(), BhsonError> {
        while value >= 128 {
            self.put(&[(value as u8 & 127) | 128])?;
            value >>= 7;
        }
        self.put(&[value as u8])
    }
    fn string(&mut self, value: &str) -> Result<(), BhsonError> {
        self.uint(value.len() as u64)?;
        self.put(value.as_bytes())
    }
    fn value(&mut self, value: &HsonValue, depth: usize) -> Result<(), BhsonError> {
        if depth > self.limits.max_depth {
            return Err(error(self.bytes.len(), "nesting exceeds depth budget"));
        }
        if self.values >= self.limits.max_values {
            return Err(error(self.bytes.len(), "document exceeds value budget"));
        }
        self.values += 1;
        match value {
            HsonValue::Null => self.put(&[0]),
            HsonValue::Bool(false) => self.put(&[1]),
            HsonValue::Bool(true) => self.put(&[2]),
            HsonValue::Integer(value) => {
                self.put(&[3])?;
                self.uint(((*value as u64) << 1) ^ ((*value >> 63) as u64))
            }
            HsonValue::Unsigned(value) => {
                self.put(&[4])?;
                self.uint(*value)
            }
            HsonValue::Float(value) => {
                if !value.is_finite() {
                    return Err(error(self.bytes.len(), "non-finite float"));
                }
                self.put(&[5])?;
                self.put(&value.to_le_bytes())
            }
            HsonValue::String(value) => {
                self.put(&[6])?;
                self.string(value)
            }
            HsonValue::Array(values) => {
                self.put(&[7])?;
                self.uint(values.len() as u64)?;
                for value in values {
                    self.value(value, depth + 1)?;
                }
                Ok(())
            }
            HsonValue::Map(values) => {
                self.put(&[8])?;
                self.uint(values.len() as u64)?;
                for (key, value) in values {
                    self.string(key)?;
                    self.value(value, depth + 1)?;
                }
                Ok(())
            }
        }
    }
}

struct Decoder<'a> {
    bytes: &'a [u8],
    offset: usize,
    limits: Limits,
    values: usize,
}
impl<'a> Decoder<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], BhsonError> {
        let end = self
            .offset
            .checked_add(count)
            .ok_or_else(|| error(self.offset, "length overflow"))?;
        let slice = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| error(self.offset, "truncated input"))?;
        self.offset = end;
        Ok(slice)
    }
    fn uint(&mut self) -> Result<u64, BhsonError> {
        let start = self.offset;
        let mut value = 0;
        for index in 0..10 {
            let byte = self.take(1)?[0];
            if index == 9 && byte > 1 {
                return Err(error(start, "integer overflow"));
            }
            value |= u64::from(byte & 127) << (index * 7);
            if byte & 128 == 0 {
                if index > 0 && byte == 0 {
                    return Err(error(start, "non-canonical integer"));
                }
                return Ok(value);
            }
        }
        Err(error(start, "integer overflow"))
    }
    fn length(&mut self) -> Result<usize, BhsonError> {
        let value = self.uint()?;
        usize::try_from(value).map_err(|_| error(self.offset, "length exceeds platform capacity"))
    }
    fn string(&mut self) -> Result<String, BhsonError> {
        let length = self.length()?;
        let start = self.offset;
        let bytes = self.take(length)?;
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| error(start, "invalid UTF-8"))
    }
    fn count(&mut self, minimum_bytes: usize) -> Result<usize, BhsonError> {
        let count = self.length()?;
        if count > self.limits.max_values.saturating_sub(self.values) {
            return Err(error(self.offset, "document exceeds value budget"));
        }
        if count > (self.bytes.len() - self.offset) / minimum_bytes {
            return Err(error(self.offset, "truncated collection"));
        }
        Ok(count)
    }
    fn value(&mut self, depth: usize) -> Result<HsonValue, BhsonError> {
        if depth > self.limits.max_depth {
            return Err(error(self.offset, "nesting exceeds depth budget"));
        }
        if self.values >= self.limits.max_values {
            return Err(error(self.offset, "document exceeds value budget"));
        }
        self.values += 1;
        let tag_offset = self.offset;
        Ok(match self.take(1)?[0] {
            0 => HsonValue::Null,
            1 => HsonValue::Bool(false),
            2 => HsonValue::Bool(true),
            3 => {
                let bits = self.uint()?;
                HsonValue::Integer(((bits >> 1) as i64) ^ -((bits & 1) as i64))
            }
            4 => HsonValue::Unsigned(self.uint()?),
            5 => {
                let mut bits = [0; 8];
                bits.copy_from_slice(self.take(8)?);
                let value = f64::from_le_bytes(bits);
                if !value.is_finite() {
                    return Err(error(tag_offset, "non-finite float"));
                }
                HsonValue::Float(value)
            }
            6 => HsonValue::String(self.string()?),
            7 => {
                let count = self.count(1)?;
                // Do not reserve from attacker-controlled counts: grow only
                // after successfully decoding each bounded element.
                let mut values = Vec::new();
                for _ in 0..count {
                    values.push(self.value(depth + 1)?);
                }
                HsonValue::Array(values)
            }
            8 => {
                let count = self.count(2)?;
                let mut values = BTreeMap::new();
                for _ in 0..count {
                    let key = self.string()?;
                    if values
                        .last_key_value()
                        .is_some_and(|(last, _)| last >= &key)
                    {
                        return Err(error(self.offset, "map keys must be unique and sorted"));
                    }
                    values.insert(key, self.value(depth + 1)?);
                }
                HsonValue::Map(values)
            }
            _ => return Err(error(tag_offset, "unknown value tag")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    fn document(body: &[u8]) -> Vec<u8> {
        [MAGIC.as_slice(), &VERSION.to_le_bytes(), body].concat()
    }

    #[test]
    fn wire_format_is_stable_and_preserves_numeric_kinds() {
        assert_eq!(
            encode(&HsonValue::Integer(-1)).expect("encode"),
            document(&[3, 1])
        );
        assert_eq!(
            encode(&HsonValue::String("alice".into())).expect("encode"),
            document(b"\x06\x05alice")
        );
        for value in [
            HsonValue::Null,
            HsonValue::Bool(true),
            HsonValue::Bool(false),
            HsonValue::Integer(i64::MIN),
            HsonValue::Integer(i64::MAX),
            HsonValue::Unsigned(u64::MAX),
            HsonValue::Unsigned(0),
            HsonValue::Float(-0.0),
            HsonValue::Float(f64::MAX),
            HsonValue::String("alice\0λ🦀".into()),
        ] {
            let encoded = encode(&value).expect("encode value");
            let decoded = parse(&encoded).expect("decode value");
            assert_eq!(decoded, value);
            if let HsonValue::Float(number) = decoded {
                if let HsonValue::Float(original) = value {
                    assert_eq!(number.to_bits(), original.to_bits());
                }
            }
        }
    }

    #[test]
    fn varint_boundaries_roundtrip_without_integer_precision_loss() {
        for shift in 0..64 {
            let boundary = 1_u64 << shift;
            for bits in [
                boundary.saturating_sub(1),
                boundary,
                boundary.saturating_add(1),
            ] {
                for value in [
                    HsonValue::Unsigned(bits),
                    HsonValue::Integer(bits as i64),
                    HsonValue::Integer(!(bits as i64)),
                ] {
                    assert_eq!(
                        parse(&encode(&value).expect("encode integer")).expect("decode integer"),
                        value
                    );
                }
            }
        }
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    enum Mood {
        Happy(u64),
        Sad,
    }
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Alice {
        name: String,
        scores: Vec<i64>,
        mood: Mood,
        optional: Option<String>,
        tuple: (bool, f64),
    }

    #[test]
    fn serde_roundtrip_and_map_order_are_deterministic() {
        let value = Alice {
            name: "alice".into(),
            scores: vec![i64::MIN, 0, i64::MAX],
            mood: Mood::Happy(u64::MAX),
            optional: None,
            tuple: (true, 1.25),
        };
        let bytes = to_vec(&value).expect("serde encode");
        assert_eq!(from_slice::<Alice>(&bytes).expect("serde decode"), value);
        let a: BTreeMap<_, _> = [("bob".to_owned(), 2), ("alice".to_owned(), 1)].into();
        let b: BTreeMap<_, _> = [("alice".to_owned(), 1), ("bob".to_owned(), 2)].into();
        assert_eq!(to_vec(&a).expect("map"), to_vec(&b).expect("map"));
        for end in 0..bytes.len() {
            assert!(
                from_slice::<Alice>(&bytes[..end]).is_err(),
                "truncated at {end}"
            );
        }
    }

    #[test]
    fn malformed_and_noncanonical_inputs_are_errors() {
        for body in [
            vec![255],
            vec![0, 0],
            vec![3, 128, 0],
            vec![4, 255, 255, 255, 255, 255, 255, 255, 255, 255, 2],
            vec![6, 1, 255],
            vec![7, 127],
            vec![8, 2, 1, b'a', 0, 1, b'a', 0],
            vec![8, 2, 1, b'b', 0, 1, b'a', 0],
        ] {
            assert!(parse(&document(&body)).is_err(), "bad body {body:?}");
        }
        assert!(parse(b".{ old: true }").is_err());
        let mut version = document(&[0]);
        version[6] = 2;
        assert!(parse(&version).is_err());
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(encode(&HsonValue::Float(value)).is_err());
            assert!(parse(&document(&[&[5][..], &value.to_le_bytes()].concat())).is_err());
        }
    }

    #[test]
    fn resource_budgets_apply_symmetrically() {
        let value = HsonValue::Array(vec![HsonValue::Array(vec![HsonValue::Null])]);
        let bytes = encode(&value).expect("nested value");
        for limits in [
            Limits {
                max_depth: 1,
                ..Limits::default()
            },
            Limits {
                max_values: 2,
                ..Limits::default()
            },
            Limits {
                max_bytes: 8,
                ..Limits::default()
            },
        ] {
            assert!(encode_with_limits(&value, limits).is_err());
            assert!(parse_with_limits(&bytes, limits).is_err());
        }
    }
}
