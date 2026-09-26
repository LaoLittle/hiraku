//! RFC 6381 AVC and ISO/IEC 14496-15 Annex E HEVC identifiers.
//! Bitstream packaging is selected by description presence, not the prefix.
use super::{CodecError, invalid};
use std::{fmt, str::FromStr};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AvcCodec {
    pub in_band: bool,
    pub profile: u8,
    pub compatibility: u8,
    pub level: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HevcCodec {
    pub in_band: bool,
    /// 0, A, B, C correspond to spaces 0..=3.
    pub profile_space: u8,
    pub profile: u8,
    /// Registry bit order (reverse of general_profile_compatibility_flags).
    pub compatibility: u32,
    pub high_tier: bool,
    pub level: u8,
    /// Network-order constraint bytes; omitted trailing bytes are zero.
    pub constraints: [u8; 6],
}

fn hex(s: &str, max: usize) -> Result<u32, CodecError> {
    if s.is_empty() || s.len() > max || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid());
    }
    u32::from_str_radix(s, 16).map_err(|_| invalid())
}
fn decimal(s: &str) -> Result<u8, CodecError> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    s.parse().map_err(|_| invalid())
}
impl FromStr for AvcCodec {
    type Err = CodecError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (prefix, value) = s.split_once('.').ok_or_else(invalid)?;
        if !matches!(prefix, "avc1" | "avc3") || value.len() != 6 {
            return Err(invalid());
        }
        let value = hex(value, 6)?;
        Ok(Self {
            in_band: prefix == "avc3",
            profile: (value >> 16) as u8,
            compatibility: (value >> 8) as u8,
            level: value as u8,
        })
    }
}
impl FromStr for HevcCodec {
    type Err = CodecError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let fields: Vec<_> = s.split('.').collect();
        if !(4..=10).contains(&fields.len()) || !matches!(fields[0], "hvc1" | "hev1") {
            return Err(invalid());
        }
        let (space, profile) = match fields[1].as_bytes().first() {
            Some(b'A'..=b'C') => (fields[1].as_bytes()[0] - b'A' + 1, &fields[1][1..]),
            _ => (0, fields[1]),
        };
        let tier = fields[3].as_bytes().first().ok_or_else(invalid)?;
        if !matches!(tier, b'L' | b'H') {
            return Err(invalid());
        }
        let mut constraints = [0; 6];
        for (dst, field) in constraints.iter_mut().zip(&fields[4..]) {
            *dst = hex(field, 2)? as u8;
        }
        let value = Self {
            in_band: fields[0] == "hev1",
            profile_space: space,
            profile: decimal(profile)?,
            compatibility: hex(fields[2], 8)?,
            high_tier: *tier == b'H',
            level: decimal(&fields[3][1..])?,
            constraints,
        };
        value.validate()?;
        Ok(value)
    }
}
impl HevcCodec {
    pub(super) fn validate(&self) -> Result<(), CodecError> {
        if self.profile_space > 3 || self.profile > 31 {
            return Err(invalid());
        }
        Ok(())
    }
}
impl fmt::Display for AvcCodec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}.{:02X}{:02X}{:02X}",
            if self.in_band { "avc3" } else { "avc1" },
            self.profile,
            self.compatibility,
            self.level
        )
    }
}
impl fmt::Display for HevcCodec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.", if self.in_band { "hev1" } else { "hvc1" })?;
        if self.profile_space != 0 {
            write!(
                f,
                "{}",
                char::from(b'A' + self.profile_space.saturating_sub(1))
            )?;
        }
        write!(
            f,
            "{}.{:X}.{}{}",
            self.profile,
            self.compatibility,
            if self.high_tier { "H" } else { "L" },
            self.level
        )?;
        let count = self
            .constraints
            .iter()
            .rposition(|b| *b != 0)
            .map_or(0, |i| i + 1);
        for b in &self.constraints[..count] {
            write!(f, ".{b:02X}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::Codec;
    #[test]
    fn registry_identifiers_round_trip() {
        for s in [
            "avc1.640028",
            "avc3.42E01E",
            "hvc1.1.6.L93.B0",
            "hev1.A2.4.H153.B0.01",
            "hvc1.2.4.L120",
        ] {
            let codec: Codec = s.parse().expect("registry identifier");
            assert_eq!(
                codec.to_string().parse::<Codec>().expect("round trip"),
                codec
            );
        }
        for s in [
            "h264",
            "hevc",
            "avc1.123",
            "avc1.💥12",
            "hev1.1.6.X93",
            "hvc1.32.6.L93",
            "hvc1.1.6.L93.100",
            "hvc1.1.6.L93.00.00.00.00.00.00.00",
        ] {
            assert!(s.parse::<Codec>().is_err(), "{s}");
        }
    }
}
