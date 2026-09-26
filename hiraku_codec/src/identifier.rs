//! Parsed registry identifiers. Syntax/profile validation is independent of
//! adapter capability; a valid HDR/profile-2 identifier is not a support claim.
use crate::CodecError;
use std::{fmt, str::FromStr};
mod nal;
pub use nal::*;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Codec {
    Avc(AvcCodec),
    Hevc(HevcCodec),
    Av1(Av1Codec),
    Vp9(Vp9Codec),
    Vp8,
    Opus,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Av1Tier {
    Main,
    High,
}
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Av1Codec {
    pub profile: u8,
    pub level: u8,
    pub tier: Av1Tier,
    pub bit_depth: u8,
    pub color: Option<Av1Color>,
}
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Av1Color {
    pub monochrome: bool,
    pub subsampling_x: bool,
    pub subsampling_y: bool,
    pub chroma_sample_position: u8,
    pub primaries: u8,
    pub transfer: u8,
    pub matrix: u8,
    pub full_range: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Vp9Codec {
    pub profile: u8,
    pub level: u8,
    pub bit_depth: u8,
    pub color: Option<Vp9Color>,
}
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Vp9Color {
    pub chroma_subsampling: u8,
    pub primaries: u8,
    pub transfer: u8,
    pub matrix: u8,
    pub full_range: bool,
}
fn invalid() -> CodecError {
    CodecError::Configuration("invalid codec registry identifier or profile constraints".into())
}
fn number(value: &str, width: usize) -> Result<u8, CodecError> {
    if value.len() != width || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    value.parse().map_err(|_| invalid())
}
fn boolean(value: &str, width: usize) -> Result<bool, CodecError> {
    match number(value, width)? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(invalid()),
    }
}
fn color_valid(p: u8, t: u8, m: u8) -> bool {
    matches!(p, 1 | 2 | 4..=12 | 22) && matches!(t, 1 | 2 | 4..=18) && matches!(m, 0..=2 | 4..=14)
}
impl Codec {
    pub fn bit_depth(&self) -> Option<u8> {
        match self {
            Self::Av1(a) => Some(a.bit_depth),
            Self::Vp9(v) => Some(v.bit_depth),
            Self::Vp8 => Some(8),
            Self::Opus => None,
            // AVC/HEVC strings do not uniquely specify coded sample depth.
            Self::Avc(_) | Self::Hevc(_) => None,
        }
    }
    pub fn family(&self) -> &'static str {
        match self {
            Self::Av1(_) => "av01",
            Self::Vp9(_) => "vp09",
            Self::Vp8 => "vp8",
            Self::Opus => "opus",
            Self::Avc(a) => {
                if a.in_band {
                    "avc3"
                } else {
                    "avc1"
                }
            }
            Self::Hevc(h) => {
                if h.in_band {
                    "hev1"
                } else {
                    "hvc1"
                }
            }
        }
    }
    pub fn validate(&self) -> Result<(), CodecError> {
        match self {
            Self::Avc(_) => {}
            Self::Hevc(h) => h.validate()?,
            Self::Av1(a) => {
                if a.profile > 2
                    || a.level > 23
                    || (a.tier == Av1Tier::High && a.level < 8)
                    || !matches!(a.bit_depth, 8 | 10 | 12)
                    || (a.profile < 2 && a.bit_depth == 12)
                {
                    return Err(invalid());
                }
                if let Some(c) = &a.color {
                    if !color_valid(c.primaries, c.transfer, c.matrix)
                        || c.chroma_sample_position > 2
                        || (!c.subsampling_x && c.subsampling_y)
                        || ((!c.subsampling_x || !c.subsampling_y) && c.chroma_sample_position != 0)
                        || (c.monochrome
                            && (a.profile == 1
                                || !c.subsampling_x
                                || !c.subsampling_y
                                || c.chroma_sample_position != 0))
                        || (!c.monochrome
                            && match a.profile {
                                0 => !c.subsampling_x || !c.subsampling_y,
                                1 => c.subsampling_x || c.subsampling_y,
                                _ => a.bit_depth < 12 && (!c.subsampling_x || c.subsampling_y),
                            })
                        || (c.matrix == 0 && (c.monochrome || c.subsampling_x || c.subsampling_y))
                    {
                        return Err(invalid());
                    }
                }
            }
            Self::Vp9(v) => {
                if v.profile > 3
                    || !matches!(
                        v.level,
                        10 | 11 | 20 | 21 | 30 | 31 | 40 | 41 | 50 | 51 | 52 | 60 | 61 | 62
                    )
                    || (v.profile < 2 && v.bit_depth != 8)
                    || (v.profile >= 2 && !matches!(v.bit_depth, 10 | 12))
                {
                    return Err(invalid());
                }
                if let Some(c) = &v.color {
                    if c.chroma_subsampling > 3
                        || !color_valid(c.primaries, c.transfer, c.matrix)
                        || (v.profile % 2 == 0 && c.chroma_subsampling > 1)
                        || (v.profile % 2 == 1 && c.chroma_subsampling < 2)
                        || (c.matrix == 0 && c.chroma_subsampling != 3)
                    {
                        return Err(invalid());
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}
impl FromStr for Codec {
    type Err = CodecError;
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let f: Vec<_> = input.split('.').collect();
        let value = match f[0] {
            "avc1" | "avc3" => Self::Avc(input.parse()?),
            "hvc1" | "hev1" => Self::Hevc(input.parse()?),
            "opus" if f.len() == 1 => Self::Opus,
            "vp8" if f.len() == 1 => Self::Vp8,
            "av01" if matches!(f.len(), 4 | 10) => {
                if f[2].len() != 3 || !f[2].is_ascii() {
                    return Err(invalid());
                }
                Self::Av1(Av1Codec {
                    profile: number(f[1], 1)?,
                    level: number(&f[2][..2], 2)?,
                    tier: match &f[2][2..] {
                        "M" => Av1Tier::Main,
                        "H" => Av1Tier::High,
                        _ => return Err(invalid()),
                    },
                    bit_depth: number(f[3], 2)?,
                    color: if f.len() == 10 {
                        if f[5].len() != 3 || !f[5].is_ascii() {
                            return Err(invalid());
                        }
                        Some(Av1Color {
                            monochrome: boolean(f[4], 1)?,
                            subsampling_x: boolean(&f[5][..1], 1)?,
                            subsampling_y: boolean(&f[5][1..2], 1)?,
                            chroma_sample_position: number(&f[5][2..], 1)?,
                            primaries: number(f[6], 2)?,
                            transfer: number(f[7], 2)?,
                            matrix: number(f[8], 2)?,
                            full_range: boolean(f[9], 1)?,
                        })
                    } else {
                        None
                    },
                })
            }
            "vp09" if matches!(f.len(), 4 | 9) => Self::Vp9(Vp9Codec {
                profile: number(f[1], 2)?,
                level: number(f[2], 2)?,
                bit_depth: number(f[3], 2)?,
                color: if f.len() == 9 {
                    Some(Vp9Color {
                        chroma_subsampling: number(f[4], 2)?,
                        primaries: number(f[5], 2)?,
                        transfer: number(f[6], 2)?,
                        matrix: number(f[7], 2)?,
                        full_range: boolean(f[8], 2)?,
                    })
                } else {
                    None
                },
            }),
            "av01" | "vp09" | "vp8" | "opus" => return Err(invalid()),
            _ if input.is_empty() || input.chars().any(char::is_whitespace) => {
                return Err(invalid());
            }
            _ => return Err(CodecError::Unsupported(input.into())),
        };
        value.validate()?;
        Ok(value)
    }
}
impl TryFrom<&str> for Codec {
    type Error = CodecError;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        value.parse()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn av1_profiles_and_hdr_round_trip() {
        for input in [
            "av01.0.08M.08",
            "av01.0.08M.10.0.110.09.16.09.0",
            "av01.1.08M.10.0.000.09.18.09.1",
            "av01.2.08M.10.0.100.01.01.01.0",
            "av01.2.08M.12.0.000.01.13.00.1",
        ] {
            let codec: Codec = input.parse().expect("valid AV1 identifier");
            assert_eq!(codec.to_string(), input);
        }
    }

    #[test]
    fn malformed_and_unsupported_identifiers_are_errors() {
        for input in [
            "",
            "future-codec",
            "av01.0.00M.12",
            "av01.3.08M.10",
            "av01.0.00H.08",
            "av01.0.08M.10.0",
            "av01.1.08M.10.0.110.01.01.01.0",
            "vp09.00.10.10",
            "av01.0.éM.08",
        ] {
            assert!(input.parse::<Codec>().is_err(), "{input}");
        }
    }

    #[test]
    fn config_accepts_string_or_enum() {
        let parsed: Codec = "av01.0.08M.10".parse().expect("valid identifier");
        let text = crate::VideoDecoderConfig::new("av01.0.08M.10", 64, 64).expect("string config");
        let typed = crate::VideoDecoderConfig::new(parsed.clone(), 64, 64).expect("enum config");
        assert_eq!(text.codec, parsed);
        assert_eq!(typed.codec, parsed);
    }
}
impl TryFrom<String> for Codec {
    type Error = CodecError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}
impl From<std::convert::Infallible> for CodecError {
    fn from(value: std::convert::Infallible) -> Self {
        match value {}
    }
}
impl fmt::Display for Codec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Avc(a) => a.fmt(f),
            Self::Hevc(h) => h.fmt(f),
            Self::Opus => write!(f, "opus"),
            Self::Vp8 => write!(f, "vp8"),
            Self::Av1(a) => {
                write!(
                    f,
                    "av01.{}.{:02}{}.{:02}",
                    a.profile,
                    a.level,
                    if a.tier == Av1Tier::Main { "M" } else { "H" },
                    a.bit_depth
                )?;
                if let Some(c) = &a.color {
                    write!(
                        f,
                        ".{}.{}{}{}.{:02}.{:02}.{:02}.{}",
                        u8::from(c.monochrome),
                        u8::from(c.subsampling_x),
                        u8::from(c.subsampling_y),
                        c.chroma_sample_position,
                        c.primaries,
                        c.transfer,
                        c.matrix,
                        u8::from(c.full_range)
                    )?;
                }
                Ok(())
            }
            Self::Vp9(v) => {
                write!(f, "vp09.{:02}.{:02}.{:02}", v.profile, v.level, v.bit_depth)?;
                if let Some(c) = &v.color {
                    write!(
                        f,
                        ".{:02}.{:02}.{:02}.{:02}.{:02}",
                        c.chroma_subsampling,
                        c.primaries,
                        c.transfer,
                        c.matrix,
                        u8::from(c.full_range)
                    )?;
                }
                Ok(())
            }
        }
    }
}
