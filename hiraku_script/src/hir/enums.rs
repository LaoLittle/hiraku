//! Generic enum construction and exhaustive match lowering.
use super::*;

impl<'hir, 'manifest> Lowerer<'hir, 'manifest> {
    fn enum_shape(
        &mut self,
        ty: &ScriptType,
    ) -> Option<(SymbolId, BTreeMap<String, Vec<ScriptType>>)> {
        match ty {
            ScriptType::Enum { name, variants, .. } => Some((*name, variants.clone())),
            ScriptType::Optional(inner) => {
                let (parameters, declarations) = self.enums.get("Optional")?.clone();
                self.type_parameters.push(
                    parameters
                        .into_iter()
                        .zip([inner.as_ref().clone()])
                        .collect(),
                );
                let variants = declarations
                    .into_iter()
                    .map(|variant| {
                        let fields = variant
                            .fields
                            .iter()
                            .filter_map(|field| self.type_from_ast(field))
                            .collect();
                        (variant.name, fields)
                    })
                    .collect();
                self.type_parameters.pop();
                Some((self.symbol("Optional"), variants))
            }
            _ => None,
        }
    }

    pub(super) fn lower_elvis(
        &mut self,
        source: &Expr,
        fallback: &Expr,
        expected: Option<&ScriptType>,
        span: Span,
    ) -> &'hir HirExpr<'hir> {
        // A known none contributes no payload type. In particular, none ?:
        // panic(...) is Never, not Any. Do not evaluate unreachable branches.
        if matches!(source.kind, ExprKind::Null)
            || matches!(&source.kind, ExprKind::Symbol(name) if name == "none")
        {
            return self.lower_expression_expected(fallback, expected);
        }
        let value = self.lower_expression(source);
        let ty = self.expression_type(value).clone();
        if let ScriptType::Optional(inner) = ty {
            return self.lower_optional_fallback(value, fallback, *inner, span);
        }
        if ty == ScriptType::Any {
            self.error("operator `?:` requires a known optional type; cast Any to Optional<T> with `as?` or `as!` first", source.span);
        }
        // Check the unreachable branch but omit it from executable HIR.
        let fallback = self.lower_expression_expected(fallback, Some(&ty));
        if ty != ScriptType::Never {
            self.check_assignment(&ty, &self.expression_type(fallback).clone(), fallback.span);
        }
        value
    }

    pub(super) fn lower_optional_fallback(
        &mut self,
        value: &'hir HirExpr<'hir>,
        fallback: &Expr,
        inner: ScriptType,
        span: Span,
    ) -> &'hir HirExpr<'hir> {
        // The fallback executes only in the none arm, just like an explicit when.
        self.scopes.push(BTreeMap::new());
        let local = self.declare_local("__optionalPayload", inner.clone(), false, span);
        let mut payload = self.alloc_expression(HirExprKind::Local(local), inner.clone(), span);
        self.scopes.pop();
        let context = if (matches!(fallback.kind, ExprKind::Null)
            || matches!(&fallback.kind, ExprKind::Symbol(name) if name == "none"))
            && !matches!(inner, ScriptType::Optional(_))
        {
            ScriptType::Optional(Box::new(inner.clone()))
        } else {
            inner.clone()
        };
        let fallback = self.lower_expression_expected(fallback, Some(&context));
        let fallback_type = self.expression_type(fallback).clone();
        let result = if inner.accepts(&fallback_type) {
            inner.clone()
        } else if fallback_type.accepts(&inner) {
            fallback_type.clone()
        } else {
            self.check_assignment(&inner, &fallback_type, fallback.span);
            inner.clone()
        };
        if result != inner && matches!(result, ScriptType::Optional(_)) {
            payload = self.alloc_expression(
                HirExprKind::Cast {
                    value: payload,
                    target: self.arena.alloc(result.clone()),
                    mode: CastMode::Static,
                },
                result.clone(),
                span,
            );
        }
        let some = self.symbol("some");
        let none = self.symbol("none");
        let name = self.symbol("Optional");
        let body = |expr| {
            self.arena.alloc(HirBlock {
                statements: self.arena.alloc_slice_copy(&[self.arena.alloc(HirStmt {
                    kind: HirStmtKind::Expr(expr),
                    span,
                }) as &HirStmt<'hir>]),
                span,
            }) as &HirBlock<'hir>
        };
        let arms = self.arena.alloc_slice_copy(&[
            HirWhenArm {
                variant: some,
                bindings: self.arena.alloc_slice_copy(&[local]),
                body: body(payload),
            },
            HirWhenArm {
                variant: none,
                bindings: &[],
                body: body(fallback),
            },
        ]);
        self.alloc_expression(
            HirExprKind::When {
                value,
                type_name: name,
                arms,
            },
            result,
            span,
        )
    }

    pub(super) fn lower_when(
        &mut self,
        value: &Expr,
        arms: &[crate::ast::WhenArm],
        expected: Option<&ScriptType>,
        span: Span,
    ) -> &'hir HirExpr<'hir> {
        let value = self.lower_expression(value);
        let subject_type = self.expression_type(value).clone();
        let Some((name, variants)) = self.enum_shape(&subject_type) else {
            self.error("when currently requires an enum subject", span);
            return self.alloc_expression(
                HirExprKind::Literal(HirLiteral::Unit),
                ScriptType::Any,
                span,
            );
        };
        let mut seen = std::collections::BTreeSet::new();
        let mut lowered = Vec::new();
        let mut result = ScriptType::Never;
        for arm in arms {
            if !seen.insert(arm.variant.clone()) {
                self.error("duplicate/unreachable when arm", arm.span);
            }
            let Some(fields) = variants.get(&arm.variant) else {
                self.error(format!("unknown variant `{}`", arm.variant), arm.span);
                continue;
            };
            if fields.len() != arm.bindings.len() {
                self.error(
                    format!("pattern expects {} payload bindings", fields.len()),
                    arm.span,
                );
            }
            self.scopes.push(BTreeMap::new());
            let mut names = std::collections::BTreeSet::new();
            let bindings = arm
                .bindings
                .iter()
                .zip(fields)
                .enumerate()
                .map(|(i, (n, ty))| {
                    if n != "_" && !names.insert(n.clone()) {
                        self.error("duplicate pattern binding", arm.span);
                    }
                    let n = if n == "_" {
                        format!("__discard#{i}")
                    } else {
                        n.clone()
                    };
                    self.declare_local(&n, ty.clone(), false, arm.span)
                })
                .collect::<Vec<_>>();
            let mut statements = Vec::new();
            for (index, statement) in arm.body.statements.iter().enumerate() {
                if index + 1 == arm.body.statements.len() {
                    if let Stmt::Expr(expression) = statement {
                        let value = self.lower_expression_expected(expression, expected);
                        statements.push(self.arena.alloc(HirStmt {
                            kind: HirStmtKind::Expr(value),
                            span: expression.span,
                        }) as &HirStmt<'hir>);
                        continue;
                    }
                }
                if let Some(statement) = self.lower_statement(statement) {
                    statements.push(statement);
                }
            }
            let body = self.arena.alloc(HirBlock {
                statements: self.arena.alloc_slice_copy(&statements),
                span: arm.body.span,
            });
            let ty = match body.statements.last().map(|s| s.kind) {
                Some(HirStmtKind::Expr(expr)) => self.expression_type(expr).clone(),
                Some(HirStmtKind::Return(_)) => ScriptType::Never,
                _ => ScriptType::Unit,
            };
            if let Some(expected) = expected {
                self.check_assignment(expected, &ty, arm.span);
            }
            if result == ScriptType::Never {
                result = ty.clone();
            } else if ty != ScriptType::Never && ty != result {
                self.error("when arms must produce the same type", arm.span);
            }
            self.scopes.pop();
            let variant = self.symbol(&arm.variant);
            lowered.push(HirWhenArm {
                variant,
                bindings: self.arena.alloc_slice_copy(&bindings),
                body,
            });
        }
        let missing = variants
            .keys()
            .filter(|v| !seen.contains(*v))
            .cloned()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            self.error(
                format!("non-exhaustive when; missing {}", missing.join(", ")),
                span,
            );
        }
        self.alloc_expression(
            HirExprKind::When {
                value,
                type_name: name,
                arms: self.arena.alloc_slice_copy(&lowered),
            },
            result,
            span,
        )
    }

    pub(super) fn lower_enum_constructor(
        &mut self,
        expression: &Expr,
        expected: Option<&ScriptType>,
    ) -> Option<&'hir HirExpr<'hir>> {
        let (callee, args, type_args, trailing) = match &expression.kind {
            ExprKind::Call {
                callee,
                arguments,
                type_arguments,
                trailing_block,
            } => (
                callee.as_ref(),
                arguments.as_slice(),
                type_arguments.as_slice(),
                trailing_block.is_some(),
            ),
            _ => (expression, &[][..], &[][..], false),
        };
        let (variant, ty) = match &callee.kind {
            ExprKind::Symbol(variant) => match expected {
                Some(ty @ ScriptType::Optional(_)) => {
                    // A selector can target the wrapped native type, e.g.
                    // camera(.canvas). Only claim actual Optional variants.
                    if !self.enum_shape(ty)?.1.contains_key(variant) {
                        return None;
                    }
                    (variant.clone(), ty.clone())
                }
                Some(ty @ ScriptType::Enum { .. }) => (variant.clone(), ty.clone()),
                _ => return None,
            },
            ExprKind::Member {
                object,
                name: variant,
            } => {
                let ExprKind::Ident(owner) = &object.kind else {
                    return None;
                };
                if !self.enums.contains_key(owner) {
                    return None;
                }
                let arguments = type_args
                    .iter()
                    .filter_map(|ty| self.type_from_ast(ty))
                    .collect::<Vec<_>>();
                let ty = if type_args.is_empty() {
                    match expected {
                        Some(ty @ ScriptType::Enum { name, .. })
                            if self.symbols.manifest().resolve(*name) == Some(owner.as_str()) =>
                        {
                            ty.clone()
                        }
                        Some(ty @ ScriptType::Optional(_)) if owner == "Optional" => ty.clone(),
                        _ => self.instantiate_named_type(owner, &arguments, expression.span)?,
                    }
                } else {
                    self.instantiate_named_type(owner, &arguments, expression.span)?
                };
                (variant.clone(), ty)
            }
            _ => return None,
        };
        let Some((name, variants)) = self.enum_shape(&ty) else {
            return None;
        };
        let Some(fields) = variants.get(&variant) else {
            self.error(format!("enum has no variant `{variant}`"), expression.span);
            return Some(self.alloc_expression(
                HirExprKind::Literal(HirLiteral::Unit),
                ScriptType::Any,
                expression.span,
            ));
        };
        if trailing || fields.len() != args.len() {
            self.error(
                format!(
                    "variant `{variant}` expects {} payload arguments",
                    fields.len()
                ),
                expression.span,
            );
        }
        let mut values = Vec::new();
        for (arg, expected) in args.iter().zip(fields) {
            let value = self.lower_expression_expected(&arg.value, Some(expected));
            self.check_assignment(expected, &self.expression_type(value).clone(), arg.span);
            values.push(value);
        }
        if matches!(ty, ScriptType::Optional(_)) {
            let kind = match values.first() {
                Some(value) => HirExprKind::OptionalSome(value),
                None => HirExprKind::Literal(HirLiteral::Null),
            };
            return Some(self.alloc_expression(kind, ty, expression.span));
        }
        let type_name = name;
        let variant = self.symbol(&variant);
        Some(self.alloc_expression(
            HirExprKind::Variant {
                type_name,
                variant,
                values: self.arena.alloc_slice_copy(&values),
            },
            ty,
            expression.span,
        ))
    }
}
