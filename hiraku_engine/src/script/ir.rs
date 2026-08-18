use std::{collections::BTreeMap, sync::mpsc};

use bevy::prelude::Resource;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub type IrPc = u32;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct IrProgram {
    pub version: u16,
    pub source_hash: u64,
    pub expressions: Vec<IrExpression>,
    pub instructions: Vec<IrInstruction>,
}

impl IrProgram {
    pub const CURRENT_VERSION: u16 = 1;

    pub fn new(source_hash: u64, instructions: Vec<IrInstruction>) -> Self {
        Self {
            version: Self::CURRENT_VERSION,
            source_hash,
            expressions: Vec::new(),
            instructions,
        }
    }

    pub fn with_expressions(
        source_hash: u64,
        expressions: Vec<IrExpression>,
        instructions: Vec<IrInstruction>,
    ) -> Self {
        Self {
            version: Self::CURRENT_VERSION,
            source_hash,
            expressions,
            instructions,
        }
    }

    pub fn validate(&self) -> Result<(), IrValidationError> {
        if self.version != Self::CURRENT_VERSION {
            return Err(IrValidationError::UnsupportedVersion(self.version));
        }
        let length = self.instructions.len() as IrPc;
        for (pc, instruction) in self.instructions.iter().enumerate() {
            let pc = pc as IrPc;
            match instruction {
                IrInstruction::Jump(target) => validate_target(pc, *target, length)?,
                IrInstruction::Branch {
                    expression,
                    then_pc,
                    else_pc,
                } => {
                    if expression.0 as usize >= self.expressions.len() {
                        return Err(IrValidationError::InvalidExpression {
                            pc,
                            expression: *expression,
                            length: self.expressions.len() as u32,
                        });
                    }
                    validate_target(pc, *then_pc, length)?;
                    validate_target(pc, *else_pc, length)?;
                }
                IrInstruction::Emit(_) | IrInstruction::Wait(_) | IrInstruction::Halt => {}
            }
        }
        Ok(())
    }
}

fn validate_target(pc: IrPc, target: IrPc, length: IrPc) -> Result<(), IrValidationError> {
    if target >= length {
        return Err(IrValidationError::InvalidTarget { pc, target, length });
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum IrInstruction {
    Emit(IrCommand),
    Wait(IrWaitKind),
    Jump(IrPc),
    Branch {
        expression: IrExpressionId,
        then_pc: IrPc,
        else_pc: IrPc,
    },
    Halt,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum IrExpression {
    BoolLiteral(bool),
    BoolVariable(String),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum IrCommand {
    Log(String),
    ClearDialogue,
    Say {
        speaker: String,
        text: String,
    },
    StopBgm,
    ReturnToTitle,
    SetBackground {
        texture: String,
    },
    ShowCharacter {
        actor_id: String,
        character_name: String,
        expressions: Vec<String>,
        position: [f32; 2],
        scale: f32,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum IrWaitKind {
    DialogueAdvance,
    ScreenChoice,
    DurationMs(u64),
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct IrExpressionId(pub u32);

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum IrVmStatus {
    Ready,
    Waiting(IrWaitKind),
    Halted,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct IrVmSnapshot {
    pub source_hash: u64,
    pub pc: IrPc,
    pub status: IrVmStatus,
    pub expressions: BTreeMap<IrExpressionId, bool>,
    pub variables: BTreeMap<String, bool>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum IrEvent {
    Command(IrCommand),
    Waiting(IrWaitKind),
    Completed,
}

#[derive(Clone, Debug)]
pub struct IrVm {
    program: IrProgram,
    pc: IrPc,
    status: IrVmStatus,
    expressions: BTreeMap<IrExpressionId, bool>,
    variables: BTreeMap<String, bool>,
}

impl IrVm {
    pub fn new(program: IrProgram) -> Result<Self, IrValidationError> {
        program.validate()?;
        Ok(Self {
            program,
            pc: 0,
            status: IrVmStatus::Ready,
            expressions: BTreeMap::new(),
            variables: BTreeMap::new(),
        })
    }

    pub fn set_expression(&mut self, expression: IrExpressionId, value: bool) {
        self.expressions.insert(expression, value);
    }

    pub fn set_bool_variable(&mut self, name: impl Into<String>, value: bool) {
        self.variables.insert(name.into(), value);
    }

    pub fn pc(&self) -> IrPc {
        self.pc
    }

    pub fn status(&self) -> &IrVmStatus {
        &self.status
    }

    pub fn resume(&mut self) -> bool {
        if matches!(self.status, IrVmStatus::Waiting(_)) {
            self.status = IrVmStatus::Ready;
            true
        } else {
            false
        }
    }

    pub fn step(&mut self) -> Option<IrEvent> {
        if !matches!(self.status, IrVmStatus::Ready) {
            return None;
        }

        loop {
            let instruction = self.program.instructions.get(self.pc as usize)?.clone();
            match instruction {
                IrInstruction::Emit(command) => {
                    self.pc += 1;
                    return Some(IrEvent::Command(command));
                }
                IrInstruction::Wait(wait) => {
                    self.pc += 1;
                    self.status = IrVmStatus::Waiting(wait.clone());
                    return Some(IrEvent::Waiting(wait));
                }
                IrInstruction::Jump(target) => self.pc = target,
                IrInstruction::Branch {
                    expression,
                    then_pc,
                    else_pc,
                } => {
                    self.pc = if self.expression_value(expression) {
                        then_pc
                    } else {
                        else_pc
                    };
                }
                IrInstruction::Halt => {
                    self.status = IrVmStatus::Halted;
                    return Some(IrEvent::Completed);
                }
            }
        }
    }

    pub fn snapshot(&self) -> IrVmSnapshot {
        IrVmSnapshot {
            source_hash: self.program.source_hash,
            pc: self.pc,
            status: self.status.clone(),
            expressions: self.expressions.clone(),
            variables: self.variables.clone(),
        }
    }

    pub fn restore(program: IrProgram, snapshot: IrVmSnapshot) -> Result<Self, IrValidationError> {
        if program.source_hash != snapshot.source_hash {
            return Err(IrValidationError::SourceHashMismatch {
                expected: program.source_hash,
                actual: snapshot.source_hash,
            });
        }
        program.validate()?;
        if snapshot.pc >= program.instructions.len() as IrPc {
            return Err(IrValidationError::InvalidSnapshotPc(snapshot.pc));
        }
        Ok(Self {
            program,
            pc: snapshot.pc,
            status: snapshot.status,
            expressions: snapshot.expressions,
            variables: snapshot.variables,
        })
    }

    fn expression_value(&self, expression: IrExpressionId) -> bool {
        if let Some(value) = self.expressions.get(&expression) {
            return *value;
        }
        match self.program.expressions.get(expression.0 as usize) {
            Some(IrExpression::BoolLiteral(value)) => *value,
            Some(IrExpression::BoolVariable(name)) => {
                self.variables.get(name).copied().unwrap_or(false)
            }
            None => false,
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum IrValidationError {
    #[error("unsupported IR version {0}")]
    UnsupportedVersion(u16),
    #[error("instruction {pc} jumps to {target}, outside program length {length}")]
    InvalidTarget {
        pc: IrPc,
        target: IrPc,
        length: IrPc,
    },
    #[error("IR snapshot points to invalid program counter {0}")]
    InvalidSnapshotPc(IrPc),
    #[error(
        "instruction {pc} references expression {expression:?}, outside expression length {length}"
    )]
    InvalidExpression {
        pc: IrPc,
        expression: IrExpressionId,
        length: u32,
    },
    #[error("IR source hash mismatch: expected {expected}, got {actual}")]
    SourceHashMismatch { expected: u64, actual: u64 },
}

#[derive(Default, Resource)]
pub struct IrRuntime {
    pub vm: Option<IrVm>,
    pub events: Vec<IrEvent>,
    pub wait_response:
        Option<std::sync::Arc<std::sync::Mutex<mpsc::Receiver<super::ScriptResponse>>>>,
}

pub fn tick_ir_runtime(mut runtime: bevy::prelude::ResMut<IrRuntime>) {
    let Some(vm) = runtime.vm.as_mut() else {
        return;
    };
    if let Some(event) = vm.step() {
        runtime.events.push(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executes_commands_and_waits_at_stable_program_counters() {
        let program = IrProgram::new(
            42,
            vec![
                IrInstruction::Emit(IrCommand::Log("before".to_string())),
                IrInstruction::Wait(IrWaitKind::DialogueAdvance),
                IrInstruction::Emit(IrCommand::Log("after".to_string())),
                IrInstruction::Halt,
            ],
        );
        let mut vm = IrVm::new(program).unwrap();

        assert_eq!(
            vm.step(),
            Some(IrEvent::Command(IrCommand::Log("before".to_string())))
        );
        assert_eq!(vm.pc(), 1);
        assert_eq!(
            vm.step(),
            Some(IrEvent::Waiting(IrWaitKind::DialogueAdvance))
        );
        assert_eq!(vm.pc(), 2);
        assert!(vm.step().is_none());
        assert!(vm.resume());
        assert_eq!(
            vm.step(),
            Some(IrEvent::Command(IrCommand::Log("after".to_string())))
        );
        assert_eq!(vm.step(), Some(IrEvent::Completed));
    }

    #[test]
    fn branch_and_snapshot_restore_are_deterministic() {
        let program = IrProgram::with_expressions(
            7,
            vec![
                IrExpression::BoolLiteral(false),
                IrExpression::BoolLiteral(false),
            ],
            vec![
                IrInstruction::Branch {
                    expression: IrExpressionId(1),
                    then_pc: 1,
                    else_pc: 2,
                },
                IrInstruction::Emit(IrCommand::Log("yes".to_string())),
                IrInstruction::Emit(IrCommand::Log("no".to_string())),
                IrInstruction::Halt,
            ],
        );
        let mut vm = IrVm::new(program.clone()).unwrap();
        vm.set_expression(IrExpressionId(1), true);
        assert_eq!(
            vm.step(),
            Some(IrEvent::Command(IrCommand::Log("yes".to_string())))
        );

        let snapshot = vm.snapshot();
        let restored = IrVm::restore(program, snapshot).unwrap();
        assert_eq!(restored.pc(), 2);
    }

    #[test]
    fn rejects_invalid_jump_targets() {
        let program = IrProgram::new(1, vec![IrInstruction::Jump(4)]);
        assert_eq!(
            IrVm::new(program).unwrap_err(),
            IrValidationError::InvalidTarget {
                pc: 0,
                target: 4,
                length: 1,
            }
        );
    }
}
