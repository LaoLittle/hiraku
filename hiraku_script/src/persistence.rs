//! Portable data snapshots, independent of VM code, object IDs and symbol order.
//! Host boundaries must export heap objects before encoding. Aliases are saved
//! by value; cyclic graphs and executable/runtime resources are not data.
use crate::{
    Value,
    symbol::{SymbolId, SymbolManifest},
};
use serde::{Deserialize, Serialize};

const MAX_DEPTH: usize = 128;
const MAX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    version: u32,
    types: Vec<String>,
    value: Value,
}

/// Convert exported script data to a portable HSON data tree. This can be
/// embedded directly in a larger BHSON document without nested byte buffers.
pub fn to_value(value: &Value, symbols: &SymbolManifest) -> Result<crate::hson::HsonValue, String> {
    let mut types = Vec::new();
    let value = transform(value, "$", 0, &mut |id| {
        let name = symbols
            .resolve(id)
            .ok_or_else(|| format!("unknown type symbol {}", id.0))?;
        let index = types
            .iter()
            .position(|item| item == name)
            .unwrap_or_else(|| {
                types.push(name.to_owned());
                types.len() - 1
            });
        Ok(SymbolId(index as u32))
    })?;
    crate::hson::to_value(&Document {
        version: 1,
        types,
        value,
    })
    .map_err(|error| error.to_string())
}

/// Encode an exported script value as BHSON, never textual HSON.
pub fn encode(value: &Value, symbols: &SymbolManifest) -> Result<Vec<u8>, String> {
    crate::bhson::encode_with_limits(
        &to_value(value, symbols)?,
        crate::bhson::Limits {
            max_bytes: MAX_BYTES,
            ..Default::default()
        },
    )
    .map_err(|error| error.to_string())
}

/// Decode only BHSON. Textual HSON and other legacy formats are not accepted.
pub fn decode(bytes: &[u8], symbols: &SymbolManifest) -> Result<Value, String> {
    from_value(
        crate::bhson::parse_with_limits(
            bytes,
            crate::bhson::Limits {
                max_bytes: MAX_BYTES,
                ..Default::default()
            },
        )
        .map_err(|error| error.to_string())?,
        symbols,
    )
}

/// Resolve nominal types against the current compiler manifest. A subsequent
/// typed cast validates the object's current schema.
pub fn from_value(
    value: crate::hson::HsonValue,
    symbols: &SymbolManifest,
) -> Result<Value, String> {
    let document: Document = crate::hson::from_value(value).map_err(|error| error.to_string())?;
    if document.version != 1 {
        return Err("unsupported persistent object version".into());
    }
    transform(&document.value, "$", 0, &mut |id| {
        let name = document
            .types
            .get(id.0 as usize)
            .ok_or("invalid persistent type index")?;
        symbols
            .find(name)
            .ok_or_else(|| format!("persistent type `{name}` is not defined"))
    })
}

fn transform(
    value: &Value,
    path: &str,
    depth: usize,
    resolve: &mut impl FnMut(SymbolId) -> Result<SymbolId, String>,
) -> Result<Value, String> {
    if depth > MAX_DEPTH {
        return Err(format!(
            "{path}: persistent object nesting exceeds {MAX_DEPTH}"
        ));
    }
    let mut child = |value: &Value, suffix: String| {
        transform(value, &format!("{path}{suffix}"), depth + 1, resolve)
    };
    Ok(match value {
        Value::Unit
        | Value::Null
        | Value::Bool(_)
        | Value::Int(_)
        | Value::UInt(_)
        | Value::String(_)
        | Value::Symbol(_) => value.clone(),
        Value::Number(n) | Value::Percent(n) if n.is_finite() => value.clone(),
        Value::Optional(value) => Value::Optional(
            value
                .as_ref()
                .map(|value| child(value, "?".into()).map(Box::new))
                .transpose()?,
        ),
        Value::Tuple(values) | Value::List(values) => {
            let items = values
                .iter()
                .enumerate()
                .map(|(i, value)| child(value, format!("[{i}]")))
                .collect::<Result<Vec<_>, _>>()?;
            if matches!(value, Value::Tuple(_)) {
                Value::Tuple(items)
            } else {
                Value::List(items)
            }
        }
        Value::Map(values) => Value::Map(
            values
                .iter()
                .map(|(key, value)| Ok((key.clone(), child(value, format!(".{key}"))?)))
                .collect::<Result<_, String>>()?,
        ),
        Value::Typed { type_id, value } => {
            let type_id = resolve(*type_id).map_err(|error| format!("{path}: {error}"))?;
            Value::Typed {
                type_id,
                value: Box::new(transform(value, path, depth + 1, resolve)?),
            }
        }
        other => {
            return Err(format!(
                "{path}: {} cannot be persisted; store data instead of executable/runtime resources",
                match other {
                    Value::Object(_) => "an unexported or cyclic object",
                    Value::Function { .. } | Value::Closure { .. } => "a function or closure",
                    Value::Handle { .. } => "a native handle",
                    Value::Task(_) => "a task",
                    Value::TextTemplate(_) => "an unevaluated text template",
                    Value::Uninitialized => "an uninitialized value",
                    Value::Ellipsis => "ellipsis",
                    _ => "a non-finite number or unsupported value",
                }
            ));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::symbol::SymbolInterner;

    #[test]
    fn nominal_objects_remap_types_and_preserve_nested_data() {
        let mut old = SymbolInterner::new();
        let alice = old.intern("Alice");
        let value = Value::Typed {
            type_id: alice,
            value: Box::new(Value::Map(
                [
                    ("name".into(), Value::String("alice".into())),
                    (
                        "data".into(),
                        Value::Tuple(vec![
                            Value::Unit,
                            Value::UInt(u64::MAX),
                            Value::Int(i64::MIN),
                            Value::Optional(Some(Box::new(Value::Optional(None)))),
                            Value::List(vec![Value::Bool(true)]),
                        ]),
                    ),
                ]
                .into(),
            )),
        };
        let encoded = encode(&value, &old.manifest()).expect("encode object");
        assert!(encoded.starts_with(crate::bhson::MAGIC));
        let mut current = SymbolInterner::new();
        current.intern("Bob");
        let new_id = current.intern("Alice");
        let mut expected = value.clone();
        if let Value::Typed { type_id, .. } = &mut expected {
            *type_id = new_id;
        }
        assert_eq!(
            decode(&encoded, &current.manifest()).expect("decode object"),
            expected
        );
        assert!(
            decode(&encoded, &SymbolManifest::default())
                .expect_err("missing type")
                .contains("Alice")
        );
    }

    #[test]
    fn resources_are_rejected_at_their_data_path() {
        let value = Value::Map(
            [(
                "callback".into(),
                Value::Function {
                    module: None,
                    symbol: SymbolId(0),
                },
            )]
            .into(),
        );
        assert!(
            encode(&value, &SymbolManifest::default())
                .expect_err("function is not data")
                .contains("$.callback")
        );
        assert!(encode(&Value::Number(f64::NAN), &SymbolManifest::default()).is_err());
        assert!(decode(b"not a document", &SymbolManifest::default()).is_err());
        assert!(
            decode(
                b".{ version: 1, types: [], value: .Unit }",
                &SymbolManifest::default()
            )
            .is_err()
        );
    }
}
