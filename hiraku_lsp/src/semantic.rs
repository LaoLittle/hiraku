use hiraku_script::{
    BuiltinManifest, CompileError, HirArena, HirBlock, HirExpr, HirExprKind as E, HirProgram,
    HirStmtKind as S, ResolvedFunction, ScriptType as T, Span, SymbolManifest, hir::HirPass,
};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
/// Compiler-resolved argument information. The type is contextual, not a
/// guess based on the argument's literal spelling.
pub struct TypedArgument {
    pub parameter: Option<String>,
    pub type_name: String,
}

#[derive(Default)]
/// A compilation-owned observer. Hosts must invalidate it when any contributing
/// source changes. Only publish observations after the associated compiler run
/// succeeds; discard partial output from a failed or superseded compilation.
pub struct TypedCalls {
    path: String,
    modules: BTreeMap<String, BTreeMap<(usize, usize), Option<TypedArgument>>>,
}

impl TypedCalls {
    pub fn argument(&self, path: &str, span: Span) -> Option<&TypedArgument> {
        self.modules
            .get(path)?
            .get(&(span.start, span.end))?
            .as_ref()
    }
}

enum Node<'a> {
    Block(&'a HirBlock<'a>),
    Expr(&'a HirExpr<'a>),
}

impl HirPass for TypedCalls {
    fn begin_module(&mut self, path: &str) {
        self.path = path.into();
        self.modules.remove(path);
    }

    fn run<'hir>(
        &mut self,
        _: &'hir HirArena,
        program: &mut HirProgram<'hir>,
        _: &BuiltinManifest,
    ) -> Result<(), Vec<CompileError>> {
        let output = self.modules.entry(self.path.clone()).or_default();
        let mut pending = vec![Node::Block(program.entry)];
        pending.extend(
            program
                .functions
                .iter()
                .map(|function| Node::Block(function.body)),
        );
        while let Some(node) = pending.pop() {
            match node {
                Node::Block(block) => {
                    for statement in block.statements {
                        match statement.kind {
                            S::Return(value) => pending.extend(value.map(Node::Expr)),
                            S::Let { value, commit, .. } => {
                                pending.push(Node::Expr(value));
                                pending.extend(commit.map(Node::Expr));
                            }
                            S::Global { value, commit, .. } => {
                                pending.extend(value.map(Node::Expr));
                                pending.extend(commit.map(Node::Expr));
                            }
                            S::Assign { value, .. } | S::Expr(value) => {
                                pending.push(Node::Expr(value))
                            }
                            S::If {
                                condition,
                                then_block,
                                else_block,
                            } => {
                                pending.push(Node::Expr(condition));
                                pending.push(Node::Block(then_block));
                                pending.extend(else_block.map(Node::Block));
                            }
                            S::While { condition, body } => {
                                pending.push(Node::Expr(condition));
                                pending.push(Node::Block(body));
                            }
                        }
                    }
                }
                Node::Expr(expression) => match expression.kind {
                    E::Call {
                        callee,
                        arguments,
                        function,
                        ..
                    } => {
                        pending.push(Node::Expr(callee));
                        let parameters = if let ResolvedFunction::User(id) = function {
                            program
                                .functions
                                .get(id.0 as usize)
                                .map(|function| function.parameters)
                        } else {
                            None
                        };
                        for (index, argument) in arguments.iter().enumerate() {
                            pending.push(Node::Expr(argument.value));
                            let Some(ty) = program.types.get(argument.value.ty) else {
                                continue;
                            };
                            let parameter = parameters
                                .and_then(|parameters| parameters.get(index))
                                .and_then(|id| program.locals.get(id.0 as usize))
                                .and_then(|local| program.symbols.resolve(local.name))
                                .map(str::to_owned);
                            let hint = TypedArgument {
                                parameter,
                                type_name: type_name(ty, &program.symbols),
                            };
                            let key = (argument.span.start, argument.span.end);
                            match output.entry(key) {
                                std::collections::btree_map::Entry::Vacant(entry) => {
                                    entry.insert(Some(hint));
                                }
                                std::collections::btree_map::Entry::Occupied(mut entry) => {
                                    if entry.get().as_ref() != Some(&hint) {
                                        entry.insert(None);
                                    }
                                }
                            }
                        }
                    }
                    E::When { value, arms, .. } => {
                        pending.push(Node::Expr(value));
                        pending.extend(arms.iter().map(|arm| Node::Block(arm.body)));
                    }
                    E::Intrinsic { argument, .. } => pending.push(Node::Expr(argument)),
                    E::GuardNever(value)
                    | E::UnaryMinus(value)
                    | E::NonNull(value)
                    | E::OptionalSome(value)
                    | E::Cast { value, .. }
                    | E::Member { object: value, .. } => pending.push(Node::Expr(value)),
                    E::Tuple(values) | E::List(values) | E::Variant { values, .. } => {
                        pending.extend(values.iter().map(|value| Node::Expr(value)))
                    }
                    E::Map { fields, .. } => {
                        pending.extend(fields.iter().map(|(_, value)| Node::Expr(value)))
                    }
                    E::Lambda { body, .. } | E::Block(body) => pending.push(Node::Block(body)),
                    E::Binary { left, right, .. } => {
                        pending.push(Node::Expr(left));
                        pending.push(Node::Expr(right));
                    }
                    E::Literal(_)
                    | E::Local(_)
                    | E::Global(_)
                    | E::Function(_)
                    | E::Builtin(_)
                    | E::Selector(_)
                    | E::Symbol(_)
                    | E::Unresolved(_) => (),
                },
            }
        }
        Ok(())
    }
}

pub fn type_name(ty: &T, symbols: &SymbolManifest) -> String {
    let list = |types: &[T]| {
        types
            .iter()
            .map(|ty| type_name(ty, symbols))
            .collect::<Vec<_>>()
            .join(", ")
    };
    match ty {
        T::Named(id) | T::TypeParameter(id) => symbols.resolve(*id).unwrap_or("?").into(),
        T::Struct {
            name, arguments, ..
        }
        | T::Enum {
            name, arguments, ..
        } => {
            let name = symbols.resolve(*name).unwrap_or("?");
            if arguments.is_empty() {
                name.into()
            } else {
                format!("{name}<{}>", list(arguments))
            }
        }
        T::Optional(ty) => match ty.as_ref() {
            T::Callable { .. } | T::Union(_) => format!("({})?", type_name(ty, symbols)),
            T::Optional(_) => format!("Optional<{}>", type_name(ty, symbols)),
            _ => format!("{}?", type_name(ty, symbols)),
        },
        T::List(ty) => format!("List<{}>", type_name(ty, symbols)),
        T::Map(key, value) => format!(
            "Map<{}, {}>",
            type_name(key, symbols),
            type_name(value, symbols)
        ),
        T::TupleOf(types) => format!("({})", list(types)),
        T::Callable { parameters, result } => {
            format!("({}) -> {}", list(parameters), type_name(result, symbols))
        }
        T::Union(types) => types
            .iter()
            .map(|ty| type_name(ty, symbols))
            .collect::<Vec<_>>()
            .join(" | "),
        T::Record(_) => "record".into(),
        T::Binding(ty) => format!("Binding<{}>", type_name(ty, symbols)),
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hiraku_script::{
        ProjectLinkPolicy, ScriptSource, compile_project_with_policy_and_hir_pass,
    };

    #[test]
    fn contextual_types_and_parameter_names_come_from_the_compiler() {
        let source = "fn configure(enabled: Bool, amount: Float, name: String) {}\nconfigure(true, 1, \"Alice\")";
        let mut calls = TypedCalls::default();
        let manifest = BuiltinManifest::new(std::iter::empty::<(&str, hiraku_script::BuiltinId)>());
        compile_project_with_policy_and_hir_pass(
            vec![ScriptSource {
                path: "alice.hks".into(),
                namespace: None,
                source: source.into(),
            }],
            &manifest,
            &ProjectLinkPolicy::default(),
            &mut calls,
        )
        .expect("Compile");
        let offset = source.rfind(", 1,").expect("Argument") + 2;
        let hint = calls
            .argument("alice.hks", Span::new_with_end(offset, offset + 1))
            .expect("Float argument");
        assert_eq!(hint.parameter.as_deref(), Some("amount"));
        assert_eq!(hint.type_name, "Float");
        assert!(
            calls
                .argument("bob.hks", Span::new_with_end(offset, offset + 1))
                .is_none()
        );
    }

    #[test]
    fn receiver_arguments_and_shadowed_callables_keep_their_real_types() {
        let source = "struct Alice {}\nextend Alice { fn configure(self, value: Float?) {} }\nlet alice: Alice = .{}\nalice.configure(1)\nfn accept(value: Int) {}\nfn inner() { let accept: (String) -> Unit = { value: String -> () }; accept(\"Bob\") }";
        let manifest = BuiltinManifest::new(std::iter::empty::<(&str, hiraku_script::BuiltinId)>());
        let mut calls = TypedCalls::default();
        compile_project_with_policy_and_hir_pass(
            vec![ScriptSource {
                path: "alice.hks".into(),
                namespace: None,
                source: source.into(),
            }],
            &manifest,
            &ProjectLinkPolicy::default(),
            &mut calls,
        )
        .expect("Compile");
        let offset = source.find("configure(1)").expect("Method") + "configure(".len();
        let hint = calls
            .argument("alice.hks", Span::new_with_end(offset, offset + 1))
            .expect("Method hint");
        assert_eq!(hint.parameter.as_deref(), Some("value"));
        assert_eq!(hint.type_name, "Float?");
        let offset = source.rfind("\"Bob\"").expect("Shadowed callable");
        let hint = calls
            .argument("alice.hks", Span::new_with_end(offset, offset + 5))
            .expect("Callable hint");
        assert_eq!(hint.type_name, "String");
        assert_eq!(hint.parameter, None);
        calls.begin_module("alice.hks");
        assert!(
            calls
                .argument("alice.hks", Span::new_with_end(offset, offset + 5))
                .is_none()
        );
    }

    #[test]
    fn optional_callable_and_nested_optional_type_names_are_unambiguous() {
        let symbols = SymbolManifest::default();
        let callable = T::Callable {
            parameters: vec![T::Int],
            result: Box::new(T::Unit),
        };
        assert_eq!(
            type_name(&T::Optional(Box::new(callable)), &symbols),
            "((Int) -> Unit)?"
        );
        assert_eq!(
            type_name(
                &T::Optional(Box::new(T::Optional(Box::new(T::String)))),
                &symbols
            ),
            "Optional<String?>"
        );
    }
}
