//! Execution-owned object storage. Values copy IDs, never object contents.
//! No locks or Rust pointers enter bytecode or snapshots.
use crate::Value;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ObjectId(pub u32);

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ObjectHeap {
    #[serde(with = "object_table")]
    objects: BTreeMap<ObjectId, Value>,
    // IDs are never reused, including after collection or snapshot restore.
    next_id: u32,
    #[serde(skip)]
    last_collection: u32,
}

mod object_table {
    use super::*;
    pub fn serialize<S: serde::Serializer>(
        objects: &BTreeMap<ObjectId, Value>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        objects.iter().collect::<Vec<_>>().serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<BTreeMap<ObjectId, Value>, D::Error> {
        let entries = Vec::<(ObjectId, Value)>::deserialize(deserializer)?;
        let count = entries.len();
        let objects: BTreeMap<_, _> = entries.into_iter().collect();
        if objects.len() != count {
            return Err(serde::de::Error::custom(
                "duplicate object ID in heap snapshot",
            ));
        }
        Ok(objects)
    }
}

impl ObjectHeap {
    /// Apply an owned host update without breaking aliases to an existing record.
    pub fn update(&mut self, old: &Value, new: Value) -> Result<Value, crate::VmError> {
        let Value::Object(id) = old else {
            return Ok(self.import(new));
        };
        if matches!(new, Value::Object(_)) {
            return Ok(new);
        }
        let previous = self.get(*id)?.clone();
        let merge =
            |heap: &mut Self, old: BTreeMap<String, Value>, new: BTreeMap<String, Value>| {
                new.into_iter()
                    .map(|(key, value)| {
                        let value = match old.get(&key) {
                            Some(old) => heap.update(old, value)?,
                            None => heap.import(value),
                        };
                        Ok((key, value))
                    })
                    .collect::<Result<BTreeMap<_, _>, crate::VmError>>()
            };
        let replacement = match (previous, new) {
            (Value::Map(old), Value::Map(new)) => Value::Map(merge(self, old, new)?),
            (
                Value::Typed {
                    type_id: old_type,
                    value: old,
                },
                Value::Typed {
                    type_id,
                    value: new,
                },
            ) if old_type == type_id => match (*old, *new) {
                (Value::Map(old), Value::Map(new)) => Value::Typed {
                    type_id,
                    value: Box::new(Value::Map(merge(self, old, new)?)),
                },
                (_, new) => {
                    return Ok(self.import(Value::Typed {
                        type_id,
                        value: Box::new(new),
                    }));
                }
            },
            (_, new) => return Ok(self.import(new)),
        };
        *self.get_mut(*id)? = replacement;
        Ok(Value::Object(*id))
    }
    pub fn allocate(&mut self, value: Value) -> Value {
        let id = ObjectId(self.next_id);
        self.next_id = self
            .next_id
            .checked_add(1)
            .expect("object identifier space exhausted");
        self.objects.insert(id, value);
        Value::Object(id)
    }

    pub fn get(&self, id: ObjectId) -> Result<&Value, crate::VmError> {
        self.objects
            .get(&id)
            .ok_or(crate::VmError::InvalidObject(id))
    }

    pub fn get_mut(&mut self, id: ObjectId) -> Result<&mut Value, crate::VmError> {
        self.objects
            .get_mut(&id)
            .ok_or(crate::VmError::InvalidObject(id))
    }

    /// Bring owned host records into this execution. Existing object IDs are
    /// already part of the execution and must retain their identity.
    pub fn import(&mut self, value: Value) -> Value {
        match value {
            Value::Map(fields) => {
                let fields = fields
                    .into_iter()
                    .map(|(key, value)| (key, self.import(value)))
                    .collect();
                self.allocate(Value::Map(fields))
            }
            Value::Typed { type_id, value } => {
                if let Value::Map(fields) = *value {
                    let fields = fields
                        .into_iter()
                        .map(|(key, value)| (key, self.import(value)))
                        .collect();
                    self.allocate(Value::Typed {
                        type_id,
                        value: Box::new(Value::Map(fields)),
                    })
                } else {
                    Value::Typed { type_id, value }
                }
            }
            Value::Closure {
                module,
                region,
                captures,
                objects,
            } => {
                let captures = if let Some(objects) = objects {
                    let base = self.next_id;
                    self.next_id = base
                        .checked_add(objects.next_id)
                        .expect("object identifier space exhausted");
                    self.objects
                        .extend(objects.objects.into_iter().map(|(id, value)| {
                            (
                                ObjectId(
                                    id.0.checked_add(base)
                                        .expect("object identifier space exhausted"),
                                ),
                                relocate(value, base),
                            )
                        }));
                    captures
                        .into_iter()
                        .map(|value| relocate(value, base))
                        .collect()
                } else {
                    captures
                        .into_iter()
                        .map(|value| self.import(value))
                        .collect()
                };
                Value::Closure {
                    module,
                    region,
                    captures,
                    objects: None,
                }
            }
            Value::List(values) => {
                Value::List(values.into_iter().map(|value| self.import(value)).collect())
            }
            Value::Tuple(values) => {
                Value::Tuple(values.into_iter().map(|value| self.import(value)).collect())
            }
            Value::Optional(Some(value)) => Value::Optional(Some(Box::new(self.import(*value)))),
            value => value,
        }
    }

    /// Read an owned view at a host boundary. Cyclic graphs remain supported in
    /// the VM/snapshot, but cannot be flattened to a host record tree.
    pub fn export(&self, value: &Value) -> Result<Value, crate::VmError> {
        self.export_inner(value, &mut BTreeSet::new())
    }

    /// Collect unreachable records at an embedding-controlled safe point.
    /// The caller must supply roots from *every* VM sharing this heap as well
    /// as any values retained by the host. Portable closures own separate heaps.
    /// Validation finishes before sweeping, so an invalid root never causes a
    /// partially applied collection. IDs remain stable and are never recycled.
    pub fn collect<'a>(
        &'a mut self,
        roots: impl IntoIterator<Item = &'a Value>,
    ) -> Result<usize, crate::VmError> {
        let mut marked = BTreeSet::new();
        let mut pending = roots.into_iter().collect::<Vec<_>>();
        while let Some(value) = pending.pop() {
            match value {
                Value::Object(id) => {
                    if marked.insert(*id) {
                        pending.push(self.get(*id)?);
                    }
                }
                Value::Map(fields) => pending.extend(fields.values()),
                Value::Typed { value, .. } | Value::Optional(Some(value)) => pending.push(value),
                Value::Tuple(values) | Value::List(values) => pending.extend(values),
                Value::Closure {
                    captures,
                    objects: None,
                    ..
                } => pending.extend(captures),
                _ => {}
            }
        }
        let previous = self.objects.len();
        self.objects.retain(|id, _| marked.contains(id));
        self.last_collection = self.next_id;
        Ok(previous - self.objects.len())
    }

    pub fn live_objects(&self) -> usize {
        self.objects.len()
    }

    pub fn collection_due(&self) -> bool {
        self.next_id.saturating_sub(self.last_collection) >= 1024
    }

    fn export_inner(
        &self,
        value: &Value,
        visiting: &mut BTreeSet<ObjectId>,
    ) -> Result<Value, crate::VmError> {
        Ok(match value {
            Value::Object(id) => {
                if !visiting.insert(*id) {
                    return Err(crate::VmError::CyclicHostValue);
                }
                let value = self.export_inner(self.get(*id)?, visiting)?;
                visiting.remove(id);
                value
            }
            Value::Map(fields) => Value::Map(
                fields
                    .iter()
                    .map(|(key, value)| Ok((key.clone(), self.export_inner(value, visiting)?)))
                    .collect::<Result<BTreeMap<_, _>, crate::VmError>>()?,
            ),
            Value::Typed { type_id, value } => Value::Typed {
                type_id: *type_id,
                value: Box::new(self.export_inner(value, visiting)?),
            },
            Value::List(values) => Value::List(
                values
                    .iter()
                    .map(|value| self.export_inner(value, visiting))
                    .collect::<Result<_, _>>()?,
            ),
            Value::Tuple(values) => Value::Tuple(
                values
                    .iter()
                    .map(|value| self.export_inner(value, visiting))
                    .collect::<Result<_, _>>()?,
            ),
            Value::Optional(Some(value)) => {
                Value::Optional(Some(Box::new(self.export_inner(value, visiting)?)))
            }
            Value::Closure {
                module,
                region,
                captures,
                objects: None,
            } => {
                let mut heap = Self::default();
                let mut ids = BTreeMap::new();
                let captures = captures
                    .iter()
                    .map(|value| self.copy_reachable(value, &mut heap, &mut ids))
                    .collect::<Result<_, _>>()?;
                Value::Closure {
                    module: *module,
                    region: *region,
                    captures,
                    objects: Some(Box::new(heap)),
                }
            }
            value => value.clone(),
        })
    }

    /// Retained callbacks own only their reachable graph, not the entire story
    /// heap. Reserve IDs before visiting fields to preserve aliases and cycles.
    fn copy_reachable(
        &self,
        value: &Value,
        target: &mut Self,
        ids: &mut BTreeMap<ObjectId, Value>,
    ) -> Result<Value, crate::VmError> {
        Ok(match value {
            Value::Object(id) => {
                if let Some(value) = ids.get(id) {
                    return Ok(value.clone());
                }
                let source = self.get(*id)?;
                let reference = target.allocate(Value::Unit);
                ids.insert(*id, reference.clone());
                let record = self.copy_reachable(source, target, ids)?;
                let Value::Object(target_id) = reference else {
                    unreachable!("allocation returns an object reference")
                };
                *target.get_mut(target_id)? = record;
                reference
            }
            Value::Map(fields) => Value::Map(
                fields
                    .iter()
                    .map(|(key, value)| Ok((key.clone(), self.copy_reachable(value, target, ids)?)))
                    .collect::<Result<_, crate::VmError>>()?,
            ),
            Value::Typed { type_id, value } => Value::Typed {
                type_id: *type_id,
                value: Box::new(self.copy_reachable(value, target, ids)?),
            },
            Value::List(values) => Value::List(
                values
                    .iter()
                    .map(|value| self.copy_reachable(value, target, ids))
                    .collect::<Result<_, _>>()?,
            ),
            Value::Tuple(values) => Value::Tuple(
                values
                    .iter()
                    .map(|value| self.copy_reachable(value, target, ids))
                    .collect::<Result<_, _>>()?,
            ),
            Value::Optional(Some(value)) => {
                Value::Optional(Some(Box::new(self.copy_reachable(value, target, ids)?)))
            }
            Value::Closure {
                module,
                region,
                captures,
                objects: None,
            } => Value::Closure {
                module: *module,
                region: *region,
                captures: captures
                    .iter()
                    .map(|value| self.copy_reachable(value, target, ids))
                    .collect::<Result<_, _>>()?,
                objects: None,
            },
            value => value.clone(),
        })
    }
}

fn relocate(value: Value, base: u32) -> Value {
    match value {
        Value::Object(ObjectId(id)) => Value::Object(ObjectId(
            id.checked_add(base).expect("object heap capacity exceeded"),
        )),
        Value::Map(fields) => Value::Map(
            fields
                .into_iter()
                .map(|(key, value)| (key, relocate(value, base)))
                .collect(),
        ),
        Value::Typed { type_id, value } => Value::Typed {
            type_id,
            value: Box::new(relocate(*value, base)),
        },
        Value::List(values) => Value::List(
            values
                .into_iter()
                .map(|value| relocate(value, base))
                .collect(),
        ),
        Value::Tuple(values) => Value::Tuple(
            values
                .into_iter()
                .map(|value| relocate(value, base))
                .collect(),
        ),
        Value::Optional(Some(value)) => Value::Optional(Some(Box::new(relocate(*value, base)))),
        Value::Closure {
            module,
            region,
            captures,
            objects: None,
        } => Value::Closure {
            module,
            region,
            captures: captures
                .into_iter()
                .map(|value| relocate(value, base))
                .collect(),
            objects: None,
        },
        value => value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collection_reclaims_cycles_and_never_reuses_stale_ids() {
        let mut heap = ObjectHeap::default();
        let live = heap.import(Value::Map(BTreeMap::from([(
            "name".into(),
            Value::String("alice".into()),
        )])));
        let dead = heap.allocate(Value::Unit);
        let Value::Object(dead_id) = dead else {
            panic!("expected reference")
        };
        *heap.get_mut(dead_id).expect("allocated object") =
            Value::Map(BTreeMap::from([("self".into(), dead)]));
        let callback = Value::Closure {
            module: None,
            region: 0,
            captures: vec![live.clone()],
            objects: None,
        };
        assert_eq!(heap.collect([&callback]).expect("collection succeeds"), 1);
        assert_eq!(heap.live_objects(), 1);
        assert!(matches!(
            heap.get(dead_id),
            Err(crate::VmError::InvalidObject(_))
        ));
        let encoded = crate::hson::to_string(&heap).expect("sparse heap serializes");
        let mut heap: ObjectHeap = crate::hson::from_str(&encoded).expect("sparse heap restores");
        assert_ne!(heap.allocate(Value::Unit), Value::Object(dead_id));
        assert!(heap.export(&live).is_ok());
        let before = heap.live_objects();
        assert!(heap.collect([&Value::Object(dead_id)]).is_err());
        assert_eq!(
            heap.live_objects(),
            before,
            "invalid roots must not partially collect the heap"
        );
    }

    #[test]
    fn portable_closure_keeps_only_reachable_objects_and_preserves_cycles() {
        let mut heap = ObjectHeap::default();
        heap.import(Value::Map(BTreeMap::from([(
            "unused".into(),
            Value::Number(42.0),
        )])));
        let object = heap.allocate(Value::Unit);
        let Value::Object(id) = object else {
            panic!("expected object reference")
        };
        *heap.get_mut(id).expect("allocated record") = Value::Map(BTreeMap::from([
            ("self".into(), object.clone()),
            ("name".into(), Value::String("alice".into())),
        ]));
        let closure = Value::Closure {
            module: None,
            region: 0,
            captures: vec![object.clone(), object],
            objects: None,
        };
        let portable = heap
            .export(&closure)
            .expect("cyclic captures export as a graph");
        let Value::Closure {
            objects: Some(objects),
            ..
        } = &portable
        else {
            panic!("expected portable closure")
        };
        assert_eq!(objects.objects.len(), 1);
        let encoded = crate::hson::to_string(&portable).expect("graph serializes");
        let portable = crate::hson::from_str(&encoded).expect("graph deserializes");
        let mut target = ObjectHeap::default();
        target.allocate(Value::Unit);
        let Value::Closure { captures, .. } = target.import(portable) else {
            panic!("expected closure")
        };
        assert_eq!(captures[0], captures[1]);
        let Value::Object(id) = captures[0] else {
            panic!("expected reference")
        };
        let Value::Map(fields) = target.get(id).expect("relocated record") else {
            panic!("expected map")
        };
        assert_eq!(fields["self"], captures[0]);
        assert_eq!(id, ObjectId(1));
    }
}
