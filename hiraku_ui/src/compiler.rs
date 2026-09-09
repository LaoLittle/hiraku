use hiraku_script::{
    BuiltinManifest, CompileError, HirArena, HirArgument, HirBlock, HirExpr, HirExprKind as E,
    HirFunction, HirProgram, HirStmt, HirStmtKind as S, ResolvedFunction, ScriptType, Span,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionKind {
    Property,
    Conditional,
    Repeated,
}

/// Compilation-owned identity, independent of optional debug tables.
#[derive(Clone, Debug)]
pub struct RegionSite {
    pub id: u32,
    pub kind: RegionKind,
    pub span: Span,
}

#[derive(Clone, Debug, Default)]
pub struct CompositionPlan {
    pub sites: Vec<RegionSite>,
    /// Conservative reads outside extracted property computations.
    pub structural_globals: BTreeSet<String>,
    /// All reads, including extracted properties and helper functions. Hosts
    /// can use this to schedule updates for time-dependent documents.
    pub read_globals: BTreeSet<String>,
}

/// Lifts host-declared property parameters into ordinary closures.
/// Accepts the legacy Union<T, Binding<T>> schema during host migration.
/// Generated code itself uses callable types, not Binding values.
#[derive(Default)]
pub struct UiCompiler {
    path: String,
    pub plans: BTreeMap<String, CompositionPlan>,
}

impl hiraku_script::hir::HirPass for UiCompiler {
    fn begin_module(&mut self, path: &str) {
        self.path = path.into();
    }
    fn run<'h>(
        &mut self,
        arena: &'h HirArena,
        program: &mut HirProgram<'h>,
        manifest: &BuiltinManifest,
    ) -> Result<(), Vec<CompileError>> {
        let mut pass = Lower {
            arena,
            program,
            manifest,
            plan: CompositionPlan::default(),
            property_depth: 0,
        };
        pass.program.entry = pass.block(pass.program.entry);
        let functions = pass
            .program
            .functions
            .to_vec()
            .into_iter()
            .map(|function| HirFunction {
                body: pass.block(function.body),
                ..function
            })
            .collect::<Vec<_>>();
        pass.program.functions = arena.alloc_slice_copy(&functions);
        self.plans.insert(self.path.clone(), pass.plan);
        Ok(())
    }
}

struct Lower<'a, 'h> {
    arena: &'h HirArena,
    program: &'a mut HirProgram<'h>,
    manifest: &'a BuiltinManifest,
    plan: CompositionPlan,
    property_depth: usize,
}

fn is_property(ty: &ScriptType) -> bool {
    matches!(ty, ScriptType::Union(types) if types.iter().any(|ty| matches!(ty, ScriptType::Binding(_))))
}

impl<'h> Lower<'_, 'h> {
    fn site(&mut self, kind: RegionKind, span: Span) {
        self.plan.sites.push(RegionSite {
            id: self.plan.sites.len() as u32,
            kind,
            span,
        });
    }
    fn block(&mut self, block: &HirBlock<'h>) -> &'h HirBlock<'h> {
        let statements = block
            .statements
            .iter()
            .map(|stmt| self.statement(stmt))
            .collect::<Vec<_>>();
        self.arena.alloc(HirBlock {
            statements: self.arena.alloc_slice_copy(&statements),
            span: block.span,
        })
    }
    fn statement(&mut self, stmt: &HirStmt<'h>) -> &'h HirStmt<'h> {
        let kind = match stmt.kind {
            S::Expr(value) => S::Expr(self.expr(value)),
            S::Return(value) => S::Return(value.map(|v| self.expr(v))),
            S::Let { local, value } => S::Let {
                local,
                value: self.expr(value),
            },
            S::Global { global, value } => S::Global {
                global,
                value: value.map(|v| self.expr(v)),
            },
            S::Assign { target, value } => S::Assign {
                target,
                value: self.expr(value),
            },
            S::If {
                condition,
                then_block,
                else_block,
            } => {
                if self.property_depth == 0 {
                    self.site(RegionKind::Conditional, stmt.span);
                }
                S::If {
                    condition: self.expr(condition),
                    then_block: self.block(then_block),
                    else_block: else_block.map(|b| self.block(b)),
                }
            }
            S::While { condition, body } => {
                if self.property_depth == 0 {
                    self.site(RegionKind::Repeated, stmt.span);
                }
                S::While {
                    condition: self.expr(condition),
                    body: self.block(body),
                }
            }
        };
        self.arena.alloc(HirStmt {
            kind,
            span: stmt.span,
        })
    }
    fn expr(&mut self, expr: &HirExpr<'h>) -> &'h HirExpr<'h> {
        let explicit_property = matches!(
            self.program.types.get(expr.ty),
            Some(ScriptType::Binding(_))
        );
        if explicit_property {
            self.property_depth += 1;
        }
        let kind = match expr.kind {
            E::Global(id) => {
                let name = self.program.globals[id.0 as usize].name;
                if let Some(name) = self.program.symbols.resolve(name) {
                    self.plan.read_globals.insert(name.into());
                }
                if self.property_depth == 0 {
                    let name = self.program.globals[id.0 as usize].name;
                    if let Some(name) = self.program.symbols.resolve(name) {
                        self.plan.structural_globals.insert(name.into());
                    }
                }
                expr.kind
            }
            E::Call {
                callee,
                arguments,
                function,
                type_bindings,
            } => {
                let callee = self.expr(callee);
                let signature = match function {
                    ResolvedFunction::Builtin(id) => self.manifest.signature(id).cloned(),
                    _ => None,
                };
                let event = match function {
                    ResolvedFunction::Builtin(id) => {
                        self.manifest.callable_name(id).is_some_and(|name| {
                            matches!(
                                name.rsplit('.').next(),
                                Some("onClick" | "onChange" | "onCommit")
                            )
                        })
                    }
                    _ => false,
                };
                let args = arguments
                    .iter()
                    .enumerate()
                    .map(|(index, arg)| {
                        let ty = self
                            .program
                            .types
                            .get(arg.value.ty)
                            .expect("typed argument");
                        let lift = signature
                            .as_ref()
                            .and_then(|s| s.parameters.get(index))
                            .is_some_and(is_property)
                            && !matches!(
                                ty,
                                ScriptType::Binding(_)
                                    | ScriptType::Callable { .. }
                                    | ScriptType::Function
                            )
                            && !matches!(arg.value.kind, E::Literal(_));
                        // Native selector receivers live in the callee's Member,
                        // not in the explicit argument slice.
                        let callback = event;
                        if lift || callback {
                            self.property_depth += 1;
                        }
                        let mut value = self.expr(arg.value);
                        if lift || callback {
                            self.property_depth -= 1;
                        }
                        if lift {
                            self.site(RegionKind::Property, arg.span);
                            let statement = self.arena.alloc(HirStmt {
                                kind: S::Expr(value),
                                span: arg.span,
                            });
                            let block = self.arena.alloc(HirBlock {
                                statements: self.arena.alloc_slice_copy(&[statement]),
                                span: arg.span,
                            });
                            let result = self
                                .program
                                .types
                                .get(value.ty)
                                .expect("property result")
                                .clone();
                            let ty = self.program.types.intern(ScriptType::Callable {
                                parameters: vec![],
                                result: Box::new(result),
                            });
                            value = self.arena.alloc(HirExpr {
                                kind: E::Block(block),
                                ty,
                                span: arg.span,
                            });
                        }
                        HirArgument { value, ..*arg }
                    })
                    .collect::<Vec<_>>();
                E::Call {
                    callee,
                    arguments: self.arena.alloc_slice_copy(&args),
                    function,
                    type_bindings,
                }
            }
            E::Member {
                object,
                member,
                safe,
            } => E::Member {
                object: self.expr(object),
                member,
                safe,
            },
            E::Intrinsic {
                operation,
                argument,
            } => E::Intrinsic {
                operation,
                argument: self.expr(argument),
            },
            E::GuardNever(v) => E::GuardNever(self.expr(v)),
            E::UnaryMinus(v) => E::UnaryMinus(self.expr(v)),
            E::NonNull(v) => E::NonNull(self.expr(v)),
            E::OptionalSome(v) => E::OptionalSome(self.expr(v)),
            E::Cast {
                value,
                target,
                mode,
            } => E::Cast {
                value: self.expr(value),
                target,
                mode,
            },
            E::Elvis { value, fallback } => E::Elvis {
                value: self.expr(value),
                fallback: self.expr(fallback),
            },
            E::Binary { left, op, right } => E::Binary {
                left: self.expr(left),
                op,
                right: self.expr(right),
            },
            E::Tuple(values) | E::List(values) => {
                let values = values.iter().map(|v| self.expr(v)).collect::<Vec<_>>();
                let values = self.arena.alloc_slice_copy(&values);
                if matches!(expr.kind, E::Tuple(_)) {
                    E::Tuple(values)
                } else {
                    E::List(values)
                }
            }
            E::Map { type_name, fields } => {
                let fields = fields
                    .iter()
                    .map(|(name, value)| (*name, self.expr(value)))
                    .collect::<Vec<_>>();
                E::Map {
                    type_name,
                    fields: self.arena.alloc_slice_copy(&fields),
                }
            }
            E::Lambda {
                parameters,
                body,
                return_value,
            } => E::Lambda {
                parameters,
                body: self.block(body),
                return_value,
            },
            E::Block(body) => E::Block(self.block(body)),
            E::Literal(_)
            | E::Local(_)
            | E::Function(_)
            | E::Builtin(_)
            | E::Selector(_)
            | E::Symbol(_)
            | E::Unresolved(_) => expr.kind,
        };
        if explicit_property {
            self.property_depth -= 1;
        }
        self.arena.alloc(HirExpr { kind, ..*expr })
    }
}
