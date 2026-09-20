use std::collections::{BTreeMap, BTreeSet};

use hiraku_script::native::{NativeError, NativeRegistry};
use hiraku_script::{LinkedVm, LinkedVmEvent, StatementValue, Value};

#[derive(Debug, thiserror::Error)]
pub enum CompositionError {
    #[error("{0}")]
    Vm(String),
    #[error("{0}")]
    Native(#[from] NativeError),
    #[error("UI invocation exceeded its instruction budget")]
    BudgetExceeded,
    #[error("bare strings are not UI nodes; wrap the value with text(...)")]
    BareString,
    #[error("UI VM stopped without completing or requesting a native call")]
    MissingCompletion,
}

/// Execute one composition invocation using the ordinary linked VM.
/// The host owns node drafts and seals them at statement boundaries. Return
/// values are not implicitly emitted as nodes. Only mount-owned globals can
/// be written; the resulting state is published after successful completion.
pub fn compose<C>(
    mut vm: LinkedVm,
    registry: &NativeRegistry<C>,
    context: &mut C,
    globals: &BTreeMap<String, Value>,
    owned_globals: &BTreeSet<String>,
    mut budget: u32,
    mut commit: impl FnMut(&mut C),
) -> Result<BTreeMap<String, Value>, CompositionError> {
    vm.set_current_globals(globals)
        .map_err(|error| CompositionError::Vm(error.to_string()))?;
    vm.set_read_only_globals(
        globals
            .keys()
            .filter(|name| !owned_globals.contains(*name))
            .cloned()
            .collect(),
    );
    loop {
        let event = vm.step_with_budget(&mut budget).map_err(|error| {
            if matches!(
                error,
                hiraku_script::LinkedVmError::Vm(hiraku_script::VmError::Panic { .. })
            ) {
                return CompositionError::Vm(error.to_string());
            }
            let snapshot = vm.snapshot();
            CompositionError::Vm(match snapshot.frames.last() {
                Some(frame) => format!(
                    "{error} in module {} at {:?}:{}",
                    frame.module.0,
                    frame.vm.location,
                    frame.vm.pc.saturating_sub(1)
                ),
                None => error.to_string(),
            })
        })?;
        match event {
            Some(LinkedVmEvent::BudgetExhausted) => return Err(CompositionError::BudgetExceeded),
            Some(LinkedVmEvent::Call(call)) => {
                let value = registry.call(context, &call)?;
                vm.resume(value)
                    .map_err(|error| CompositionError::Vm(error.to_string()))?;
            }
            Some(LinkedVmEvent::Statement(StatementValue::Commit | StatementValue::Value(_))) => {
                commit(context)
            }
            Some(LinkedVmEvent::Statement(
                StatementValue::String(_) | StatementValue::TextTemplate(_),
            )) => return Err(CompositionError::BareString),
            Some(LinkedVmEvent::Completed(_)) => {
                // Helper function tails may complete without a statement event.
                commit(context);
                return Ok(vm
                    .current_globals()
                    .map_err(|error| CompositionError::Vm(error.to_string()))?
                    .into_iter()
                    .filter(|(name, value)| {
                        owned_globals.contains(name) && *value != Value::Uninitialized
                    })
                    .collect());
            }
            None => return Err(CompositionError::MissingCompletion),
        }
    }
}
