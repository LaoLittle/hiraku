//! Conservative, bounded inlining of annotated straight-line value functions.
//! Argument evaluation stays in the caller; only already-computed registers
//! are substituted. Unsupported bodies retain an ordinary call.
use super::*;
use std::collections::BTreeMap;

pub(super) fn run(program: &mut MirProgram, hir: &HirProgram<'_>) {
    // Bottom-up wrappers can become eligible after a leaf was inlined. Bound
    // optimization work and code growth rather than attempting recursive expansion.
    for _ in 0..4 {
        let candidates = program
            .functions
            .iter()
            .enumerate()
            .filter_map(|(index, function)| {
                let declaration = &hir.functions[index];
                (declaration.inline && !declaration.track_caller && eligible(function))
                    .then(|| (crate::HirFunctionId(index as u32), function.clone()))
            })
            .collect::<BTreeMap<_, _>>();
        rewrite(&mut program.entry, &candidates);
        for function in &mut program.functions {
            rewrite(function, &candidates);
        }
    }
}

fn eligible(function: &MirFunction) -> bool {
    if function.blocks.len() != 1
        || !function.regions.is_empty()
        || matches!(
            function.signature.result,
            crate::ScriptType::Unit | crate::ScriptType::Never
        )
        || !matches!(
            function.blocks[0].terminator,
            MirTerminator::Return(Some(_))
        )
        || function.blocks[0].instructions.len() > 24
    {
        return false;
    }
    function.blocks[0]
        .instructions
        .iter()
        .all(|instruction| match instruction {
            MirInstruction::LoadLocal { local, .. } => function.parameters.contains(local),
            MirInstruction::Constant { .. }
            | MirInstruction::Move { .. }
            | MirInstruction::UnaryMinus { .. }
            | MirInstruction::Binary { .. }
            | MirInstruction::ToString { .. } => true,
            _ => false,
        })
}

fn rewrite(caller: &mut MirFunction, candidates: &BTreeMap<crate::HirFunctionId, MirFunction>) {
    // Keep pathological generated scripts from exceeding the VM register limit.
    const REGISTER_BUDGET: u32 = 4096;
    for block in &mut caller.blocks {
        let mut output = Vec::new();
        for instruction in std::mem::take(&mut block.instructions) {
            let MirInstruction::Call {
                function: ResolvedFunction::User(id),
                type_bindings,
                receiver: None,
                arguments,
                dst,
                ..
            } = &instruction
            else {
                output.push(instruction);
                continue;
            };
            let Some(function) = candidates.get(id) else {
                output.push(instruction);
                continue;
            };
            if !type_bindings.is_empty()
                || function.parameters.len() != arguments.len()
                || caller
                    .virtual_register_count
                    .saturating_add(function.virtual_register_count)
                    > REGISTER_BUDGET
            {
                output.push(instruction);
                continue;
            }
            let base = caller.virtual_register_count;
            caller.virtual_register_count += function.virtual_register_count;
            let reg = |value: VirtualRegister| VirtualRegister(base + value.0);
            for operation in &function.blocks[0].instructions {
                let operation = match operation {
                    MirInstruction::LoadLocal { dst, local } => {
                        let index = function
                            .parameters
                            .iter()
                            .position(|parameter| parameter == local)
                            .expect("inline eligibility checked parameter");
                        MirInstruction::Move {
                            dst: reg(*dst),
                            src: arguments[index].1,
                        }
                    }
                    MirInstruction::Constant { dst, value } => MirInstruction::Constant {
                        dst: reg(*dst),
                        value: value.clone(),
                    },
                    MirInstruction::Move { dst, src } => MirInstruction::Move {
                        dst: reg(*dst),
                        src: reg(*src),
                    },
                    MirInstruction::UnaryMinus { dst, value } => MirInstruction::UnaryMinus {
                        dst: reg(*dst),
                        value: reg(*value),
                    },
                    MirInstruction::ToString { dst, value } => MirInstruction::ToString {
                        dst: reg(*dst),
                        value: reg(*value),
                    },
                    MirInstruction::Binary {
                        dst,
                        op,
                        left,
                        right,
                    } => MirInstruction::Binary {
                        dst: reg(*dst),
                        op: *op,
                        left: reg(*left),
                        right: reg(*right),
                    },
                    _ => unreachable!("inline eligibility excludes effectful instructions"),
                };
                output.push(operation);
            }
            let MirTerminator::Return(Some(value)) = function.blocks[0].terminator else {
                unreachable!("inline eligibility checked return")
            };
            output.push(MirInstruction::Move {
                dst: *dst,
                src: reg(value),
            });
        }
        block.instructions = output;
    }
    for region in &mut caller.regions {
        rewrite(region, candidates);
    }
}
