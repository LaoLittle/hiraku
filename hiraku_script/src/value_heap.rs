//! Owning, compact value storage. Slots never escape their owning heap.
//! Mutable script identity remains ObjectId; value payload nodes have unique
//! ownership and are reclaimed immediately, including their nested slots.
use crate::{SharedStrings, SymbolId, Value, nanbox::Slot};
use lasso::{Key, Spur};
use std::collections::BTreeMap;

const NODE: u16 = 7;
const STRING: u16 = 10;
const SYMBOL: u16 = 11;
const SELECTOR: u16 = 12;
const INT64: u16 = 13;
const UINT64: u16 = 14;
const PERCENT: u16 = 15;
const TASK: u16 = 16;

#[derive(Clone, Debug)]
enum Node {
    String(String),
    Symbol(String),
    Selector(String),
    Optional(Slot),
    Typed(SymbolId, Slot),
    Tuple(Vec<Slot>),
    List(Vec<Slot>),
    Map(BTreeMap<String, Slot>),
    Function {
        module: Option<u32>,
        symbol: SymbolId,
    },
    Handle {
        type_id: u32,
        id: u64,
    },
    Template {
        source: Slot,
        captures: BTreeMap<String, Slot>,
    },
    Closure(Box<ClosurePayload>),
}

// Keep rarely used callable metadata from inflating every payload node.
#[derive(Clone, Debug)]
struct ClosurePayload {
    module: Option<u32>,
    region: u32,
    captures: Vec<Slot>,
    type_bindings: Vec<(SymbolId, crate::ScriptType)>,
    objects: Option<Box<crate::ObjectHeap>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_payloads_roundtrip_and_reuse_all_storage() {
        let mut heap = ValueHeap::default();
        let values = Value::List(vec![
            Value::Int(i64::MIN),
            Value::UInt(u64::MAX),
            Value::Percent(12.5),
            Value::Task(u64::MAX),
            Value::Tuple(vec![
                Value::String("alice".into()),
                Value::Selector("left".into()),
            ]),
            Value::Optional(Some(Box::new(Value::UInt(u64::MAX)))),
            Value::Typed {
                type_id: SymbolId(3),
                value: Box::new(Value::Map(BTreeMap::from([(
                    "name".into(),
                    Value::String("bob".into()),
                )]))),
            },
            Value::Function {
                module: Some(2),
                symbol: SymbolId(1),
            },
            Value::Handle {
                type_id: 3,
                id: u64::MAX,
            },
            Value::Closure {
                module: None,
                region: 1,
                type_bindings: vec![],
                objects: None,
                captures: vec![Value::Int(i64::MAX)],
            },
        ]);
        let slot = heap.pack(values.clone());
        let capacity = (heap.nodes.len(), heap.numbers.len());
        assert_eq!(heap.unpack(slot), values);
        heap.release(slot);
        for _ in 0..1000 {
            let slot = heap.pack(values.clone());
            assert_eq!(heap.unpack(slot), values);
            heap.release(slot);
        }
        assert_eq!(heap.usage(), (0, 0));
        assert_eq!((heap.nodes.len(), heap.numbers.len()), capacity);
    }

    #[test]
    fn shared_literals_do_not_retain_transient_strings() {
        let strings = SharedStrings::with_budget(4096);
        strings.intern("alice").expect("pool capacity");
        let mut first = ValueHeap::with_strings(strings.clone());
        let mut second = ValueHeap::with_strings(strings.clone());
        let a = first.pack(Value::String("alice".into()));
        let b = second.pack(Value::String("alice".into()));
        assert_eq!(a, b);
        assert_eq!(a.tag(), Some(STRING));
        for index in 0..1000 {
            let value = first.pack(Value::String(format!("temporary-{index}")));
            first.release(value);
        }
        assert_eq!(strings.len(), 1);
        assert_eq!(first.usage(), (0, 0));
        assert_eq!(first.nodes.len(), 1);
        assert_eq!(second.unpack(b), Value::String("alice".into()));
    }

    #[test]
    fn wide_integers_use_only_dense_scalar_lane() {
        let mut heap = ValueHeap::default();
        for _ in 0..1000 {
            let slot = heap.pack(Value::UInt(u64::MAX));
            assert_eq!(heap.unpack(slot), Value::UInt(u64::MAX));
            heap.release(slot);
        }
        assert!(heap.nodes.is_empty());
        assert_eq!(heap.numbers.len(), 1);
        assert_eq!(heap.usage(), (0, 0));
    }

    #[test]
    fn field_mutation_releases_only_replaced_payload() {
        let mut heap = ValueHeap::default();
        let record = heap.pack(Value::Map(BTreeMap::from([
            ("left".into(), Value::UInt(u64::MAX)),
            ("right".into(), Value::Int(i64::MIN)),
        ])));
        for _ in 0..1000 {
            heap.set_member(record, "left", Value::Int(i64::MAX))
                .expect("field");
        }
        assert_eq!(
            heap.member(record, "right").expect("field"),
            Value::Int(i64::MIN)
        );
        assert_eq!(heap.usage(), (1, 2));
        assert_eq!(heap.numbers.len(), 3);
        heap.release(record);
        assert_eq!(heap.usage(), (0, 0));
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ValueHeap {
    pub strings: SharedStrings,
    nodes: Vec<Option<Node>>,
    free_nodes: Vec<u32>,
    // Dense scalar lane: no Value discriminant, object header or per-int Box.
    numbers: Vec<u64>,
    free_numbers: Vec<u32>,
}

impl ValueHeap {
    /// Transfer ownership into another arena without materializing scalar Values.
    /// Heap indices are local; only immediates and shared interner keys may be copied verbatim.
    pub fn copy_from(&mut self, source: &Self, slot: Slot) -> Slot {
        match slot.tag() {
            Some(INT64 | UINT64 | PERCENT | TASK) => self.number(
                slot.tag().expect("numeric tag"),
                source.numbers[slot.payload() as usize],
            ),
            Some(STRING | SYMBOL | SELECTOR)
                if self.strings.shares_storage_with(&source.strings) =>
            {
                slot
            }
            Some(NODE | STRING | SYMBOL | SELECTOR) => self.pack(source.unpack(slot)),
            _ => slot,
        }
    }

    pub fn duplicate(&mut self, slot: Slot) -> Slot {
        match slot.tag() {
            Some(INT64 | UINT64 | PERCENT | TASK) => self.number(
                slot.tag().expect("numeric tag"),
                self.numbers[slot.payload() as usize],
            ),
            Some(NODE) => self.pack(self.unpack(slot)),
            _ => slot,
        }
    }

    pub fn with_strings(strings: SharedStrings) -> Self {
        Self {
            strings,
            ..Self::default()
        }
    }

    pub fn pack(&mut self, value: Value) -> Slot {
        if let Some(slot) = Slot::immediate(&value) {
            return slot;
        }
        let node = match value {
            Value::Int(n) => return self.number(INT64, n as u64),
            Value::UInt(n) => return self.number(UINT64, n),
            Value::Percent(n) => return self.number(PERCENT, n.to_bits()),
            Value::Task(n) => return self.number(TASK, n),
            Value::String(s) => {
                if let Some(key) = self.strings.get(&s) {
                    return Slot::tagged(STRING, key.into_usize() as u32);
                }
                Node::String(s)
            }
            Value::Symbol(s) => {
                if let Some(key) = self.strings.intern(&s) {
                    return Slot::tagged(SYMBOL, key.into_usize() as u32);
                }
                Node::Symbol(s)
            }
            Value::Selector(s) => {
                if let Some(key) = self.strings.intern(&s) {
                    return Slot::tagged(SELECTOR, key.into_usize() as u32);
                }
                Node::Selector(s)
            }
            Value::Optional(Some(v)) => Node::Optional(self.pack(*v)),
            Value::Typed { type_id, value } => Node::Typed(type_id, self.pack(*value)),
            Value::Tuple(v) => Node::Tuple(self.pack_many(v)),
            Value::List(v) => Node::List(self.pack_many(v)),
            Value::Map(v) => Node::Map(v.into_iter().map(|(k, v)| (k, self.pack(v))).collect()),
            Value::Function { module, symbol } => Node::Function { module, symbol },
            Value::Handle { type_id, id } => Node::Handle { type_id, id },
            Value::TextTemplate(v) => Node::Template {
                source: self.pack(Value::String(v.source)),
                captures: v
                    .captures
                    .iter()
                    .map(|(k, v)| (k.clone(), self.pack(v.clone())))
                    .collect(),
            },
            Value::Closure {
                type_bindings,
                module,
                region,
                captures,
                objects,
            } => Node::Closure(Box::new(ClosurePayload {
                type_bindings,
                module,
                region,
                objects,
                captures: self.pack_many(captures),
            })),
            _ => unreachable!("all remaining variants are immediate"),
        };
        let id = if let Some(id) = self.free_nodes.pop() {
            self.nodes[id as usize] = Some(node);
            id
        } else {
            let id = u32::try_from(self.nodes.len()).expect("value heap exhausted u32 indices");
            self.nodes.push(Some(node));
            id
        };
        Slot::tagged(NODE, id)
    }

    fn pack_many(&mut self, values: Vec<Value>) -> Vec<Slot> {
        values.into_iter().map(|v| self.pack(v)).collect()
    }
    fn number(&mut self, tag: u16, value: u64) -> Slot {
        let id = if let Some(id) = self.free_numbers.pop() {
            self.numbers[id as usize] = value;
            id
        } else {
            let id = u32::try_from(self.numbers.len()).expect("integer heap exhausted u32 indices");
            self.numbers.push(value);
            id
        };
        Slot::tagged(tag, id)
    }

    pub fn unpack(&self, slot: Slot) -> Value {
        let index = slot.payload() as usize;
        match slot.tag() {
            Some(STRING | SYMBOL | SELECTOR) => {
                let text = self
                    .strings
                    .resolve(Spur::try_from_usize(index).expect("interned key"))
                    .to_owned();
                match slot.tag() {
                    Some(STRING) => Value::String(text),
                    Some(SYMBOL) => Value::Symbol(text),
                    _ => Value::Selector(text),
                }
            }
            Some(INT64) => Value::Int(self.numbers[index] as i64),
            Some(UINT64) => Value::UInt(self.numbers[index]),
            Some(PERCENT) => Value::Percent(f64::from_bits(self.numbers[index])),
            Some(TASK) => Value::Task(self.numbers[index]),
            Some(NODE) => match self.nodes[index].as_ref().expect("live heap node") {
                Node::String(v) => Value::String(v.clone()),
                Node::Symbol(v) => Value::Symbol(v.clone()),
                Node::Selector(v) => Value::Selector(v.clone()),
                Node::Optional(v) => Value::Optional(Some(Box::new(self.unpack(*v)))),
                Node::Typed(type_id, v) => Value::Typed {
                    type_id: *type_id,
                    value: Box::new(self.unpack(*v)),
                },
                Node::Tuple(v) => Value::Tuple(self.unpack_many(v)),
                Node::List(v) => Value::List(self.unpack_many(v)),
                Node::Map(v) => Value::Map(
                    v.iter()
                        .map(|(k, v)| (k.clone(), self.unpack(*v)))
                        .collect(),
                ),
                Node::Function { module, symbol } => Value::Function {
                    module: *module,
                    symbol: *symbol,
                },
                Node::Handle { type_id, id } => Value::Handle {
                    type_id: *type_id,
                    id: *id,
                },
                Node::Template { source, captures } => {
                    let Value::String(source) = self.unpack(*source) else {
                        unreachable!("template source")
                    };
                    Value::TextTemplate(crate::runtime::TemplateValue {
                        source,
                        captures: captures
                            .iter()
                            .map(|(k, v)| (k.clone(), self.unpack(*v)))
                            .collect::<BTreeMap<_, _>>()
                            .into(),
                    })
                }
                Node::Closure(closure) => Value::Closure {
                    module: closure.module,
                    region: closure.region,
                    captures: self.unpack_many(&closure.captures),
                    type_bindings: closure.type_bindings.clone(),
                    objects: closure.objects.clone(),
                },
            },
            _ => slot.decode_immediate(),
        }
    }

    fn unpack_many(&self, values: &[Slot]) -> Vec<Value> {
        values.iter().map(|v| self.unpack(*v)).collect()
    }

    pub fn visit_objects(&self, slot: Slot, visit: &mut impl FnMut(crate::ObjectId)) {
        if slot.tag() == Some(5) {
            visit(crate::ObjectId(slot.payload()));
            return;
        }
        if slot.tag() != Some(NODE) {
            return;
        }
        match self.nodes[slot.payload() as usize]
            .as_ref()
            .expect("live node")
        {
            Node::Typed(_, value) | Node::Optional(value) => self.visit_objects(*value, visit),
            Node::List(values) | Node::Tuple(values) => {
                for value in values {
                    self.visit_objects(*value, visit);
                }
            }
            Node::Closure(closure) if closure.objects.is_none() => {
                for value in &closure.captures {
                    self.visit_objects(*value, visit);
                }
            }
            Node::Map(fields)
            | Node::Template {
                captures: fields, ..
            } => {
                for value in fields.values() {
                    self.visit_objects(*value, visit);
                }
            }
            _ => {}
        }
    }

    // Field access must not unpack/repack every sibling in a record.
    pub fn member(&self, slot: Slot, name: &str) -> Result<Value, crate::VmError> {
        match self
            .nodes
            .get(slot.payload() as usize)
            .and_then(Option::as_ref)
            .filter(|_| slot.tag() == Some(NODE))
        {
            Some(Node::Typed(_, value)) => self.member(*value, name),
            Some(Node::Map(fields)) => fields
                .get(name)
                .map(|v| self.unpack(*v))
                .ok_or_else(|| crate::VmError::UnknownMember(name.to_owned())),
            _ => Err(crate::VmError::TypeMismatch(
                "member access expects an object",
            )),
        }
    }

    pub fn set_member(
        &mut self,
        slot: Slot,
        name: &str,
        value: Value,
    ) -> Result<(), crate::VmError> {
        let index = slot.payload() as usize;
        let node = self
            .nodes
            .get(index)
            .and_then(Option::as_ref)
            .filter(|_| slot.tag() == Some(NODE));
        match node {
            Some(Node::Typed(_, inner)) => return self.set_member(*inner, name, value),
            Some(Node::Map(fields)) if fields.contains_key(name) => {}
            Some(Node::Map(_)) => return Err(crate::VmError::UnknownMember(name.to_owned())),
            _ => {
                return Err(crate::VmError::TypeMismatch(
                    "member assignment expects an object",
                ));
            }
        }
        let next = self.pack(value);
        let Some(Node::Map(fields)) = self.nodes[index].as_mut() else {
            unreachable!("validated map")
        };
        let previous = fields
            .insert(name.to_owned(), next)
            .expect("validated member");
        self.release(previous);
        Ok(())
    }

    pub fn release(&mut self, slot: Slot) {
        match slot.tag() {
            Some(INT64 | UINT64 | PERCENT | TASK) => self.free_numbers.push(slot.payload()),
            Some(NODE) => {
                let node = self.nodes[slot.payload() as usize]
                    .take()
                    .expect("uniquely owned heap node");
                self.free_nodes.push(slot.payload());
                match node {
                    Node::Optional(v) | Node::Typed(_, v) => self.release(v),
                    Node::Tuple(v) | Node::List(v) => {
                        for v in v {
                            self.release(v);
                        }
                    }
                    Node::Closure(v) => {
                        for v in v.captures {
                            self.release(v);
                        }
                    }
                    Node::Map(v) => {
                        for v in v.into_values() {
                            self.release(v);
                        }
                    }
                    Node::Template { source, captures } => {
                        self.release(source);
                        for v in captures.into_values() {
                            self.release(v);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    #[cfg(test)]
    pub fn usage(&self) -> (usize, usize) {
        (
            self.nodes.len() - self.free_nodes.len(),
            self.numbers.len() - self.free_numbers.len(),
        )
    }
}
