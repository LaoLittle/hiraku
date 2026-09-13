use hiraku_script::native::{NativeError, NativeRegistry};
use hiraku_script::{LinkedProgram, LinkedVm, LinkedVmEvent, Value};
use std::collections::{BTreeMap, BTreeSet};

/// One-way property computation. Not a script-visible value and not serialized:
/// the mounted component recompiles/recreates it. Writes belong to callbacks.
#[derive(Clone, Debug)]
pub struct PropertyComputation {
    pub program: LinkedProgram,
    pub getter: Value,
    pub globals: BTreeMap<String, Value>,
    /// Conservative transitive reads of this callable, not the whole document.
    pub dependencies: BTreeSet<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum PropertyError {
    #[error("property VM failed: {0}")]
    Vm(hiraku_script::linked_vm::LinkedVmError),
    #[error("property native call failed: {0}")]
    Native(#[from] NativeError),
    #[error("UI property exceeded its instruction budget")]
    BudgetExceeded,
    #[error("UI property stopped without returning a value")]
    MissingReturn,
}

impl From<hiraku_script::linked_vm::LinkedVmError> for PropertyError {
    fn from(error: hiraku_script::linked_vm::LinkedVmError) -> Self {
        Self::Vm(error)
    }
}

impl PropertyComputation {
    pub fn new(
        program: LinkedProgram,
        getter: Value,
        mut globals: BTreeMap<String, Value>,
    ) -> Self {
        let mut reads = BTreeSet::new();
        let mut visited = BTreeSet::new();
        let mut pending = vec![getter.clone()];
        let mut opaque = false;
        while let Some(callable) = pending.pop() {
            use hiraku_script::{
                linker::{LinkedFunction, ModuleId},
                vm::{Constant, Instruction},
            };
            let (module, region, index) = match callable {
                Value::Closure { module, region, .. } => (module.unwrap_or(0), true, region),
                Value::Function { module, symbol } => {
                    let Some(owner) = program.modules.get(module.unwrap_or(0) as usize) else {
                        opaque = true;
                        continue;
                    };
                    match owner.resolve(symbol) {
                        Some(LinkedFunction::Script {
                            module: ModuleId(module),
                            function,
                        }) => (module, false, function),
                        _ => {
                            opaque = true;
                            continue;
                        }
                    }
                }
                _ => {
                    opaque = true;
                    continue;
                }
            };
            if !visited.insert((module, region, index)) {
                continue;
            }
            let Some(owner) = program.modules.get(module as usize) else {
                opaque = true;
                continue;
            };
            let code = &owner.bytecode;
            let instructions = if region {
                code.regions.get(index as usize).map(|r| &r.instructions)
            } else {
                code.functions.get(index as usize).map(|f| &f.instructions)
            };
            let Some(instructions) = instructions else {
                opaque = true;
                continue;
            };
            for instruction in instructions {
                match instruction {
                    Instruction::LoadGlobal { global, .. }
                    | Instruction::GlobalInitialized { global, .. } => {
                        if let Some(name) = code
                            .globals
                            .get(*global as usize)
                            .and_then(|s| code.symbols.resolve(*s))
                        {
                            reads.insert(name.to_owned());
                        }
                    }
                    Instruction::Call { function, .. }
                    | Instruction::Constant {
                        value: Constant::Function(function),
                        ..
                    } => {
                        pending.push(Value::Function {
                            module: Some(module),
                            symbol: *function,
                        });
                    }
                    Instruction::MakeClosure { region, .. } => pending.push(Value::Closure {
                        module: Some(module),
                        region: *region,
                        captures: vec![],
                        objects: None,
                        type_bindings: vec![],
                    }),
                    // A dynamic callee or native host may read inputs not visible in bytecode.
                    Instruction::CallValue { .. } => opaque = true,
                    _ => {}
                }
            }
        }
        if opaque {
            reads.extend(globals.keys().cloned());
        }
        globals.retain(|name, _| reads.contains(name));
        Self {
            program,
            getter,
            globals,
            dependencies: reads,
        }
    }

    /// Runs the ordinary script VM with read-only captured inputs. The host
    /// controls which native capabilities are available during evaluation.
    pub fn evaluate<C>(
        &self,
        registry: &NativeRegistry<C>,
        context: &mut C,
        mut budget: u32,
    ) -> Result<Value, PropertyError> {
        let mut vm = LinkedVm::from_callable(self.program.clone(), &self.getter, Vec::new())?;
        vm.set_current_globals(&self.globals)?;
        vm.set_read_only_globals(self.globals.keys().cloned().collect());
        vm.freeze_invocation_inputs()?;
        loop {
            match vm.step_with_budget(&mut budget)? {
                Some(LinkedVmEvent::BudgetExhausted) => return Err(PropertyError::BudgetExceeded),
                Some(LinkedVmEvent::Call(call)) => {
                    vm.resume(registry.call(context, &call)?)?;
                }
                Some(LinkedVmEvent::Completed(value)) => return Ok(value),
                Some(LinkedVmEvent::Statement(_)) => {}
                None => return Err(PropertyError::MissingReturn),
            }
        }
    }
}
