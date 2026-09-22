//! Validate saved execution frames before allocating a live VM.
//! Frame metadata validation; VM restore also validates the execution-owned heap.
//! Host authority and linked callable identities remain the embedding's responsibility.
use crate::Register;
use crate::vm::{Bytecode, CodeLocation, Instruction, VmError, VmSnapshot, VmStatus};

const MAX_FRAMES: usize = 1024;
const MAX_SLOTS: usize = 1_048_576;

fn invalid(message: impl Into<String>) -> VmError {
    VmError::InvalidSnapshot(message.into())
}

fn code_at(code: &Bytecode, location: CodeLocation) -> Result<(u16, &[Instruction]), VmError> {
    match location {
        CodeLocation::Entry => Ok((code.register_count, &code.instructions)),
        CodeLocation::Function(id) => code
            .functions
            .get(id as usize)
            .map(|f| (f.register_count, f.instructions.as_slice()))
            .ok_or(VmError::UnknownFunction(id)),
        CodeLocation::Region(id) => code
            .regions
            .get(id as usize)
            .map(|f| (f.register_count, f.instructions.as_slice()))
            .ok_or(VmError::UnknownRegion(id)),
    }
}

fn validate_frame(
    code: &Bytecode,
    location: CodeLocation,
    pc: usize,
    registers: usize,
    locals: usize,
) -> Result<&[Instruction], VmError> {
    let (expected, instructions) = code_at(code, location)?;
    if registers != expected as usize || locals != code.local_count as usize {
        return Err(VmError::FrameShapeMismatch);
    }
    if pc > instructions.len() {
        return Err(VmError::InvalidProgramCounter(pc));
    }
    Ok(instructions)
}

fn validate_destination(
    instructions: &[Instruction],
    pc: usize,
    destination: Register,
    registers: usize,
) -> Result<(), VmError> {
    if destination.0 as usize >= registers {
        return Err(VmError::InvalidRegister(destination));
    }
    let previous = pc.checked_sub(1).and_then(|pc| instructions.get(pc));
    match previous {
        Some(Instruction::Call { dst, .. } | Instruction::CallValue { dst, .. })
            if *dst == destination =>
        {
            Ok(())
        }
        _ => Err(invalid(
            "saved call destination does not match its suspended call instruction",
        )),
    }
}

pub(crate) fn validate(code: &Bytecode, saved: &VmSnapshot) -> Result<(), VmError> {
    if saved.call_stack.len() >= MAX_FRAMES {
        return Err(invalid("saved call stack exceeds 1024 frames"));
    }
    let slots = saved.call_stack.iter().try_fold(
        saved
            .registers
            .len()
            .saturating_add(saved.locals.len())
            .saturating_add(saved.globals.len()),
        |count, frame| {
            count
                .checked_add(frame.registers.len())?
                .checked_add(frame.locals.len())
        },
    );
    if slots.is_none_or(|count| count > MAX_SLOTS) {
        return Err(invalid("saved frames exceed the slot budget"));
    }
    if saved.globals.len() != code.globals.len() {
        return Err(VmError::FrameShapeMismatch);
    }
    let instructions = validate_frame(
        code,
        saved.location,
        saved.pc,
        saved.registers.len(),
        saved.locals.len(),
    )?;
    match (saved.status, saved.waiting_destination) {
        (VmStatus::WaitingForHost, Some(dst)) => {
            validate_destination(instructions, saved.pc, dst, saved.registers.len())?
        }
        (VmStatus::Ready, None) => {
            if saved.pc == instructions.len() {
                return Err(VmError::InvalidProgramCounter(saved.pc));
            }
        }
        (VmStatus::Completed, None) => {
            let previous = saved.pc.checked_sub(1).and_then(|pc| instructions.get(pc));
            if !saved.call_stack.is_empty()
                || !matches!(previous, Some(Instruction::Halt | Instruction::Return(_)))
            {
                return Err(invalid(
                    "completed execution has no terminal instruction or retains callers",
                ));
            }
        }
        _ => return Err(invalid("saved wait status and destination disagree")),
    }
    for frame in &saved.call_stack {
        let instructions = validate_frame(
            code,
            frame.location,
            frame.pc,
            frame.registers.len(),
            frame.locals.len(),
        )?;
        validate_destination(
            instructions,
            frame.pc,
            frame.destination,
            frame.registers.len(),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BuiltinId, BuiltinManifest, Vm, VmEvent, compile_with_manifest, parse_program};

    fn program() -> Bytecode {
        compile_with_manifest(&parse_program("fn child() -> Int { 7 }\nfn parent() -> Int { child() }\nglobal let result = parent()")
            .expect("parse"), 42, &BuiltinManifest::new(Vec::<(String, BuiltinId)>::new())).expect("compile")
    }

    fn suspended(code: &Bytecode) -> VmSnapshot {
        let mut vm = Vm::new(code.clone()).expect("VM");
        for _ in 0..100 {
            vm.step_with_budget(&mut 1).expect("step");
            let snapshot = vm.snapshot();
            if !snapshot.call_stack.is_empty() {
                return snapshot;
            }
        }
        panic!("expected saved caller");
    }

    #[test]
    fn legitimate_snapshots_restore_at_every_instruction() {
        let code = program();
        let mut vm = Vm::new(code.clone()).expect("VM");
        for _ in 0..100 {
            let event = vm.step_with_budget(&mut 1).expect("execute");
            vm = Vm::restore(code.clone(), vm.snapshot()).expect("restore checkpoint");
            if matches!(event, Some(VmEvent::Completed(_))) {
                assert_eq!(vm.global("result"), Some(crate::Value::Int(7)));
                return;
            }
        }
        panic!("expected completion");
    }

    #[test]
    fn malformed_callers_are_rejected_before_activation() {
        let code = program();
        let saved = suspended(&code);
        let mutations: &[fn(&mut VmSnapshot)] = &[
            |s| s.call_stack[0].registers.clear(),
            |s| s.call_stack[0].locals.push(crate::Value::Unit),
            |s| s.call_stack[0].location = CodeLocation::Function(u32::MAX),
            |s| s.call_stack[0].pc = usize::MAX,
            |s| s.call_stack[0].pc = 0,
            |s| s.call_stack[0].destination = Register(u16::MAX),
            |s| s.pc = usize::MAX,
            |s| s.status = VmStatus::WaitingForHost,
            |s| s.waiting_destination = Some(Register(0)),
            |s| s.status = VmStatus::Completed,
            |s| s.call_stack = vec![s.call_stack[0].clone(); MAX_FRAMES],
        ];
        for mutate in mutations {
            let mut invalid = saved.clone();
            mutate(&mut invalid);
            assert!(Vm::restore(code.clone(), invalid).is_err());
        }
    }

    #[test]
    fn wait_destination_must_match_the_call_site() {
        let mut code = program();
        code.register_count = 2;
        code.instructions = vec![
            Instruction::CallValue {
                dst: Register(0),
                callee: Register(1),
                labels: vec![],
                arguments: crate::vm::RegisterSlice {
                    start: Register(0),
                    count: 0,
                },
            },
            Instruction::Halt,
        ];
        let mut saved = Vm::new(code.clone()).expect("VM").snapshot();
        saved.pc = 1;
        saved.status = VmStatus::WaitingForHost;
        saved.waiting_destination = Some(Register(0));
        assert!(Vm::restore(code.clone(), saved.clone()).is_ok());
        saved.waiting_destination = Some(Register(1));
        assert!(matches!(
            Vm::restore(code, saved),
            Err(VmError::InvalidSnapshot(_))
        ));
    }

    #[test]
    fn restore_rejects_dangling_values_in_active_and_saved_frames() {
        let code = program();
        let saved = suspended(&code);
        let mutations: &[fn(&mut VmSnapshot)] = &[
            |s| s.registers[0] = crate::Value::Object(crate::ObjectId(42)),
            |s| s.globals[0] = crate::Value::Object(crate::ObjectId(42)),
            |s| s.call_stack[0].registers[0] = crate::Value::Object(crate::ObjectId(42)),
        ];
        for mutate in mutations {
            let mut invalid = saved.clone();
            mutate(&mut invalid);
            assert!(matches!(
                Vm::restore(code.clone(), invalid),
                Err(VmError::InvalidObject(_))
            ));
        }
    }

    #[test]
    fn shared_frame_restore_requires_the_explicit_correct_heap() {
        let code = program();
        let mut saved = suspended(&code);
        let mut heap = crate::ObjectHeap::default();
        saved.globals[0] = heap.import(crate::Value::Map(std::collections::BTreeMap::from([(
            "name".into(),
            crate::Value::String("Alice".into()),
        )])));
        assert!(matches!(
            Vm::restore(code.clone(), saved.clone()),
            Err(VmError::InvalidObject(_))
        ));
        Vm::restore_with_shared_heap(code.clone(), saved.clone(), &heap)
            .expect("explicit shared ownership");
        saved.objects = heap.clone();
        Vm::restore(code.clone(), saved.clone()).expect("private ownership");
        assert!(matches!(
            Vm::restore_with_shared_heap(code, saved, &heap),
            Err(VmError::InvalidSnapshot(_))
        ));
    }
}
