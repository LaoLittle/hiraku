//! Runtime linking for symbolic register bytecode calls.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::{
    SymbolId,
    runtime::{BuiltinId, BuiltinManifest},
    vm::{Bytecode, Instruction},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ModuleId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinkedFunction {
    Native(BuiltinId),
    Script { module: ModuleId, function: u32 },
}

#[derive(Clone, Debug)]
pub struct LinkedModule {
    pub fingerprint: crate::ProgramFingerprint,
    pub id: ModuleId,
    pub bytecode: Arc<Bytecode>,
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
    pub modules: Arc<[LinkedModule]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkError {
    pub module: ModuleId,
    pub symbol: Option<SymbolId>,
    pub message: String,
}

/// Host-owned grants. Deliberately not serializable: restore must re-authorize.
#[derive(Clone, Debug, Default)]
pub struct LinkPolicy {
    grants: BTreeMap<ModuleId, BTreeSet<String>>,
}
impl LinkPolicy {
    pub fn grant(&mut self, module: ModuleId, capability: impl Into<String>) {
        self.grants
            .entry(module)
            .or_default()
            .insert(capability.into());
    }
    pub fn allows(&self, module: ModuleId, capability: &str) -> bool {
        self.grants
            .get(&module)
            .is_some_and(|grants| grants.contains(capability))
    }
}

pub fn link_bytecode(
    bytecode: Bytecode,
    natives: &BuiltinManifest,
) -> Result<LinkedBytecode, Vec<LinkError>> {
    link_register_modules(vec![bytecode], natives).map(|program| program.modules[0].clone())
}

pub fn link_register_modules(
    modules: Vec<Bytecode>,
    natives: &BuiltinManifest,
) -> Result<LinkedProgram, Vec<LinkError>> {
    link_named_modules(
        modules.into_iter().map(|module| (None, module)).collect(),
        natives,
    )
}

/// Links modules whose exported functions live under explicit namespaces.
/// Local calls inside a provider remain unqualified, while consumers address
/// exports through names such as `ui.widgets.button`.
pub fn link_named_modules(
    modules: Vec<(Option<String>, Bytecode)>,
    natives: &BuiltinManifest,
) -> Result<LinkedProgram, Vec<LinkError>> {
    link_named_modules_with_policy(modules, natives, &LinkPolicy::default())
}

/// Authorization is checked against each function's defining module, not its caller.
pub fn link_named_modules_with_policy(
    modules: Vec<(Option<String>, Bytecode)>,
    natives: &BuiltinManifest,
    policy: &LinkPolicy,
) -> Result<LinkedProgram, Vec<LinkError>> {
    let mut exports = BTreeMap::<String, LinkedFunction>::new();
    let manifests = modules
        .iter()
        .map(|(_, module)| module.symbols.clone())
        .collect::<Vec<_>>();
    let mut type_symbols = crate::SymbolInterner::default();
    let signatures = modules
        .iter()
        .map(|(_, module)| {
            module
                .functions
                .iter()
                .map(|function| function.signature.clone())
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let mut errors = Vec::new();
    for (module_index, (namespace, module)) in modules.iter().enumerate() {
        let module_id = ModuleId(module_index as u32);
        for (function_index, function) in module.functions.iter().enumerate() {
            let local_name = module.symbols.resolve(function.name).unwrap_or("");
            let qualified = namespace
                .as_ref()
                .map(|ns| format!("{ns}.{local_name}"))
                .unwrap_or_else(|| local_name.to_string());
            if qualified == "intrinsics"
                || qualified.starts_with("intrinsics.")
                || local_name.starts_with("intrinsics.")
            {
                errors.push(LinkError {
                    module: module_id,
                    symbol: Some(function.name),
                    message: "the intrinsics namespace is reserved for host-provided functions"
                        .into(),
                });
            }
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
            let export_name = namespace
                .as_ref()
                .map(|namespace| format!("{namespace}.{name}"))
                .unwrap_or_else(|| name.to_string());
            if exports.insert(export_name.clone(), target).is_some() {
                errors.push(LinkError {
                    module: module_id,
                    symbol: Some(function.name),
                    message: format!("global function `{export_name}` is exported more than once"),
                });
            }
        }
    }

    let mut linked_modules = Vec::with_capacity(modules.len());
    for (module_index, (_, bytecode)) in modules.into_iter().enumerate() {
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
            // A direct intrinsic reference could otherwise escape through a returned
            // function value. Standard modules must export script wrappers instead.
            if let Instruction::Constant {
                value: crate::vm::Constant::Function(symbol),
                ..
            } = instruction
            {
                if let Some(name) = bytecode.symbols.resolve(*symbol) {
                    if let Some((_, builtin)) = natives
                        .callable_name_candidates()
                        .find(|(candidate, _)| candidate == name)
                    {
                        if natives.required_capability(builtin).is_some() {
                            errors.push(LinkError {module: module_id, symbol: Some(*symbol), message: format!("restricted intrinsic `{name}` cannot be used as a function value; export a script wrapper")});
                        }
                    }
                }
            }
            let Instruction::Call {
                function,
                arguments,
                receiver,
                argument_types,
                ..
            } = instruction
            else {
                continue;
            };
            if argument_types.len() != arguments.count as usize + usize::from(receiver.is_some()) {
                errors.push(LinkError {
                    module: module_id,
                    symbol: Some(*function),
                    message: "call argument type metadata does not match its register window"
                        .into(),
                });
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
                    if let LinkedFunction::Native(builtin) = target {
                        if let Some(capability) = natives.required_capability(builtin) {
                            if !policy.allows(module_id, capability) {
                                errors.push(LinkError {
                                    module: module_id,
                                    symbol: Some(*function),
                                    message: format!(
                                        "call to `{name}` requires capability `{capability}`"
                                    ),
                                });
                                continue;
                            }
                        }
                    }
                    if let LinkedFunction::Script {
                        module,
                        function: index,
                    } = target
                    {
                        let signature = &signatures[module.0 as usize][index as usize];
                        let expected =
                            signature.parameters.len() + usize::from(signature.receiver.is_some());
                        let actual = arguments.count as usize + usize::from(receiver.is_some());
                        if expected != actual {
                            errors.push(LinkError {
                                module: module_id,
                                symbol: Some(*function),
                                message: format!(
                                    "function `{name}` expects {expected} arguments, got {actual}"
                                ),
                            });
                        }
                        let expected_types = signature.receiver.iter().chain(&signature.parameters);
                        let mut substitutions = BTreeMap::new();
                        for (index, (expected, actual)) in
                            expected_types.zip(argument_types).enumerate()
                        {
                            let expected = canonical_type(
                                expected,
                                &manifests[module.0 as usize],
                                module,
                                &mut type_symbols,
                            );
                            let numeric_literal = actual.numeric_literal;
                            let actual = canonical_type(
                                &actual.ty,
                                &bytecode.symbols,
                                module_id,
                                &mut type_symbols,
                            );
                            if numeric_literal
                                && expected == crate::ScriptType::Float
                                && actual == crate::ScriptType::Int
                            {
                                continue;
                            }
                            if !parameter_accepts(&expected, &actual, &mut substitutions) {
                                errors.push(LinkError { module: module_id, symbol: Some(*function), message: format!("argument {} of `{name}` expects {expected:?}, got {actual:?}", index + 1) });
                            }
                        }
                    }
                    calls.insert(*function, target);
                }
                None => errors.push(LinkError {
                    module: module_id,
                    symbol: Some(*function),
                    message: format!("function `{name}` has no script or native implementation"),
                }),
            }
        }
        let fingerprint = match crate::fingerprint::fingerprint(&(&bytecode, &calls)) {
            Ok(fingerprint) => fingerprint,
            Err(error) => {
                errors.push(LinkError {
                    module: module_id,
                    symbol: None,
                    message: format!("cannot fingerprint bytecode: {error}"),
                });
                continue;
            }
        };
        linked_modules.push(LinkedModule {
            fingerprint,
            id: module_id,
            bytecode: Arc::new(bytecode),
            calls,
        });
    }
    if errors.is_empty() {
        Ok(LinkedProgram {
            modules: linked_modules.into(),
        })
    } else {
        Err(errors)
    }
}

/// Symbol IDs are local to a module. Compare types in a shared link-time
/// namespace; a script-defined nominal struct remains owned by its module.
fn canonical_type(
    ty: &crate::ScriptType,
    manifest: &crate::SymbolManifest,
    module: ModuleId,
    symbols: &mut crate::SymbolInterner,
) -> crate::ScriptType {
    use crate::ScriptType as T;
    let name = |id| manifest.resolve(id).unwrap_or("<invalid type symbol>");
    match ty {
        T::Named(id) => T::Named(symbols.intern(name(*id))),
        T::TypeParameter(id) => {
            T::TypeParameter(symbols.intern(format!("{}::{}", module.0, name(*id))))
        }
        T::Enum {
            name: id,
            arguments,
            variants,
        } => T::Enum {
            name: symbols.intern(format!("{}::{}", module.0, name(*id))),
            arguments: arguments
                .iter()
                .map(|ty| canonical_type(ty, manifest, module, symbols))
                .collect(),
            variants: variants
                .iter()
                .map(|(n, fields)| {
                    (
                        n.clone(),
                        fields
                            .iter()
                            .map(|ty| canonical_type(ty, manifest, module, symbols))
                            .collect(),
                    )
                })
                .collect(),
        },
        T::Struct {
            name: id,
            arguments,
            fields,
        } => T::Struct {
            name: symbols.intern(format!("{}::{}", module.0, name(*id))),
            arguments: arguments
                .iter()
                .map(|ty| canonical_type(ty, manifest, module, symbols))
                .collect(),
            fields: fields
                .iter()
                .map(|(key, ty)| (key.clone(), canonical_type(ty, manifest, module, symbols)))
                .collect(),
        },
        T::Callable { parameters, result } => T::Callable {
            parameters: parameters
                .iter()
                .map(|ty| canonical_type(ty, manifest, module, symbols))
                .collect(),
            result: Box::new(canonical_type(result, manifest, module, symbols)),
        },
        T::TupleOf(values) => T::TupleOf(
            values
                .iter()
                .map(|ty| canonical_type(ty, manifest, module, symbols))
                .collect(),
        ),
        T::Union(values) => T::Union(
            values
                .iter()
                .map(|ty| canonical_type(ty, manifest, module, symbols))
                .collect(),
        ),
        T::List(inner) => T::List(Box::new(canonical_type(inner, manifest, module, symbols))),
        T::Optional(inner) => {
            T::Optional(Box::new(canonical_type(inner, manifest, module, symbols)))
        }
        T::Binding(inner) => T::Binding(Box::new(canonical_type(inner, manifest, module, symbols))),
        T::Map(key, value) => T::Map(
            Box::new(canonical_type(key, manifest, module, symbols)),
            Box::new(canonical_type(value, manifest, module, symbols)),
        ),
        T::Record(fields) => T::Record(
            fields
                .iter()
                .map(|(key, ty)| (key.clone(), canonical_type(ty, manifest, module, symbols)))
                .collect(),
        ),
        ty => ty.clone(),
    }
}

fn parameter_accepts(
    expected: &crate::ScriptType,
    actual: &crate::ScriptType,
    substitutions: &mut BTreeMap<SymbolId, crate::ScriptType>,
) -> bool {
    use crate::ScriptType as T;
    // Erased values cannot be proven here; concrete mismatches can and must be
    // rejected before execution. Dynamic checks are a separate VM boundary.
    if actual == &T::Any {
        return true;
    }
    match (expected, actual) {
        (T::TypeParameter(id), actual) => match substitutions.get(id) {
            Some(bound) => bound.accepts(actual),
            None => {
                substitutions.insert(*id, actual.clone());
                true
            }
        },
        (T::List(expected), T::List(actual))
        | (T::Optional(expected), T::Optional(actual))
        | (T::Binding(expected), T::Binding(actual)) => {
            parameter_accepts(expected, actual, substitutions)
        }
        (T::TupleOf(expected), T::TupleOf(actual)) => {
            expected.len() == actual.len()
                && expected
                    .iter()
                    .zip(actual)
                    .all(|(expected, actual)| parameter_accepts(expected, actual, substitutions))
        }
        (
            T::Enum {
                name,
                arguments,
                variants,
            },
            T::Enum {
                name: actual_name,
                arguments: actuals,
                variants: actual_variants,
            },
        ) => {
            name == actual_name
                && arguments.len() == actuals.len()
                && arguments
                    .iter()
                    .zip(actuals)
                    .all(|(e, a)| parameter_accepts(e, a, substitutions))
                && variants.len() == actual_variants.len()
                && variants.iter().all(|(tag, fields)| {
                    actual_variants.get(tag).is_some_and(|actual| {
                        fields.len() == actual.len()
                            && fields
                                .iter()
                                .zip(actual)
                                .all(|(e, a)| parameter_accepts(e, a, substitutions))
                    })
                })
        }
        (T::Map(ek, ev), T::Map(ak, av)) => {
            parameter_accepts(ek, ak, substitutions) && parameter_accepts(ev, av, substitutions)
        }
        _ => expected.accepts(actual),
    }
}

#[cfg(test)]
mod tests {
    use crate::{BuiltinManifest, compile_with_manifest, parse_program};

    use super::*;

    #[test]
    fn intrinsic_grants_are_module_local_and_default_deny() {
        let mut registry = crate::native::NativeRegistry::<()>::new();
        let id = registry
            .register_fn("hostSay", |_: &mut (), _: String| Ok(()))
            .expect("native");
        registry
            .require_capability(id, "dialogue.write")
            .expect("capability");
        let manifest = registry.manifest();
        // Compile a relocation against a declared external name, then link
        // against the actual implementation and capability policy.
        registry
            .register_fn("narrate", |_: &mut (), _: String| Ok(()))
            .expect("external declaration");
        let declarations = registry.manifest();
        let compile = |source: &str| {
            compile_with_manifest(&parse_program(source).expect("parse"), 17, &declarations)
                .expect("compile")
        };
        let provider = compile("global fn narrate(text: String) { hostSay(text) }");
        let consumer = compile("narrate(\"hello\")");
        let modules = vec![(None, provider.clone()), (None, consumer.clone())];
        assert!(
            link_named_modules(modules.clone(), &manifest)
                .expect_err("default deny")
                .iter()
                .any(|e| e.message.contains("dialogue.write"))
        );
        let mut policy = LinkPolicy::default();
        policy.grant(ModuleId(0), "dialogue.write");
        link_named_modules_with_policy(modules, &manifest, &policy).expect("authorized wrapper");
        let untrusted = compile("hostSay(\"bypass\")");
        let errors = link_named_modules_with_policy(
            vec![(None, provider), (None, untrusted)],
            &manifest,
            &policy,
        )
        .expect_err("no privilege propagation");
        assert!(
            errors
                .iter()
                .any(|e| e.module == ModuleId(1) && e.message.contains("dialogue.write"))
        );
        policy.grant(ModuleId(1), "unrelated");
        assert!(!policy.allows(ModuleId(1), "dialogue.write"));
    }

    #[test]
    fn intrinsic_references_cannot_escape_even_from_authorized_modules() {
        let mut registry = crate::native::NativeRegistry::<()>::new();
        let id = registry
            .register_fn("hostSay", |_: &mut (), _: String| Ok(()))
            .expect("native");
        registry
            .require_capability(id, "dialogue.write")
            .expect("capability");
        let manifest = registry.manifest();
        let code = compile_with_manifest(
            &parse_program("let leaked = hostSay").expect("parse"),
            17,
            &manifest,
        )
        .expect("compile");
        let mut policy = LinkPolicy::default();
        policy.grant(ModuleId(0), "dialogue.write");
        let errors = link_named_modules_with_policy(vec![(None, code)], &manifest, &policy)
            .expect_err("reference must fail");
        assert!(errors.iter().any(|e| e.message.contains("function value")));
    }

    #[test]
    fn capability_requirements_affect_abi_hash_and_namespace_is_reserved() {
        let registry = crate::native::NativeRegistry::<()>::new();
        let public = registry.manifest();
        let restricted = public
            .clone()
            .with_capabilities([(BuiltinId(0), "read".into())].into());
        assert_ne!(public.hash(), restricted.hash());
        let provider = compile("global fn forged() {}");
        let errors =
            link_named_modules(vec![(Some("intrinsics.engine".into()), provider)], &public)
                .expect_err("reserved namespace");
        assert!(errors.iter().any(|e| e.message.contains("reserved")));
    }

    #[test]
    fn cross_module_concrete_types_are_checked_before_execution() {
        let natives = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let provider = compile("global fn score(value: Int) -> Int { value }");
        for source in ["score(\"alice\")", "score(1)\nscore(false)"] {
            let errors = link_register_modules(vec![compile(source), provider.clone()], &natives)
                .expect_err("concrete argument mismatch must fail during linking");
            assert!(
                errors
                    .iter()
                    .any(|error| error.message.contains("argument 1 of `score` expects Int"))
            );
        }
        let provider = compile("global fn label(value: (Int, String)) -> Unit { () }");
        assert!(
            link_register_modules(
                vec![compile("label((1, \"alice\"))"), provider.clone()],
                &natives
            )
            .is_ok()
        );
        assert!(
            link_register_modules(vec![compile("label((1, false))"), provider], &natives).is_err()
        );
    }

    #[test]
    fn late_float_context_accepts_literals_but_not_int_bindings() {
        let natives = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let provider = compile("global fn scale(value: Float) -> Float { value }");
        assert!(
            link_register_modules(
                vec![compile("scale(1)\nscale(-2)"), provider.clone()],
                &natives
            )
            .is_ok()
        );
        let errors = link_register_modules(
            vec![compile("let count: Int = 1\nscale(count)"), provider],
            &natives,
        )
        .expect_err("typed Int is not a contextual literal");
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("expects Float, got Int"))
        );
    }

    #[test]
    fn identically_named_private_structs_are_not_shared_module_types() {
        let natives = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let provider = compile(
            "type Player = .{ score: Int }\nglobal fn score(player: Player) -> Int { player.score }",
        );
        let consumer = compile("type Player = .{ score: Int }\nscore(Player.{ score: 1 })");
        assert!(link_register_modules(vec![consumer, provider], &natives).is_err());
    }

    #[test]
    fn every_cross_module_call_is_checked_for_arity() {
        let provider = compile("global fn score(value: Int) -> Int { value }");
        let consumer = compile("score(1)\nscore(1, 2)");
        let signature = &provider.functions[0].signature;
        assert_eq!(signature.parameters, vec![crate::ScriptType::Int]);
        assert_eq!(signature.result, crate::ScriptType::Int);
        assert_eq!(signature.receiver, None);
        let errors = link_register_modules(
            vec![consumer, provider],
            &BuiltinManifest::new(Vec::<(String, BuiltinId)>::new()),
        )
        .expect_err("a prior valid call must not hide a later invalid call");
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("expects 1 arguments, got 2"))
        );
    }

    fn compile(source: &str) -> Bytecode {
        // Untyped external declarations deliberately leave signature validation
        // to these linker-defense tests. Production compilation rejects names
        // which have no declaration at all.
        let manifest = BuiltinManifest::new([
            ("greet", BuiltinId(100)),
            ("score", BuiltinId(101)),
            ("scale", BuiltinId(102)),
            ("ui.widgets.button", BuiltinId(103)),
            ("missingFunction", BuiltinId(104)),
            ("label", BuiltinId(105)),
        ]);
        compile_with_manifest(
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

    #[test]
    fn wildcard_imports_resolve_namespaced_exports() {
        let provider = compile("global fn button(label: String) { label }");
        let consumer = compile("import ui.widgets.*\nbutton(\"Continue\")");
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let linked = link_named_modules(
            vec![(Some("ui.widgets".to_string()), provider), (None, consumer)],
            &manifest,
        )
        .expect("wildcard import must link to the namespaced export");
        let consumer = &linked.modules[1];
        let symbol = consumer
            .bytecode
            .symbols
            .find("ui.widgets.button")
            .expect("import must normalize the function to its qualified symbol");
        assert_eq!(
            consumer.resolve(symbol),
            Some(LinkedFunction::Script {
                module: ModuleId(0),
                function: 0,
            })
        );
    }
}
