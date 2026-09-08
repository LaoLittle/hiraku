use hiraku_script::native::{NativeError, NativeRegistry};
use hiraku_script::{LinkedProgram, LinkedVm, LinkedVmEvent, Value};
use std::collections::BTreeMap;

/// One-way property computation. Not a script-visible value and not serialized:
/// the mounted component recompiles/recreates it. Writes belong to callbacks.
#[derive(Clone, Debug)]
pub struct PropertyComputation {
    pub program: LinkedProgram,
    pub getter: Value,
    pub globals: BTreeMap<String, Value>,
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
