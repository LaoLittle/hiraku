//! Register allocation and fixed-size runtime frame primitives.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{MirFunction, VirtualRegister, vm::Value};

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

#[derive(Clone, Debug, PartialEq)]
pub struct RegisterFrame {
    registers: Box<[Value]>,
}

impl RegisterFrame {
    pub fn new(register_count: u16) -> Self {
        Self {
            registers: vec![Value::Uninitialized; usize::from(register_count)].into_boxed_slice(),
        }
    }

    pub fn len(&self) -> usize {
        self.registers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.registers.is_empty()
    }

    pub fn read(&self, register: Register) -> Option<&Value> {
        self.registers.get(usize::from(register.0))
    }

    pub fn write(&mut self, register: Register, value: Value) -> Result<(), InvalidRegister> {
        let Some(slot) = self.registers.get_mut(usize::from(register.0)) else {
            return Err(InvalidRegister(register));
        };
        *slot = value;
        Ok(())
    }

    pub fn values(&self) -> &[Value] {
        &self.registers
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidRegister(pub Register);

#[cfg(test)]
mod tests {
    use crate::{HirArena, lower_hir_to_mir, lower_to_hir, parse_program};

    use super::*;

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
        assert_eq!(frame.read(Register(1)), Some(&Value::Number(42.0)));
        assert_eq!(
            frame.write(Register(2), Value::Null),
            Err(InvalidRegister(Register(2)))
        );
    }
}
