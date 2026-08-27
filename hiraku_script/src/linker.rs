//! Runtime linking for symbolic register bytecode calls.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::{
    SymbolId,
    vm::{RegisterBytecode, RegisterInstruction},
    runtime::{BuiltinId, BuiltinManifest},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ModuleId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkedFunction {
    Native(BuiltinId),
    Script { module: ModuleId, function: u32 },
}

#[derive(Clone, Debug)]
pub struct LinkedModule {
    pub id: ModuleId,
    pub bytecode: RegisterBytecode,
    calls: BTreeMap<SymbolId, LinkedFunction>,
}

impl LinkedModule {
    pub fn resolve(&self, symbol: SymbolId) -> Option<LinkedFunction> {
        self.calls.get(&symbol).copied()
    }

    pub fn calls(&self) -> &BTreeMap<SymbolId, LinkedFunction> {
        &self.calls
    }
}

pub type LinkedBytecode = LinkedModule;

#[derive(Clone, Debug)]
pub struct LinkedProgram {
    pub modules: Vec<LinkedModule>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkError {
    pub module: ModuleId,
    pub symbol: Option<SymbolId>,
    pub message: String,
}

pub fn link_register_bytecode(
    bytecode: RegisterBytecode,
    natives: &BuiltinManifest,
) -> Result<LinkedBytecode, Vec<LinkError>> {
    link_register_modules(vec![bytecode], natives).map(|mut program| program.modules.remove(0))
}

pub fn link_register_modules(
    modules: Vec<RegisterBytecode>,
    natives: &BuiltinManifest,
) -> Result<LinkedProgram, Vec<LinkError>> {
    let mut exports = BTreeMap::<String, LinkedFunction>::new();
    let mut errors = Vec::new();
    for (module_index, module) in modules.iter().enumerate() {
        let module_id = ModuleId(module_index as u32);
        for (function_index, function) in module.functions.iter().enumerate() {
            if !function.exported {
                continue;
            }
            let Some(name) = module.symbols.resolve(function.name) else {
                errors.push(LinkError {
                    module: module_id,
                    symbol: Some(function.name),
                    message: "exported function has an unknown symbol".into(),
                });
                continue;
            };
            let target = LinkedFunction::Script {
                module: module_id,
                function: function_index as u32,
            };
            if exports.insert(name.to_string(), target).is_some() {
                errors.push(LinkError {
                    module: module_id,
                    symbol: Some(function.name),
                    message: format!("global function `{name}` is exported more than once"),
                });
            }
        }
    }

    let mut linked_modules = Vec::with_capacity(modules.len());
    for (module_index, bytecode) in modules.into_iter().enumerate() {
        let module_id = ModuleId(module_index as u32);
        let local_functions = bytecode
            .functions
            .iter()
            .enumerate()
            .filter_map(|(index, function)| {
                bytecode.symbols.resolve(function.name).map(|name| {
                    (
                        name.to_string(),
                        LinkedFunction::Script {
                            module: module_id,
                            function: index as u32,
                        },
                    )
                })
            })
            .collect::<BTreeMap<_, _>>();
        let mut calls = BTreeMap::new();
        for instruction in bytecode
            .instructions
            .iter()
            .chain(
                bytecode
                    .functions
                    .iter()
                    .flat_map(|function| &function.instructions),
            )
            .chain(
                bytecode
                    .regions
                    .iter()
                    .flat_map(|region| &region.instructions),
            )
        {
            let RegisterInstruction::Call { function, .. } = instruction else {
                continue;
            };
            if calls.contains_key(function) {
                continue;
            }
            let Some(name) = bytecode.symbols.resolve(*function) else {
                errors.push(LinkError {
                    module: module_id,
                    symbol: Some(*function),
                    message: format!("call references unknown symbol {:?}", function),
                });
                continue;
            };
            let target = local_functions
                .get(name)
                .or_else(|| exports.get(name))
                .copied()
                .or_else(|| {
                    natives
                        .callable_name_candidates()
                        .find_map(|(candidate, builtin)| {
                            (candidate == name).then_some(LinkedFunction::Native(builtin))
                        })
                });
            match target {
                Some(target) => {
                    calls.insert(*function, target);
                }
                None => errors.push(LinkError {
                    module: module_id,
                    symbol: Some(*function),
                    message: format!("function `{name}` has no script or native implementation"),
                }),
            }
        }
        linked_modules.push(LinkedModule {
            id: module_id,
            bytecode,
            calls,
        });
    }
    if errors.is_empty() {
        Ok(LinkedProgram {
            modules: linked_modules,
        })
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use crate::{BuiltinManifest, compile_register_with_manifest, parse_program};

    use super::*;

    fn compile(source: &str) -> RegisterBytecode {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        compile_register_with_manifest(
            &parse_program(source).expect("source parses"),
            17,
            &manifest,
        )
        .expect("source compiles")
    }

    #[test]
    fn global_functions_link_across_modules() {
        let provider = compile("global fn greet(name: String) { name }");
        let consumer = compile("greet(\"alice\")");
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let linked = link_register_modules(vec![provider, consumer], &manifest)
            .expect("global function must link");
        let consumer = &linked.modules[1];
        let symbol = consumer
            .bytecode
            .symbols
            .find("greet")
            .expect("consumer symbol exists");
        assert_eq!(
            consumer.resolve(symbol),
            Some(LinkedFunction::Script {
                module: ModuleId(0),
                function: 0,
            })
        );
    }

    #[test]
    fn unresolved_function_is_reported_during_linking() {
        let module = compile("missingFunction()");
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let errors = link_register_modules(vec![module], &manifest)
            .expect_err("missing implementation must fail linking");
        assert!(errors[0].message.contains("missingFunction"));
    }
}
