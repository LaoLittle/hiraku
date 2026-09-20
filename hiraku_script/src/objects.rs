//! Execution-owned object storage. Values copy IDs, never object contents.
//! No locks or Rust pointers enter bytecode or snapshots.
use crate::Value;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ObjectId(pub u32);

#[derive(Clone, Debug, Default)]
pub struct ObjectHeap {
    objects: BTreeMap<ObjectId, crate::nanbox::Slot>,
    storage: crate::value_heap::ValueHeap,
    // IDs are never reused, including after collection or snapshot restore.
    next_id: u32,
    last_collection: u32,
    read_only: BTreeSet<ObjectId>,
}

#[derive(Serialize, Deserialize)]
struct HeapSnapshot {
    objects: Vec<(ObjectId, Value)>,
    next_id: u32,
    read_only: BTreeSet<ObjectId>,
}

impl PartialEq for ObjectHeap {
    fn eq(&self, other: &Self) -> bool {
        self.next_id == other.next_id
            && self.read_only == other.read_only
            && self.objects.len() == other.objects.len()
            && self
                .objects
                .keys()
                .all(|id| self.get(*id) == other.get(*id))
    }
}
impl Serialize for ObjectHeap {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        HeapSnapshot {
            objects: self
                .objects
                .iter()
                .map(|(id, slot)| (*id, self.storage.unpack(*slot)))
                .collect(),
            next_id: self.next_id,
            read_only: self.read_only.clone(),
        }
        .serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for ObjectHeap {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let saved = HeapSnapshot::deserialize(deserializer)?;
        let mut heap = Self {
            next_id: saved.next_id,
            read_only: saved.read_only,
            ..Self::default()
        };
        for (id, value) in saved.objects {
            if id.0 >= heap.next_id || heap.objects.contains_key(&id) {
                return Err(serde::de::Error::custom(
                    "duplicate or out-of-range object ID in heap snapshot",
                ));
            }
            let value = heap.storage.pack(value);
            heap.objects.insert(id, value);
        }
        if heap
            .read_only
            .iter()
            .any(|id| !heap.objects.contains_key(id))
        {
            return Err(serde::de::Error::custom(
                "read-only set refers to a missing object",
            ));
        }
        Ok(heap)
    }
}

impl ObjectHeap {
    pub fn with_strings(strings: crate::SharedStrings) -> Self {
        Self {
            storage: crate::value_heap::ValueHeap::with_strings(strings),
            ..Self::default()
        }
    }
    /// Apply an owned host update without breaking aliases to an existing record.
    pub fn update(&mut self, old: &Value, new: Value) -> Result<Value, crate::VmError> {
        let Value::Object(id) = old else {
            return Ok(self.import(new));
        };
        if matches!(new, Value::Object(_)) {
            return Ok(new);
        }
        let previous = self.get(*id)?;
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
        self.replace(*id, replacement)?;
        Ok(Value::Object(*id))
    }
    pub fn allocate(&mut self, value: Value) -> Value {
        let id = ObjectId(self.next_id);
        self.next_id = self
            .next_id
            .checked_add(1)
            .expect("object identifier space exhausted");
        let value = self.storage.pack(value);
        self.objects.insert(id, value);
        Value::Object(id)
    }

    pub fn get(&self, id: ObjectId) -> Result<Value, crate::VmError> {
        self.objects
            .get(&id)
            .map(|slot| self.storage.unpack(*slot))
            .ok_or(crate::VmError::InvalidObject(id))
    }

    pub(crate) fn member(&self, id: ObjectId, name: &str) -> Result<Value, crate::VmError> {
        let slot = self
            .objects
            .get(&id)
            .ok_or(crate::VmError::InvalidObject(id))?;
        self.storage.member(*slot, name)
    }

    pub(crate) fn set_member(
        &mut self,
        id: ObjectId,
        name: &str,
        value: Value,
    ) -> Result<(), crate::VmError> {
        if self.read_only.contains(&id) {
            return Err(crate::VmError::ReadOnlyValue);
        }
        let slot = *self
            .objects
            .get(&id)
            .ok_or(crate::VmError::InvalidObject(id))?;
        self.storage.set_member(slot, name, value)
    }

    pub fn replace(&mut self, id: ObjectId, value: Value) -> Result<(), crate::VmError> {
        if self.read_only.contains(&id) {
            return Err(crate::VmError::ReadOnlyValue);
        }
        let slot = self
            .objects
            .get_mut(&id)
            .ok_or(crate::VmError::InvalidObject(id))?;
        self.storage.release(*slot);
        *slot = self.storage.pack(value);
        Ok(())
    }

    /// Freeze a reachable graph, including aliases, nested records and cycles.
    /// Portable closures retain this property when their heaps are relocated.
    pub fn freeze(&mut self, root: &Value) -> Result<(), crate::VmError> {
        let mut pending = vec![root.clone()];
        while let Some(value) = pending.pop() {
            match value {
                Value::Object(id) => {
                    if self.read_only.insert(id) {
                        pending.push(self.get(id)?);
                    }
                }
                Value::Map(fields) => pending.extend(fields.into_values()),
                Value::TextTemplate(template) => {
                    pending.extend(template.captures.values().cloned())
                }
                Value::Typed { value, .. } | Value::Optional(Some(value)) => pending.push(*value),
                Value::Tuple(values) | Value::List(values) => pending.extend(values),
                Value::Closure {
                    captures,
                    objects: None,
                    ..
                } => pending.extend(captures),
                _ => {}
            }
        }
        Ok(())
    }

    /// Bring owned host records into this execution. Existing object IDs are
    /// already part of the execution and must retain their identity.
    pub fn import(&mut self, value: Value) -> Value {
        match value {
            Value::TextTemplate(mut template) => {
                template.captures = template
                    .captures
                    .iter()
                    .map(|(name, value)| (name.clone(), self.import(value.clone())))
                    .collect::<BTreeMap<_, _>>()
                    .into();
                Value::TextTemplate(template)
            }
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
                type_bindings,
                module,
                region,
                captures,
                objects,
            } => {
                let captures = if let Some(objects) = objects {
                    let base = self.next_id;
                    self.read_only.extend(objects.read_only.iter().map(|id| {
                        ObjectId(
                            id.0.checked_add(base)
                                .expect("object identifier space exhausted"),
                        )
                    }));
                    self.next_id = base
                        .checked_add(objects.next_id)
                        .expect("object identifier space exhausted");
                    for (id, slot) in objects.objects {
                        let value = relocate(objects.storage.unpack(slot), base);
                        let slot = self.storage.pack(value);
                        self.objects.insert(
                            ObjectId(
                                id.0.checked_add(base)
                                    .expect("object identifier space exhausted"),
                            ),
                            slot,
                        );
                    }
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
                    type_bindings,
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
        let mut pending = roots.into_iter().cloned().collect::<Vec<_>>();
        while let Some(value) = pending.pop() {
            match value {
                Value::Object(id) => {
                    if marked.insert(id) {
                        let slot = *self
                            .objects
                            .get(&id)
                            .ok_or(crate::VmError::InvalidObject(id))?;
                        self.storage
                            .visit_objects(slot, &mut |id| pending.push(Value::Object(id)));
                    }
                }
                Value::Map(fields) => pending.extend(fields.into_values()),
                Value::TextTemplate(template) => {
                    pending.extend(template.captures.values().cloned())
                }
                Value::Typed { value, .. } | Value::Optional(Some(value)) => pending.push(*value),
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
        self.objects.retain(|id, slot| {
            if marked.contains(id) {
                true
            } else {
                self.storage.release(*slot);
                false
            }
        });
        self.read_only.retain(|id| marked.contains(id));
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
            Value::TextTemplate(template) => Value::TextTemplate(crate::runtime::TemplateValue {
                source: template.source.clone(),
                captures: template
                    .captures
                    .iter()
                    .map(|(name, value)| Ok((name.clone(), self.export_inner(value, visiting)?)))
                    .collect::<Result<BTreeMap<_, _>, crate::VmError>>()?
                    .into(),
            }),
            Value::Object(id) => {
                if !visiting.insert(*id) {
                    return Err(crate::VmError::CyclicHostValue);
                }
                let value = self.export_inner(&self.get(*id)?, visiting)?;
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
                type_bindings,
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
                    type_bindings: type_bindings.clone(),
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
            Value::TextTemplate(template) => Value::TextTemplate(crate::runtime::TemplateValue {
                source: template.source.clone(),
                captures: template
                    .captures
                    .iter()
                    .map(|(name, value)| {
                        Ok((name.clone(), self.copy_reachable(value, target, ids)?))
                    })
                    .collect::<Result<BTreeMap<_, _>, crate::VmError>>()?
                    .into(),
            }),
            Value::Object(id) => {
                if let Some(value) = ids.get(id) {
                    return Ok(value.clone());
                }
                let source = self.get(*id)?;
                let reference = target.allocate(Value::Unit);
                ids.insert(*id, reference.clone());
                let record = self.copy_reachable(&source, target, ids)?;
                let Value::Object(target_id) = reference else {
                    unreachable!("allocation returns an object reference")
                };
                target.replace(target_id, record)?;
                if self.read_only.contains(id) {
                    target.read_only.insert(target_id);
                }
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
                type_bindings,
                module,
                region,
                captures,
                objects: None,
            } => Value::Closure {
                type_bindings: type_bindings.clone(),
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
        Value::TextTemplate(mut template) => {
            template.captures = template
                .captures
                .iter()
                .map(|(name, value)| (name.clone(), relocate(value.clone(), base)))
                .collect::<BTreeMap<_, _>>()
                .into();
            Value::TextTemplate(template)
        }
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
            type_bindings,
            module,
            region,
            captures,
            objects: None,
        } => Value::Closure {
            type_bindings,
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
    fn snapshot_restores_logical_values_not_pool_indices() {
        let strings = crate::SharedStrings::with_budget(4096);
        strings.intern("bob");
        strings.intern("alice");
        let mut heap = ObjectHeap::with_strings(strings);
        let value = Value::Map(BTreeMap::from([
            ("name".into(), Value::String("alice".into())),
            ("score".into(), Value::UInt(u64::MAX)),
        ]));
        let reference = heap.import(value.clone());
        let bytes = crate::hson::to_vec(&heap).expect("snapshot serializes");
        let restored: ObjectHeap = crate::hson::from_slice(&bytes).expect("snapshot restores");
        assert_eq!(restored.export(&reference).expect("object exports"), value);
        assert_eq!(heap, restored);
        heap.collect(std::iter::empty())
            .expect("collect unreachable payloads");
        assert_eq!(heap.storage.usage(), (0, 0));
    }

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
        heap.replace(dead_id, Value::Map(BTreeMap::from([("self".into(), dead)])))
            .expect("allocated object");
        let callback = Value::Closure {
            type_bindings: Vec::new(),
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
        heap.replace(
            id,
            Value::Map(BTreeMap::from([
                ("self".into(), object.clone()),
                ("name".into(), Value::String("alice".into())),
            ])),
        )
        .expect("allocated record");
        let closure = Value::Closure {
            type_bindings: Vec::new(),
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

    #[test]
    fn frozen_nested_graph_survives_portable_closure_and_collection() {
        let mut heap = ObjectHeap::default();
        let root = heap.import(Value::Map(BTreeMap::from([(
            "child".into(),
            Value::Map(BTreeMap::from([(
                "name".into(),
                Value::String("alice".into()),
            )])),
        )])));
        heap.freeze(&root).expect("freeze nested record");
        let closure = Value::Closure {
            type_bindings: Vec::new(),
            module: None,
            region: 0,
            captures: vec![root],
            objects: None,
        };
        let portable = heap.export(&closure).expect("portable closure");
        let encoded = crate::hson::to_string(&portable).expect("serialize");
        let portable = crate::hson::from_str(&encoded).expect("deserialize");
        let mut target = ObjectHeap::default();
        target.allocate(Value::Unit);
        let closure = target.import(portable);
        target.collect([&closure]).expect("collect unused object");
        let Value::Closure { captures, .. } = closure else {
            panic!("closure");
        };
        let Value::Object(root) = captures[0] else {
            panic!("object");
        };
        let Value::Map(fields) = target.get(root).expect("root") else {
            panic!("record");
        };
        let Value::Object(child) = fields["child"] else {
            panic!("child");
        };
        assert_eq!(
            target.replace(root, Value::Unit),
            Err(crate::VmError::ReadOnlyValue)
        );
        assert_eq!(
            target.replace(child, Value::Unit),
            Err(crate::VmError::ReadOnlyValue)
        );
        let Value::Object(local) = target.allocate(Value::Unit) else {
            panic!("object");
        };
        assert!(
            target.replace(local, Value::Unit).is_ok(),
            "new local allocations remain mutable"
        );
    }
}
