//! VM-only value representation. Never serialize these bits or accept them from
//! the embedding: NaN payloads are private tags, not portable object addresses.
use crate::{ObjectId, Value};

const QNAN: u64 = 0x7ff8_0000_0000_0000;
const PREFIX: u64 = 0xfff8_0000_0000_0000;
const TAG_MASK: u64 = 0xffff_ffff_0000_0000;
const UNIT: u64 = PREFIX;
const NULL: u64 = PREFIX | (1 << 32);
const UNINITIALIZED: u64 = PREFIX | (2 << 32);
const ELLIPSIS: u64 = PREFIX | (3 << 32);
const BOOL: u64 = PREFIX | (4 << 32);
const OBJECT: u64 = PREFIX | (5 << 32);
const NONE: u64 = PREFIX | (6 << 32);
const BOXED: u64 = PREFIX | (7 << 32);
const INT: u64 = PREFIX | (8 << 32);
const UINT: u64 = PREFIX | (9 << 32);

/// Every scalar register occupies one machine word on native and wasm targets.
/// Negative quiet NaNs are reserved; floating NaNs use the positive quiet NaN.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Slot(u64);

impl Slot {
    pub const EMPTY: Self = Self(UNINITIALIZED);

    pub(crate) fn tagged(tag: u16, payload: u32) -> Self {
        Self(PREFIX | (u64::from(tag) << 32) | u64::from(payload))
    }
    pub(crate) fn tag(self) -> Option<u16> {
        (self.0 >> 48 == PREFIX >> 48).then_some((self.0 >> 32) as u16)
    }
    pub(crate) fn payload(self) -> u32 {
        self.0 as u32
    }

    #[inline]
    pub fn immediate(value: &Value) -> Option<Self> {
        Some(Self(match value {
            Value::Number(value) => {
                if value.is_nan() {
                    QNAN
                } else {
                    value.to_bits()
                }
            }
            Value::Int(value) => INT | u64::from(i32::try_from(*value).ok()? as u32),
            Value::UInt(value) => UINT | u64::from(u32::try_from(*value).ok()?),
            Value::Unit => UNIT,
            Value::Null => NULL,
            Value::Uninitialized => UNINITIALIZED,
            Value::Ellipsis => ELLIPSIS,
            Value::Bool(value) => BOOL | u64::from(*value),
            Value::Object(ObjectId(id)) => OBJECT | u64::from(*id),
            Value::Optional(None) => NONE,
            _ => return None,
        }))
    }

    /// Operate without materializing the 72-byte embedding enum. Wide integer
    /// results and nonnumeric operands fall back to the cold path.
    #[inline(always)]
    pub fn binary(self, op: crate::BinaryOp, other: Self) -> Result<Option<Self>, crate::VmError> {
        use crate::{BinaryOp as B, VmError};
        macro_rules! calculate {
            ($a:expr, $b:expr, $tag:expr, $small:ty) => {{
                let (a, b) = ($a, $b);
                let predicate = match op {
                    B::Equal => Some(a == b),
                    B::NotEqual => Some(a != b),
                    B::Less => Some(a < b),
                    B::LessEqual => Some(a <= b),
                    B::Greater => Some(a > b),
                    B::GreaterEqual => Some(a >= b),
                    _ => None,
                };
                if let Some(value) = predicate {
                    return Ok(Some(Self(BOOL | u64::from(value))));
                }
                let value = match op {
                    B::Add => a.checked_add(b),
                    B::Subtract => a.checked_sub(b),
                    B::Multiply => a.checked_mul(b),
                    B::Divide => {
                        if b == 0 {
                            return Err(VmError::DivisionByZero);
                        }
                        a.checked_div(b)
                    }
                    _ => return Ok(None),
                }
                .ok_or(VmError::IntegerOverflow)?;
                return Ok(<$small>::try_from(value)
                    .ok()
                    .map(|value| Self($tag | u64::from(value as u32))));
            }};
        }
        let a = self.0 & TAG_MASK;
        let b = other.0 & TAG_MASK;
        if a == INT && b == INT {
            calculate!(
                self.0 as u32 as i32 as i64,
                other.0 as u32 as i32 as i64,
                INT,
                i32
            );
        }
        if a == UINT && b == UINT {
            calculate!(self.0 as u32 as u64, other.0 as u32 as u64, UINT, u32);
        }
        if self.0 >> 48 != PREFIX >> 48 && other.0 >> 48 != PREFIX >> 48 {
            let (a, b) = (f64::from_bits(self.0), f64::from_bits(other.0));
            let predicate = match op {
                B::Equal => Some(a == b),
                B::NotEqual => Some(a != b),
                B::Less => Some(a < b),
                B::LessEqual => Some(a <= b),
                B::Greater => Some(a > b),
                B::GreaterEqual => Some(a >= b),
                _ => None,
            };
            if let Some(value) = predicate {
                return Ok(Some(Self(BOOL | u64::from(value))));
            }
            let value = match op {
                B::Add => a + b,
                B::Subtract => a - b,
                B::Multiply => a * b,
                B::Divide => {
                    if b == 0.0 {
                        return Err(VmError::DivisionByZero);
                    }
                    a / b
                }
                _ => return Ok(None),
            };
            return Ok(Some(Self(if value.is_nan() {
                QNAN
            } else {
                value.to_bits()
            })));
        }
        Ok(None)
    }

    #[inline]
    pub fn decode_immediate(self) -> Value {
        match self.0 & TAG_MASK {
            UNIT => Value::Unit,
            NULL => Value::Null,
            UNINITIALIZED => Value::Uninitialized,
            ELLIPSIS => Value::Ellipsis,
            BOOL => Value::Bool(self.0 as u32 != 0),
            OBJECT => Value::Object(ObjectId(self.0 as u32)),
            NONE => Value::Optional(None),
            BOXED => unreachable!("heap values must be decoded by their owner"),
            INT => Value::Int(self.0 as u32 as i32 as i64),
            UINT => Value::UInt(self.0 as u32 as u64),
            _ => Value::Number(f64::from_bits(self.0)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_are_one_word_and_nan_payloads_cannot_forge_tags() {
        assert_eq!(size_of::<Slot>(), 8);
        for bits in [QNAN, PREFIX, u64::MAX, 0x7ff0_0000_0000_0001] {
            let slot = Slot::immediate(&Value::Number(f64::from_bits(bits))).expect("float");
            assert_eq!(slot.0, QNAN);
            assert!(matches!(slot.decode_immediate(), Value::Number(n) if n.is_nan()));
        }
        for n in [
            0.0,
            -0.0,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::MIN_POSITIVE,
            1.5,
        ] {
            let Value::Number(decoded) = Slot::immediate(&Value::Number(n))
                .expect("float")
                .decode_immediate()
            else {
                panic!("float")
            };
            assert_eq!(n.to_bits(), decoded.to_bits());
        }
    }
}
