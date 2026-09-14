//! Lexical Self aliases and conformance-qualified associated type projections.
use super::LoweringError;
use crate::{Block, Expr, ExprKind, Stmt, TypeExpr, TypeExprKind};
use std::collections::BTreeMap;

pub(super) struct TypeContext {
    pub aliases: BTreeMap<String, TypeExpr>,
    pub associated: BTreeMap<String, TypeExpr>,
    pub owner: TypeExpr,
    pub protocol: Option<TypeExpr>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: &str) -> TypeExpr {
        TypeExpr {
            kind: TypeExprKind::Named(name.into()),
            span: crate::Span { start: 0, end: 0 },
        }
    }

    #[test]
    fn self_retains_conformance_until_member_projection_is_resolved() {
        let mut context = TypeContext::new(named("Item"), Some(named("Notify")));
        assert!(matches!(
            context.aliases["Self"].kind,
            TypeExprKind::Qualified { .. }
        ));
        context.associated.insert(
            "Output".into(),
            TypeExpr {
                kind: TypeExprKind::Unit,
                span: named("Item").span,
            },
        );
        let projection = TypeExpr {
            kind: TypeExprKind::Member {
                object: Box::new(named("Self")),
                name: "Output".into(),
            },
            span: named("Item").span,
        };
        let mut errors = Vec::new();
        assert_eq!(context.resolve(&named("Self"), &mut errors), named("Item"));
        assert_eq!(
            context.resolve(&projection, &mut errors).kind,
            TypeExprKind::Unit
        );
        context.aliases.insert("T".into(), named("T"));
        assert_eq!(context.resolve(&named("T"), &mut errors), named("T"));
        assert!(errors.is_empty());
    }
}

impl TypeContext {
    pub fn new(owner: TypeExpr, protocol: Option<TypeExpr>) -> Self {
        let view = TypeExpr {
            kind: protocol.as_ref().map_or_else(
                || owner.kind.clone(),
                |protocol| TypeExprKind::Qualified {
                    owner: Box::new(owner.clone()),
                    protocol: Box::new(protocol.clone()),
                },
            ),
            span: owner.span,
        };
        Self {
            aliases: BTreeMap::from([("Self".into(), view)]),
            associated: BTreeMap::new(),
            owner,
            protocol,
        }
    }

    pub fn resolve(&self, ty: &TypeExpr, errors: &mut Vec<LoweringError>) -> TypeExpr {
        self.normalize(ty, &mut Vec::new(), errors, false)
    }

    fn normalize(
        &self,
        ty: &TypeExpr,
        active: &mut Vec<String>,
        errors: &mut Vec<LoweringError>,
        keep_view: bool,
    ) -> TypeExpr {
        if let TypeExprKind::Named(name) = &ty.kind {
            let replacement = self.aliases.get(name).cloned().or_else(|| {
                self.associated.contains_key(name).then(|| TypeExpr {
                    kind: TypeExprKind::Member {
                        object: Box::new(TypeExpr {
                            kind: TypeExprKind::Named("Self".into()),
                            span: ty.span,
                        }),
                        name: name.clone(),
                    },
                    span: ty.span,
                })
            });
            if let Some(replacement) = replacement {
                // Generic arguments may deliberately preserve a parameter,
                // e.g. Protocol<T> instantiated with the caller's own T.
                if replacement.kind == ty.kind {
                    return ty.clone();
                }
                let key = format!("alias:{name}");
                if active.contains(&key) {
                    errors.push(LoweringError {
                        message: format!("cyclic type alias or associated type `{name}`"),
                        span: ty.span,
                    });
                    return ty.clone();
                }
                active.push(key);
                let result = self.normalize(&replacement, active, errors, keep_view);
                active.pop();
                return TypeExpr {
                    span: ty.span,
                    ..result
                };
            }
        }
        let mut result = ty.clone();
        match &mut result.kind {
            TypeExprKind::Member { object, name } => {
                let view = self.normalize(object, active, errors, true);
                if let TypeExprKind::Qualified { owner, protocol } = &view.kind {
                    // The qualification is retained until lookup, so two
                    // protocols' Output members never share an owner-only key.
                    if super::protocols::canonical(owner)
                        == super::protocols::canonical(&self.owner)
                        && self.protocol.as_ref().is_some_and(|expected| {
                            super::protocols::canonical(protocol)
                                == super::protocols::canonical(expected)
                        })
                    {
                        if let Some(binding) = self.associated.get(name) {
                            let key = format!("projection:{name}");
                            if active.contains(&key) {
                                errors.push(LoweringError {
                                    message: format!("cyclic associated type `{name}`"),
                                    span: ty.span,
                                });
                                return ty.clone();
                            }
                            active.push(key);
                            let resolved = self.normalize(binding, active, errors, keep_view);
                            active.pop();
                            return TypeExpr {
                                span: ty.span,
                                ..resolved
                            };
                        }
                        errors.push(LoweringError {
                            message: format!(
                                "protocol implementation has no associated type `{name}`"
                            ),
                            span: ty.span,
                        });
                    }
                }
                **object = view;
            }
            TypeExprKind::Qualified { owner, .. } if !keep_view => {
                return self.normalize(owner, active, errors, false);
            }
            TypeExprKind::Tuple(types)
            | TypeExprKind::Applied {
                arguments: types, ..
            } => {
                for ty in types {
                    *ty = self.normalize(ty, active, errors, false);
                }
            }
            TypeExprKind::Function { parameters, result } => {
                for ty in parameters {
                    *ty = self.normalize(ty, active, errors, false);
                }
                **result = self.normalize(result, active, errors, false);
            }
            TypeExprKind::Nullable(inner)
            | TypeExprKind::List(inner)
            | TypeExprKind::Binding(inner) => {
                **inner = self.normalize(inner, active, errors, false);
            }
            TypeExprKind::Record(fields) => {
                for field in fields {
                    field.ty = self.normalize(&field.ty, active, errors, false);
                }
            }
            _ => {}
        }
        result
    }

    pub fn block(&self, block: &mut Block, errors: &mut Vec<LoweringError>) {
        for statement in &mut block.statements {
            self.statement(statement, errors);
        }
    }

    pub fn statement(&self, statement: &mut Stmt, errors: &mut Vec<LoweringError>) {
        match statement {
            Stmt::Function {
                parameters,
                return_type,
                body,
                ..
            } => {
                for parameter in parameters {
                    if let Some(ty) = &mut parameter.ty {
                        *ty = self.resolve(ty, errors);
                    }
                }
                if let Some(ty) = return_type {
                    *ty = self.resolve(ty, errors);
                }
                self.block(body, errors);
            }
            Stmt::Let {
                type_annotation,
                value,
                ..
            } => {
                if let Some(ty) = type_annotation {
                    *ty = self.resolve(ty, errors);
                }
                self.expression(value, errors);
            }
            Stmt::Global {
                type_annotation,
                value,
                ..
            } => {
                if let Some(ty) = type_annotation {
                    *ty = self.resolve(ty, errors);
                }
                if let Some(value) = value {
                    self.expression(value, errors);
                }
            }
            Stmt::Return { value, .. } => {
                if let Some(value) = value {
                    self.expression(value, errors);
                }
            }
            Stmt::Expr(value) => self.expression(value, errors),
            Stmt::Assign { target, value, .. } => {
                self.expression(target, errors);
                self.expression(value, errors);
            }
            Stmt::If {
                condition,
                then_block,
                else_block,
                ..
            } => {
                self.expression(condition, errors);
                self.block(then_block, errors);
                if let Some(block) = else_block {
                    self.block(block, errors);
                }
            }
            Stmt::While {
                condition, body, ..
            } => {
                self.expression(condition, errors);
                self.block(body, errors);
            }
            Stmt::TypeAlias { ty, .. } | Stmt::Struct { ty, .. } => *ty = self.resolve(ty, errors),
            Stmt::Property {
                ty, getter, setter, ..
            } => {
                *ty = self.resolve(ty, errors);
                self.block(getter, errors);
                if let Some((_, body)) = setter {
                    self.block(body, errors);
                }
            }
            Stmt::Enum { variants, .. } => {
                for variant in variants {
                    for ty in &mut variant.fields {
                        *ty = self.resolve(ty, errors);
                    }
                }
            }
            Stmt::Extend { .. } | Stmt::Protocol { .. } | Stmt::Import { .. } => {}
        }
    }

    fn expression(&self, expression: &mut Expr, errors: &mut Vec<LoweringError>) {
        match &mut expression.kind {
            ExprKind::Ident(name) if name == "Self" => {
                if let TypeExprKind::Named(owner) = &self.owner.kind {
                    *name = owner.clone();
                }
            }
            ExprKind::Member { object, .. }
            | ExprKind::SafeMember { object, .. }
            | ExprKind::Binding(object)
            | ExprKind::UnaryMinus(object)
            | ExprKind::Not(object)
            | ExprKind::NonNull(object) => self.expression(object, errors),
            ExprKind::Cast { value, ty, .. } => {
                self.expression(value, errors);
                *ty = self.resolve(ty, errors);
            }
            ExprKind::Elvis {
                value: left,
                fallback: right,
            }
            | ExprKind::Binary { left, right, .. } => {
                self.expression(left, errors);
                self.expression(right, errors);
            }
            ExprKind::Call {
                callee,
                type_arguments,
                arguments,
                trailing_block,
            } => {
                self.expression(callee, errors);
                for ty in type_arguments {
                    *ty = self.resolve(ty, errors);
                }
                for argument in arguments {
                    self.expression(&mut argument.value, errors);
                }
                if let Some(block) = trailing_block {
                    self.block(block, errors);
                }
            }
            ExprKind::Tuple(values) | ExprKind::List(values) => {
                for value in values {
                    self.expression(value, errors);
                }
            }
            ExprKind::TypedStructLiteral { type_name, fields } => {
                if type_name == "Self" {
                    if let TypeExprKind::Named(owner) = &self.owner.kind {
                        *type_name = owner.clone();
                    }
                }
                for field in fields {
                    self.expression(&mut field.value, errors);
                }
            }
            ExprKind::StructLiteral(fields) => {
                for field in fields {
                    self.expression(&mut field.value, errors);
                }
            }
            ExprKind::Lambda { parameters, body } => {
                for parameter in parameters {
                    if let Some(ty) = &mut parameter.ty {
                        *ty = self.resolve(ty, errors);
                    }
                }
                self.block(body, errors);
            }
            ExprKind::Block(block) => self.block(block, errors),
            ExprKind::When { value, arms } => {
                self.expression(value, errors);
                for arm in arms {
                    self.block(&mut arm.body, errors);
                }
            }
            _ => {}
        }
    }
}
