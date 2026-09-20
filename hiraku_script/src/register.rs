//! Register allocation and fixed-size runtime frame primitives.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{MirFunction, Value, VirtualRegister};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Register(pub u16);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisterAllocation {
    physical: Vec<Register>,
    pub register_count: u16,
}

impl RegisterAllocation {
    pub fn register_for(&self, virtual_register: VirtualRegister) -> Option<Register> {
        self.physical.get(virtual_register.0 as usize).copied()
    }

    pub fn registers(&self) -> &[Register] {
        &self.physical
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegisterAllocationError {
    TooManyRegisters(u32),
    InvalidSuccessor(u32),
}

/// Computes block liveness to a fixed point, builds an interference graph, and
/// greedily colors the graph in descending-degree order. Every virtual value
/// receives a physical register; this backend deliberately has no spill path.
pub fn allocate_registers(
    function: &MirFunction,
) -> Result<RegisterAllocation, RegisterAllocationError> {
    let count = function.virtual_register_count as usize;
    let mut block_use = vec![BTreeSet::new(); function.blocks.len()];
    let mut block_def = vec![BTreeSet::new(); function.blocks.len()];
    for (index, block) in function.blocks.iter().enumerate() {
        for instruction in &block.instructions {
            for register in instruction.used_registers() {
                if !block_def[index].contains(&register.0) {
                    block_use[index].insert(register.0);
                }
            }
            if let Some(register) = instruction.defined_register() {
                block_def[index].insert(register.0);
            }
        }
        for register in block.terminator.used_registers() {
            if !block_def[index].contains(&register.0) {
                block_use[index].insert(register.0);
            }
        }
    }

    let mut live_in = vec![BTreeSet::new(); function.blocks.len()];
    let mut live_out = vec![BTreeSet::new(); function.blocks.len()];
    loop {
        let mut changed = false;
        for index in (0..function.blocks.len()).rev() {
            let mut next_out = BTreeSet::new();
            for successor in function.blocks[index].terminator.successors() {
                let Some(successor_live) = live_in.get(successor.0 as usize) else {
                    return Err(RegisterAllocationError::InvalidSuccessor(successor.0));
                };
                next_out.extend(successor_live.iter().copied());
            }
            let mut next_in = block_use[index].clone();
            next_in.extend(
                next_out
                    .iter()
                    .filter(|register| !block_def[index].contains(register))
                    .copied(),
            );
            changed |= next_in != live_in[index] || next_out != live_out[index];
            live_in[index] = next_in;
            live_out[index] = next_out;
        }
        if !changed {
            break;
        }
    }

    let mut interference = vec![BTreeSet::<u32>::new(); count];
    for (index, block) in function.blocks.iter().enumerate() {
        let mut live = live_out[index].clone();
        live.extend(
            block
                .terminator
                .used_registers()
                .into_iter()
                .map(|register| register.0),
        );
        for instruction in block.instructions.iter().rev() {
            if let Some(defined) = instruction.defined_register() {
                for other in live.iter().copied().filter(|other| *other != defined.0) {
                    add_edge(&mut interference, defined.0, other);
                }
                live.remove(&defined.0);
            }
            live.extend(
                instruction
                    .used_registers()
                    .into_iter()
                    .map(|register| register.0),
            );
        }
    }

    let mut order = (0..function.virtual_register_count).collect::<Vec<_>>();
    order.sort_by_key(|register| {
        (
            std::cmp::Reverse(interference[*register as usize].len()),
            *register,
        )
    });
    let mut colors = vec![None::<u16>; count];
    let mut register_count = 0u32;
    for virtual_register in order {
        let occupied = interference[virtual_register as usize]
            .iter()
            .filter_map(|neighbor| colors[*neighbor as usize])
            .collect::<BTreeSet<_>>();
        let color = (0..=u16::MAX)
            .find(|candidate| !occupied.contains(candidate))
            .ok_or(RegisterAllocationError::TooManyRegisters(
                function.virtual_register_count,
            ))?;
        colors[virtual_register as usize] = Some(color);
        register_count = register_count.max(u32::from(color) + 1);
    }
    let register_count = u16::try_from(register_count)
        .map_err(|_| RegisterAllocationError::TooManyRegisters(function.virtual_register_count))?;
    Ok(RegisterAllocation {
        physical: colors
            .into_iter()
            .map(|color| Register(color.expect("every virtual register is colored")))
            .collect(),
        register_count,
    })
}

fn add_edge(graph: &mut [BTreeSet<u32>], left: u32, right: u32) {
    if let Some(edges) = graph.get_mut(left as usize) {
        edges.insert(right);
    }
    if let Some(edges) = graph.get_mut(right as usize) {
        edges.insert(left);
    }
}

#[derive(Clone, Debug)]
pub struct RegisterFrame {
    registers: Box<[crate::nanbox::Slot]>,
    heap: crate::value_heap::ValueHeap,
}

impl PartialEq for RegisterFrame {
    fn eq(&self, other: &Self) -> bool {
        self.values().eq(other.values())
    }
}

impl RegisterFrame {
    /// Copy a register without expanding immediate values or interned strings.
    pub fn copy(&mut self, dst: Register, src: Register) -> Result<(), InvalidRegister> {
        let slot = *self
            .registers
            .get(src.0 as usize)
            .ok_or(InvalidRegister(src))?;
        if dst.0 as usize >= self.len() {
            return Err(InvalidRegister(dst));
        }
        if dst == src {
            return Ok(());
        }
        let value = self.heap.duplicate(slot);
        let previous = std::mem::replace(&mut self.registers[dst.0 as usize], value);
        self.heap.release(previous);
        Ok(())
    }

    pub(crate) fn copy_from(
        &mut self,
        dst: usize,
        source: &Self,
        src: usize,
    ) -> Result<(), SlotCopyError> {
        let slot = *source.registers.get(src).ok_or(SlotCopyError::Source)?;
        if dst >= self.len() {
            return Err(SlotCopyError::Destination);
        }
        let value = self.heap.copy_from(&source.heap, slot);
        let previous = std::mem::replace(&mut self.registers[dst], value);
        self.heap.release(previous);
        Ok(())
    }

    pub(crate) fn is_initialized(&self, index: usize) -> Option<bool> {
        self.registers
            .get(index)
            .map(|value| *value != crate::nanbox::Slot::EMPTY)
    }
    pub fn new(register_count: u16) -> Self {
        Self::with_len(register_count as usize, crate::SharedStrings::default())
    }

    pub(crate) fn with_len(count: usize, strings: crate::SharedStrings) -> Self {
        Self {
            registers: vec![crate::nanbox::Slot::EMPTY; count].into_boxed_slice(),
            heap: crate::value_heap::ValueHeap::with_strings(strings),
        }
    }

    pub(crate) fn from_values(values: Vec<Value>, strings: crate::SharedStrings) -> Self {
        let mut heap = crate::value_heap::ValueHeap::with_strings(strings);
        let registers = values.into_iter().map(|value| heap.pack(value)).collect();
        Self { registers, heap }
    }
    pub(crate) fn strings(&self) -> crate::SharedStrings {
        self.heap.strings.clone()
    }

    pub fn len(&self) -> usize {
        self.registers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.registers.is_empty()
    }

    #[inline]
    pub fn read(&self, register: Register) -> Option<Value> {
        self.get(register.0 as usize)
    }

    pub(crate) fn get(&self, index: usize) -> Option<Value> {
        self.registers
            .get(index)
            .map(|slot| self.heap.unpack(*slot))
    }

    #[inline]
    pub fn write(&mut self, register: Register, value: Value) -> Result<(), InvalidRegister> {
        self.set(register.0 as usize, value)
            .map_err(|_| InvalidRegister(register))
    }

    pub(crate) fn set(&mut self, index: usize, value: Value) -> Result<(), ()> {
        let Some(slot) = self.registers.get_mut(index) else {
            return Err(());
        };
        self.heap.release(*slot);
        *slot = self.heap.pack(value);
        Ok(())
    }

    pub fn values(&self) -> impl Iterator<Item = Value> + '_ {
        self.registers.iter().map(|slot| self.heap.unpack(*slot))
    }

    /// Scalar arithmetic stays inside compact registers, including comparisons.
    #[inline(always)]
    pub fn binary(
        &mut self,
        dst: Register,
        op: crate::BinaryOp,
        left: Register,
        right: Register,
    ) -> Result<(), crate::VmError> {
        let a = *self
            .registers
            .get(left.0 as usize)
            .ok_or(crate::VmError::InvalidRegister(left))?;
        let b = *self
            .registers
            .get(right.0 as usize)
            .ok_or(crate::VmError::InvalidRegister(right))?;
        if let Some(value) = a.binary(op, b)? {
            let slot = self
                .registers
                .get_mut(dst.0 as usize)
                .ok_or(crate::VmError::InvalidRegister(dst))?;
            self.heap.release(*slot);
            *slot = value;
            Ok(())
        } else {
            let value = crate::vm::binary(op, &self.heap.unpack(a), &self.heap.unpack(b))?;
            self.write(dst, value)
                .map_err(|_| crate::VmError::InvalidRegister(dst))
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidRegister(pub Register);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SlotCopyError {
    Source,
    Destination,
}

#[cfg(test)]
mod tests {
    use crate::{HirArena, lower_hir_to_mir, lower_to_hir, parse_program};

    use super::*;

    #[test]
    fn copies_own_their_payload_and_allow_self_assignment() {
        let mut frame = RegisterFrame::new(3);
        for value in [
            Value::UInt(u64::MAX),
            Value::Int(i64::MIN),
            Value::List(vec![Value::String("alice".into()), Value::Int(i64::MAX)]),
        ] {
            frame.write(Register(0), value.clone()).expect("source");
            frame.copy(Register(1), Register(0)).expect("copy");
            frame
                .copy(Register(1), Register(1))
                .expect("self assignment");
            frame
                .write(Register(0), Value::Unit)
                .expect("release source");
            assert_eq!(frame.read(Register(1)), Some(value.clone()));
            let mut other = RegisterFrame::new(1);
            other.copy_from(0, &frame, 1).expect("cross-frame copy");
            frame
                .write(Register(1), Value::Unit)
                .expect("release original");
            assert_eq!(other.read(Register(0)), Some(value));
        }
        assert_eq!(frame.heap.usage(), (0, 0));
    }

    #[test]
    fn copies_between_distinct_string_pools_remap_keys() {
        let first = crate::SharedStrings::with_budget(4096);
        let second = crate::SharedStrings::with_budget(4096);
        first.intern("alice");
        second.intern("bob");
        second.intern("alice");
        let source = RegisterFrame::from_values(vec![Value::String("alice".into())], first);
        let mut target = RegisterFrame::with_len(1, second);
        target
            .copy_from(0, &source, 0)
            .expect("copy remaps interner key");
        drop(source);
        assert_eq!(
            target.read(Register(0)),
            Some(Value::String("alice".into()))
        );
    }

    #[test]
    fn invalid_copy_does_not_mutate_destination() {
        let mut frame = RegisterFrame::new(1);
        frame
            .write(Register(0), Value::UInt(u64::MAX))
            .expect("slot");
        assert_eq!(
            frame.copy(Register(0), Register(1)),
            Err(InvalidRegister(Register(1)))
        );
        assert_eq!(
            frame.copy(Register(1), Register(0)),
            Err(InvalidRegister(Register(1)))
        );
        assert_eq!(frame.read(Register(0)), Some(Value::UInt(u64::MAX)));
        assert_eq!(frame.heap.usage(), (0, 1));
    }

    #[test]
    fn cold_slots_are_reused_and_dropped_on_overwrite() {
        let mut frame = RegisterFrame::new(2);
        for _ in 0..10_000 {
            frame
                .write(Register(0), Value::String("alice".into()))
                .expect("slot");
            frame
                .write(Register(1), Value::UInt(u64::MAX))
                .expect("slot");
            frame.write(Register(0), Value::Int(3)).expect("slot");
        }
        assert_eq!(frame.heap.usage(), (0, 1));
        assert_eq!(frame.read(Register(0)), Some(Value::Int(3)));
        assert_eq!(frame.read(Register(1)), Some(Value::UInt(u64::MAX)));
    }

    #[test]
    fn immediate_and_escaped_integer_boundaries_roundtrip() {
        let mut frame = RegisterFrame::new(1);
        for value in [
            Value::Int(i64::MIN),
            Value::Int(i64::MAX),
            Value::Int(i32::MIN as i64),
            Value::Int(i32::MAX as i64),
            Value::UInt(u32::MAX as u64),
            Value::UInt(u64::MAX),
        ] {
            frame.write(Register(0), value.clone()).expect("slot");
            assert_eq!(frame.read(Register(0)), Some(value));
        }
    }

    #[test]
    fn arithmetic_crosses_inline_boundary_and_canonicalizes_nan() {
        let mut frame = RegisterFrame::new(3);
        frame
            .write(Register(0), Value::Int(i32::MAX as i64))
            .expect("slot");
        frame.write(Register(1), Value::Int(1)).expect("slot");
        frame
            .binary(Register(0), crate::BinaryOp::Add, Register(0), Register(1))
            .expect("promotes to wide integer");
        assert_eq!(
            frame.read(Register(0)),
            Some(Value::Int(i32::MAX as i64 + 1))
        );
        frame
            .binary(
                Register(0),
                crate::BinaryOp::Subtract,
                Register(0),
                Register(1),
            )
            .expect("returns to immediate");
        assert_eq!(frame.read(Register(0)), Some(Value::Int(i32::MAX as i64)));
        frame
            .write(Register(0), Value::Number(f64::INFINITY))
            .expect("slot");
        frame
            .binary(
                Register(2),
                crate::BinaryOp::Subtract,
                Register(0),
                Register(0),
            )
            .expect("IEEE NaN");
        assert!(matches!(frame.read(Register(2)), Some(Value::Number(n)) if n.is_nan()));
        frame
            .binary(
                Register(1),
                crate::BinaryOp::Equal,
                Register(2),
                Register(2),
            )
            .expect("IEEE comparison");
        assert_eq!(frame.read(Register(1)), Some(Value::Bool(false)));
    }

    #[test]
    fn interference_coloring_reuses_dead_virtual_registers() {
        let syntax = parse_program("let a = 1 + 2\nlet b = 3 + 4").expect("source parses");
        let arena = HirArena::new();
        let hir = lower_to_hir(&arena, &syntax, None).expect("HIR lowers");
        let mir = lower_hir_to_mir(&hir).expect("MIR lowers");
        let allocation = allocate_registers(&mir.entry).expect("registers allocate");
        assert!(allocation.register_count < mir.entry.virtual_register_count as u16);
    }

    #[test]
    fn frame_has_one_fixed_allocation_and_checked_access() {
        let mut frame = RegisterFrame::new(2);
        assert_eq!(frame.len(), 2);
        frame
            .write(Register(1), Value::Number(42.0))
            .expect("register exists");
        assert_eq!(frame.read(Register(1)), Some(Value::Number(42.0)));
        assert_eq!(
            frame.write(Register(2), Value::Null),
            Err(InvalidRegister(Register(2)))
        );
    }
}
