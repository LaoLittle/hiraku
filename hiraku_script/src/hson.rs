//! Hiraku Script Object Notation (HSON).
//!
//! HSON reuses HKS literal syntax (`.{}` maps, `()` tuples and `.symbol`
//! symbols), but deliberately excludes calls, variables and control flow.

use std::{collections::BTreeMap, fmt, str};

use serde::{
    Deserialize, Deserializer as _, Serialize,
    de::{
        self, DeserializeOwned, DeserializeSeed, EnumAccess, IntoDeserializer, MapAccess,
        SeqAccess, VariantAccess, Visitor,
    },
    ser::{
        self, SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant, SerializeTuple,
        SerializeTupleStruct, SerializeTupleVariant,
    },
};

use crate::{Expr, ExprKind, NumberUnit, Stmt, parse_program};

pub type HsonMap = BTreeMap<String, HsonValue>;

#[derive(Clone, Debug, PartialEq)]
pub enum HsonValue {
    Null,
    Bool(bool),
    Integer(i64),
    Unsigned(u64),
    Float(f64),
    String(String),
    Array(Vec<HsonValue>),
    Map(HsonMap),
}

impl HsonValue {
    pub fn as_map(&self) -> Option<&HsonMap> {
        match self {
            Self::Map(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[HsonValue]> {
        match self {
            Self::Array(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Integer(value) => Some(*value),
            Self::Unsigned(value) => i64::try_from(*value).ok(),
            Self::Float(value) if value.is_finite() && value.fract() == 0.0 => {
                let integer = *value as i64;
                (integer as f64 == *value).then_some(integer)
            }
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Self::Integer(value) => u64::try_from(*value).ok(),
            Self::Unsigned(value) => Some(*value),
            Self::Float(value) if value.is_finite() && value.fract() == 0.0 => {
                let integer = *value as u64;
                (integer as f64 == *value).then_some(integer)
            }
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Integer(value) => Some(*value as f64),
            Self::Unsigned(value) => Some(*value as f64),
            Self::Float(value) => Some(*value),
            _ => None,
        }
    }
}

impl Serialize for HsonValue {
    fn serialize<S: ser::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Null => serializer.serialize_unit(),
            Self::Bool(value) => serializer.serialize_bool(*value),
            Self::Integer(value) => serializer.serialize_i64(*value),
            Self::Unsigned(value) => serializer.serialize_u64(*value),
            Self::Float(value) => serializer.serialize_f64(*value),
            Self::String(value) => serializer.serialize_str(value),
            Self::Array(values) => values.serialize(serializer),
            Self::Map(values) => values.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for HsonValue {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct HsonValueVisitor;

        impl<'de> Visitor<'de> for HsonValueVisitor {
            type Value = HsonValue;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an HSON-compatible value")
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(HsonValue::Null)
            }

            fn visit_none<E>(self) -> Result<Self::Value, E> {
                Ok(HsonValue::Null)
            }

            fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
                Ok(HsonValue::Bool(value))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
                Ok(HsonValue::Integer(value))
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
                Ok(HsonValue::Unsigned(value))
            }

            fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E> {
                Ok(HsonValue::Float(value))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
                Ok(HsonValue::String(value.to_string()))
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
                Ok(HsonValue::String(value))
            }

            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<Self::Value, A::Error> {
                let mut values = Vec::with_capacity(sequence.size_hint().unwrap_or(0));
                while let Some(value) = sequence.next_element()? {
                    values.push(value);
                }
                Ok(HsonValue::Array(values))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut values = HsonMap::new();
                while let Some((key, value)) = map.next_entry()? {
                    values.insert(key, value);
                }
                Ok(HsonValue::Map(values))
            }
        }

        deserializer.deserialize_any(HsonValueVisitor)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HsonError {
    message: String,
    offset: Option<usize>,
}

impl HsonError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            offset: None,
        }
    }

    fn at(message: impl Into<String>, offset: usize) -> Self {
        Self {
            message: message.into(),
            offset: Some(offset),
        }
    }
}

impl fmt::Display for HsonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.offset {
            Some(offset) => write!(formatter, "{} at byte {offset}", self.message),
            None => formatter.write_str(&self.message),
        }
    }
}

impl std::error::Error for HsonError {}

impl de::Error for HsonError {
    fn custom<T: fmt::Display>(message: T) -> Self {
        Self::new(message.to_string())
    }
}

impl ser::Error for HsonError {
    fn custom<T: fmt::Display>(message: T) -> Self {
        Self::new(message.to_string())
    }
}

pub fn parse(source: &str) -> Result<HsonValue, HsonError> {
    let program = parse_program(source).map_err(|errors| {
        HsonError::new(
            errors
                .into_iter()
                .map(|error| format!("{} at byte {}", error.message, error.span.start))
                .collect::<Vec<_>>()
                .join("; "),
        )
    })?;
    let [Stmt::Expr(expression)] = program.statements.as_slice() else {
        return Err(HsonError::new(
            "an HSON document must contain exactly one value",
        ));
    };
    literal_value(expression, source)
}

pub fn from_str<T: DeserializeOwned>(source: &str) -> Result<T, HsonError> {
    T::deserialize(parse(source)?)
}

pub fn from_slice<T: DeserializeOwned>(source: &[u8]) -> Result<T, HsonError> {
    let source = str::from_utf8(source).map_err(|error| HsonError::new(error.to_string()))?;
    from_str(source)
}

pub fn to_value<T: Serialize + ?Sized>(value: &T) -> Result<HsonValue, HsonError> {
    value.serialize(HsonSerializer)
}

pub fn from_value<T: DeserializeOwned>(value: HsonValue) -> Result<T, HsonError> {
    T::deserialize(value)
}

pub fn to_string<T: Serialize + ?Sized>(value: &T) -> Result<String, HsonError> {
    let mut output = String::new();
    write_value(&to_value(value)?, &mut output, 0)?;
    output.push('\n');
    Ok(output)
}

pub fn to_vec<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, HsonError> {
    Ok(to_string(value)?.into_bytes())
}

fn literal_value(expression: &Expr, source: &str) -> Result<HsonValue, HsonError> {
    let invalid = |message: &str| HsonError::at(message, expression.span.start);
    match &expression.kind {
        ExprKind::String(value) | ExprKind::Symbol(value) => Ok(HsonValue::String(value.clone())),
        ExprKind::Bool(value) => Ok(HsonValue::Bool(*value)),
        ExprKind::Null => Ok(HsonValue::Null),
        ExprKind::Number { value, unit } => match unit {
            NumberUnit::Scalar | NumberUnit::Percent if value.is_finite() => {
                parse_number(expression, source)
            }
            _ => Err(invalid("HSON numbers must be finite")),
        },
        ExprKind::UnaryMinus(value) => {
            let ExprKind::Number { value, .. } = value.kind else {
                return Err(invalid("unary minus is only valid for numbers"));
            };
            if value.is_finite() {
                parse_number(expression, source)
            } else {
                Err(invalid("HSON numbers must be finite"))
            }
        }
        ExprKind::Tuple(values) => values
            .iter()
            .map(|value| literal_value(value, source))
            .collect::<Result<Vec<_>, _>>()
            .map(HsonValue::Array),
        ExprKind::List(values) => values
            .iter()
            .map(|value| literal_value(value, source))
            .collect::<Result<Vec<_>, _>>()
            .map(HsonValue::Array),
        ExprKind::Map(fields) => {
            let mut map = HsonMap::new();
            for field in fields {
                if map
                    .insert(field.name.clone(), literal_value(&field.value, source)?)
                    .is_some()
                {
                    return Err(HsonError::at(
                        format!("duplicate map key `{}`", field.name),
                        field.span.start,
                    ));
                }
            }
            Ok(HsonValue::Map(map))
        }
        _ => Err(invalid(
            "HSON may contain only null, literals, symbols, tuples, and maps",
        )),
    }
}

fn parse_number(expression: &Expr, source: &str) -> Result<HsonValue, HsonError> {
    let raw = source
        .get(expression.span.start..expression.span.end)
        .ok_or_else(|| HsonError::at("invalid numeric source span", expression.span.start))?
        .trim()
        .trim_end_matches('%');
    if !raw.contains(['.', 'e', 'E']) {
        if let Ok(value) = raw.parse::<i64>() {
            return Ok(HsonValue::Integer(value));
        }
        if !raw.starts_with('-')
            && let Ok(value) = raw.parse::<u64>()
        {
            return Ok(HsonValue::Unsigned(value));
        }
    }
    raw.parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .map(HsonValue::Float)
        .ok_or_else(|| HsonError::at("invalid HSON number", expression.span.start))
}

fn write_value(value: &HsonValue, output: &mut String, depth: usize) -> Result<(), HsonError> {
    match value {
        HsonValue::Null => output.push_str("null"),
        HsonValue::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        HsonValue::Integer(value) => output.push_str(&value.to_string()),
        HsonValue::Unsigned(value) => output.push_str(&value.to_string()),
        HsonValue::Float(value) if value.is_finite() => output.push_str(&value.to_string()),
        HsonValue::Float(_) => return Err(HsonError::new("HSON numbers must be finite")),
        HsonValue::String(value) => {
            output.push('"');
            for character in value.chars() {
                match character {
                    '"' => output.push_str("\\\""),
                    '\\' => output.push_str("\\\\"),
                    '\n' => output.push_str("\\n"),
                    '\r' => output.push_str("\\r"),
                    '\t' => output.push_str("\\t"),
                    other => output.push(other),
                }
            }
            output.push('"');
        }
        HsonValue::Array(values) => {
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push_str(", ");
                }
                write_value(value, output, depth)?;
            }
            output.push(']');
        }
        HsonValue::Map(values) => {
            output.push_str(".{");
            if !values.is_empty() {
                output.push('\n');
                for (key, value) in values {
                    output.push_str(&"    ".repeat(depth + 1));
                    if is_identifier(key) {
                        output.push_str(key);
                    } else {
                        write_value(&HsonValue::String(key.clone()), output, depth + 1)?;
                    }
                    output.push_str(": ");
                    write_value(value, output, depth + 1)?;
                    output.push_str(",\n");
                }
                output.push_str(&"    ".repeat(depth));
            }
            output.push('}');
        }
    }
    Ok(())
}

fn is_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    chars.next().is_some_and(|character| {
        character == '_' || unicode_xid::UnicodeXID::is_xid_start(character)
    }) && chars.all(|character| unicode_xid::UnicodeXID::is_xid_continue(character))
}

struct HsonSerializer;

impl ser::Serializer for HsonSerializer {
    type Ok = HsonValue;
    type Error = HsonError;
    type SerializeSeq = ArraySerializer;
    type SerializeTuple = ArraySerializer;
    type SerializeTupleStruct = ArraySerializer;
    type SerializeTupleVariant = VariantArraySerializer;
    type SerializeMap = MapSerializer;
    type SerializeStruct = MapSerializer;
    type SerializeStructVariant = VariantMapSerializer;

    fn serialize_bool(self, value: bool) -> Result<Self::Ok, Self::Error> {
        Ok(HsonValue::Bool(value))
    }
    fn serialize_i8(self, value: i8) -> Result<Self::Ok, Self::Error> {
        self.serialize_i64(value.into())
    }
    fn serialize_i16(self, value: i16) -> Result<Self::Ok, Self::Error> {
        self.serialize_i64(value.into())
    }
    fn serialize_i32(self, value: i32) -> Result<Self::Ok, Self::Error> {
        self.serialize_i64(value.into())
    }
    fn serialize_i64(self, value: i64) -> Result<Self::Ok, Self::Error> {
        Ok(HsonValue::Integer(value))
    }
    fn serialize_u8(self, value: u8) -> Result<Self::Ok, Self::Error> {
        self.serialize_u64(value.into())
    }
    fn serialize_u16(self, value: u16) -> Result<Self::Ok, Self::Error> {
        self.serialize_u64(value.into())
    }
    fn serialize_u32(self, value: u32) -> Result<Self::Ok, Self::Error> {
        self.serialize_u64(value.into())
    }
    fn serialize_u64(self, value: u64) -> Result<Self::Ok, Self::Error> {
        Ok(HsonValue::Unsigned(value))
    }
    fn serialize_f32(self, value: f32) -> Result<Self::Ok, Self::Error> {
        self.serialize_f64(value.into())
    }
    fn serialize_f64(self, value: f64) -> Result<Self::Ok, Self::Error> {
        value
            .is_finite()
            .then_some(HsonValue::Float(value))
            .ok_or_else(|| HsonError::new("HSON numbers must be finite"))
    }
    fn serialize_char(self, value: char) -> Result<Self::Ok, Self::Error> {
        self.serialize_str(&value.to_string())
    }
    fn serialize_str(self, value: &str) -> Result<Self::Ok, Self::Error> {
        Ok(HsonValue::String(value.to_string()))
    }
    fn serialize_bytes(self, value: &[u8]) -> Result<Self::Ok, Self::Error> {
        Ok(HsonValue::Array(
            value
                .iter()
                .map(|value| HsonValue::Unsigned((*value).into()))
                .collect(),
        ))
    }
    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        Ok(HsonValue::Null)
    }
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<Self::Ok, Self::Error> {
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        Ok(HsonValue::Null)
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok, Self::Error> {
        self.serialize_unit()
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        self.serialize_str(variant)
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        Ok(HsonValue::Map(HsonMap::from([(
            variant.to_string(),
            value.serialize(HsonSerializer)?,
        )])))
    }
    fn serialize_seq(self, length: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        Ok(ArraySerializer(Vec::with_capacity(length.unwrap_or(0))))
    }
    fn serialize_tuple(self, length: usize) -> Result<Self::SerializeTuple, Self::Error> {
        self.serialize_seq(Some(length))
    }
    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        length: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        self.serialize_seq(Some(length))
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        length: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        Ok(VariantArraySerializer {
            variant,
            values: Vec::with_capacity(length),
        })
    }
    fn serialize_map(self, _length: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        Ok(MapSerializer::default())
    }
    fn serialize_struct(
        self,
        _name: &'static str,
        _length: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        Ok(MapSerializer::default())
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        _length: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        Ok(VariantMapSerializer {
            variant,
            map: HsonMap::new(),
        })
    }
}

struct ArraySerializer(Vec<HsonValue>);
impl SerializeSeq for ArraySerializer {
    type Ok = HsonValue;
    type Error = HsonError;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Self::Error> {
        self.0.push(value.serialize(HsonSerializer)?);
        Ok(())
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(HsonValue::Array(self.0))
    }
}
impl SerializeTuple for ArraySerializer {
    type Ok = HsonValue;
    type Error = HsonError;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Self::Error> {
        SerializeSeq::serialize_element(self, value)
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        SerializeSeq::end(self)
    }
}
impl SerializeTupleStruct for ArraySerializer {
    type Ok = HsonValue;
    type Error = HsonError;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Self::Error> {
        SerializeSeq::serialize_element(self, value)
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        SerializeSeq::end(self)
    }
}

struct VariantArraySerializer {
    variant: &'static str,
    values: Vec<HsonValue>,
}
impl SerializeTupleVariant for VariantArraySerializer {
    type Ok = HsonValue;
    type Error = HsonError;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Self::Error> {
        self.values.push(value.serialize(HsonSerializer)?);
        Ok(())
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(HsonValue::Map(HsonMap::from([(
            self.variant.to_string(),
            HsonValue::Array(self.values),
        )])))
    }
}

#[derive(Default)]
struct MapSerializer {
    map: HsonMap,
    key: Option<String>,
}
impl SerializeMap for MapSerializer {
    type Ok = HsonValue;
    type Error = HsonError;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), Self::Error> {
        self.key = Some(map_key(key)?);
        Ok(())
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Self::Error> {
        let key = self
            .key
            .take()
            .ok_or_else(|| HsonError::new("map value serialized before key"))?;
        self.map.insert(key, value.serialize(HsonSerializer)?);
        Ok(())
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(HsonValue::Map(self.map))
    }
}
impl SerializeStruct for MapSerializer {
    type Ok = HsonValue;
    type Error = HsonError;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        self.map
            .insert(key.to_string(), value.serialize(HsonSerializer)?);
        Ok(())
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(HsonValue::Map(self.map))
    }
}

struct VariantMapSerializer {
    variant: &'static str,
    map: HsonMap,
}
impl SerializeStructVariant for VariantMapSerializer {
    type Ok = HsonValue;
    type Error = HsonError;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        self.map
            .insert(key.to_string(), value.serialize(HsonSerializer)?);
        Ok(())
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(HsonValue::Map(HsonMap::from([(
            self.variant.to_string(),
            HsonValue::Map(self.map),
        )])))
    }
}

fn map_key<T: Serialize + ?Sized>(key: &T) -> Result<String, HsonError> {
    match key.serialize(HsonSerializer)? {
        HsonValue::String(value) => Ok(value),
        HsonValue::Integer(value) => Ok(value.to_string()),
        HsonValue::Unsigned(value) => Ok(value.to_string()),
        HsonValue::Bool(value) => Ok(value.to_string()),
        _ => Err(HsonError::new(
            "HSON map keys must be strings or scalar values",
        )),
    }
}

impl<'de> de::Deserializer<'de> for HsonValue {
    type Error = HsonError;
    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Self::Null => visitor.visit_unit(),
            Self::Bool(v) => visitor.visit_bool(v),
            Self::Integer(v) => visitor.visit_i64(v),
            Self::Unsigned(v) => visitor.visit_u64(v),
            Self::Float(v) => visitor.visit_f64(v),
            Self::String(v) => visitor.visit_string(v),
            Self::Array(v) => visitor.visit_seq(HsonSeqAccess(v.into_iter())),
            Self::Map(v) => visitor.visit_map(HsonMapAccess {
                values: v.into_iter(),
                value: None,
            }),
        }
    }
    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        if self == Self::Null {
            visitor.visit_none()
        } else {
            visitor.visit_some(self)
        }
    }
    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        match self {
            Self::String(variant) => visitor.visit_enum(variant.into_deserializer()),
            Self::Map(map) if map.len() == 1 => {
                let (variant, value) = map
                    .into_iter()
                    .next()
                    .expect("single-entry enum map must contain its entry");
                visitor.visit_enum(HsonEnumAccess {
                    variant,
                    value: Some(value),
                })
            }
            _ => Err(HsonError::new(
                "expected an enum string or single-entry map",
            )),
        }
    }
    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        visitor.visit_newtype_struct(self)
    }
    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Self::Bool(v) => visitor.visit_bool(v),
            _ => Err(HsonError::new("expected boolean")),
        }
    }
    fn deserialize_i8<V: Visitor<'de>>(self, v: V) -> Result<V::Value, Self::Error> {
        self.deserialize_i64(v)
    }
    fn deserialize_i16<V: Visitor<'de>>(self, v: V) -> Result<V::Value, Self::Error> {
        self.deserialize_i64(v)
    }
    fn deserialize_i32<V: Visitor<'de>>(self, v: V) -> Result<V::Value, Self::Error> {
        self.deserialize_i64(v)
    }
    fn deserialize_i64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.as_i64().map_or_else(
            || Err(HsonError::new("expected integer")),
            |v| visitor.visit_i64(v),
        )
    }
    fn deserialize_u8<V: Visitor<'de>>(self, v: V) -> Result<V::Value, Self::Error> {
        self.deserialize_u64(v)
    }
    fn deserialize_u16<V: Visitor<'de>>(self, v: V) -> Result<V::Value, Self::Error> {
        self.deserialize_u64(v)
    }
    fn deserialize_u32<V: Visitor<'de>>(self, v: V) -> Result<V::Value, Self::Error> {
        self.deserialize_u64(v)
    }
    fn deserialize_u64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.as_u64().map_or_else(
            || Err(HsonError::new("expected unsigned integer")),
            |v| visitor.visit_u64(v),
        )
    }
    fn deserialize_f32<V: Visitor<'de>>(self, v: V) -> Result<V::Value, Self::Error> {
        self.deserialize_f64(v)
    }
    fn deserialize_f64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.as_f64().map_or_else(
            || Err(HsonError::new("expected number")),
            |v| visitor.visit_f64(v),
        )
    }
    fn deserialize_char<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Self::String(v) if v.chars().count() == 1 => visitor.visit_char(
                v.chars()
                    .next()
                    .expect("one-character string must have a character"),
            ),
            _ => Err(HsonError::new("expected one-character string")),
        }
    }
    fn deserialize_str<V: Visitor<'de>>(self, v: V) -> Result<V::Value, Self::Error> {
        self.deserialize_string(v)
    }
    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Self::String(v) => visitor.visit_string(v),
            _ => Err(HsonError::new("expected string")),
        }
    }
    fn deserialize_bytes<V: Visitor<'de>>(self, v: V) -> Result<V::Value, Self::Error> {
        self.deserialize_byte_buf(v)
    }
    fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Self::Array(values) => values
                .into_iter()
                .map(|v| {
                    v.as_u64()
                        .and_then(|v| u8::try_from(v).ok())
                        .ok_or_else(|| HsonError::new("expected byte array"))
                })
                .collect::<Result<Vec<_>, _>>()
                .and_then(|v| visitor.visit_byte_buf(v)),
            _ => Err(HsonError::new("expected byte array")),
        }
    }
    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Self::Null => visitor.visit_unit(),
            _ => Err(HsonError::new("expected null")),
        }
    }
    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        v: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_unit(v)
    }
    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Self::Array(v) => visitor.visit_seq(HsonSeqAccess(v.into_iter())),
            _ => Err(HsonError::new("expected tuple")),
        }
    }
    fn deserialize_tuple<V: Visitor<'de>>(self, _: usize, v: V) -> Result<V::Value, Self::Error> {
        self.deserialize_seq(v)
    }
    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        _: usize,
        v: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_seq(v)
    }
    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Self::Map(v) => visitor.visit_map(HsonMapAccess {
                values: v.into_iter(),
                value: None,
            }),
            _ => Err(HsonError::new("expected map")),
        }
    }
    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        _: &'static [&'static str],
        v: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_map(v)
    }
    fn deserialize_identifier<V: Visitor<'de>>(self, v: V) -> Result<V::Value, Self::Error> {
        self.deserialize_string(v)
    }
    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_unit()
    }
}

struct HsonSeqAccess(std::vec::IntoIter<HsonValue>);
impl<'de> SeqAccess<'de> for HsonSeqAccess {
    type Error = HsonError;
    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, Self::Error> {
        self.0
            .next()
            .map(|value| seed.deserialize(value))
            .transpose()
    }
    fn size_hint(&self) -> Option<usize> {
        Some(self.0.len())
    }
}

struct HsonMapAccess {
    values: std::collections::btree_map::IntoIter<String, HsonValue>,
    value: Option<HsonValue>,
}
impl<'de> MapAccess<'de> for HsonMapAccess {
    type Error = HsonError;
    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, Self::Error> {
        match self.values.next() {
            Some((key, value)) => {
                self.value = Some(value);
                seed.deserialize(MapKeyDeserializer(key)).map(Some)
            }
            None => Ok(None),
        }
    }
    fn next_value_seed<V: DeserializeSeed<'de>>(
        &mut self,
        seed: V,
    ) -> Result<V::Value, Self::Error> {
        seed.deserialize(
            self.value
                .take()
                .ok_or_else(|| HsonError::new("map value requested before key"))?,
        )
    }
    fn size_hint(&self) -> Option<usize> {
        Some(self.values.len())
    }
}

struct MapKeyDeserializer(String);
impl<'de> de::Deserializer<'de> for MapKeyDeserializer {
    type Error = HsonError;
    fn deserialize_any<V: Visitor<'de>>(self, v: V) -> Result<V::Value, Self::Error> {
        v.visit_string(self.0)
    }
    fn deserialize_identifier<V: Visitor<'de>>(self, v: V) -> Result<V::Value, Self::Error> {
        v.visit_string(self.0)
    }
    fn deserialize_str<V: Visitor<'de>>(self, v: V) -> Result<V::Value, Self::Error> {
        v.visit_string(self.0)
    }
    fn deserialize_string<V: Visitor<'de>>(self, v: V) -> Result<V::Value, Self::Error> {
        v.visit_string(self.0)
    }
    fn deserialize_u32<V: Visitor<'de>>(self, v: V) -> Result<V::Value, Self::Error> {
        self.0
            .parse()
            .map_err(|error: std::num::ParseIntError| HsonError::new(error.to_string()))
            .and_then(|n| v.visit_u32(n))
    }
    fn deserialize_u64<V: Visitor<'de>>(self, v: V) -> Result<V::Value, Self::Error> {
        self.0
            .parse()
            .map_err(|error: std::num::ParseIntError| HsonError::new(error.to_string()))
            .and_then(|n| v.visit_u64(n))
    }
    fn deserialize_i64<V: Visitor<'de>>(self, v: V) -> Result<V::Value, Self::Error> {
        self.0
            .parse()
            .map_err(|error: std::num::ParseIntError| HsonError::new(error.to_string()))
            .and_then(|n| v.visit_i64(n))
    }
    serde::forward_to_deserialize_any! {bool i8 i16 i32 u8 u16 f32 f64 char bytes byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct map struct enum ignored_any}
}

struct HsonEnumAccess {
    variant: String,
    value: Option<HsonValue>,
}
impl<'de> EnumAccess<'de> for HsonEnumAccess {
    type Error = HsonError;
    type Variant = HsonVariantAccess;
    fn variant_seed<V: DeserializeSeed<'de>>(
        self,
        seed: V,
    ) -> Result<(V::Value, Self::Variant), Self::Error> {
        Ok((
            seed.deserialize(self.variant.into_deserializer())?,
            HsonVariantAccess(self.value),
        ))
    }
}
struct HsonVariantAccess(Option<HsonValue>);
impl<'de> VariantAccess<'de> for HsonVariantAccess {
    type Error = HsonError;
    fn unit_variant(self) -> Result<(), Self::Error> {
        if self.0.is_none() || self.0 == Some(HsonValue::Null) {
            Ok(())
        } else {
            Err(HsonError::new("expected unit variant"))
        }
    }
    fn newtype_variant_seed<T: DeserializeSeed<'de>>(
        self,
        seed: T,
    ) -> Result<T::Value, Self::Error> {
        seed.deserialize(
            self.0
                .ok_or_else(|| HsonError::new("expected enum value"))?,
        )
    }
    fn tuple_variant<V: Visitor<'de>>(self, _: usize, visitor: V) -> Result<V::Value, Self::Error> {
        self.0
            .ok_or_else(|| HsonError::new("expected enum tuple"))?
            .deserialize_seq(visitor)
    }
    fn struct_variant<V: Visitor<'de>>(
        self,
        _: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.0
            .ok_or_else(|| HsonError::new("expected enum map"))?
            .deserialize_map(visitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Serialize, serde::Deserialize)]
    struct Document {
        name: String,
        values: Vec<f64>,
        enabled: bool,
    }

    #[test]
    fn serde_roundtrip_uses_hson_syntax() {
        let value = Document {
            name: "Hiraku".into(),
            values: vec![1.0, 2.5],
            enabled: true,
        };
        let source = to_string(&value).expect("document should serialize");
        assert!(source.starts_with(".{"));
        assert!(source.contains("values: [1, 2.5]"));
        assert_eq!(
            from_str::<Document>(&source).expect("document should deserialize"),
            value
        );
    }

    #[test]
    fn square_brackets_are_hson_lists() {
        assert_eq!(
            parse("[1, 2, 3]").expect("list must parse"),
            HsonValue::Array(vec![
                HsonValue::Integer(1),
                HsonValue::Integer(2),
                HsonValue::Integer(3),
            ])
        );
    }

    #[test]
    fn rejects_procedural_hks() {
        assert!(parse("loadScript(\"story.hks\")").is_err());
    }

    #[test]
    fn preserves_full_width_unsigned_integers() {
        let value = u64::MAX;
        let source = to_string(&value).expect("u64 should serialize");
        assert_eq!(
            from_str::<u64>(&source).expect("u64 should deserialize"),
            value
        );
    }
}
