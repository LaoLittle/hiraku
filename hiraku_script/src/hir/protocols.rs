//! Static protocol conformance checking before ordinary method lowering.
use super::LoweringError;
use crate::{Program, Stmt, TypeExpr, TypeExprKind};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn resolve(source: &Program, core: &Program) -> Result<Program, Vec<LoweringError>> {
    let mut declarations = BTreeMap::new();
    let mut errors = Vec::new();
    for statement in core.statements.iter().chain(&source.statements) {
        if let Stmt::Protocol {
            name,
            type_parameters,
            associated_types,
            methods,
            span,
        } = statement
        {
            if declarations.insert(name.clone(), statement).is_some() {
                errors.push(LoweringError {
                    message: format!("protocol `{name}` is defined more than once"),
                    span: *span,
                });
            }
            let mut names = BTreeSet::new();
            for name in type_parameters.iter().chain(associated_types) {
                if name == "Self" || !names.insert(name) {
                    errors.push(LoweringError {
                        message: format!("duplicate or reserved protocol type `{name}`"),
                        span: *span,
                    });
                }
            }
            let mut method_names = BTreeSet::new();
            for method in methods {
                if let Stmt::Function {
                    name,
                    parameters,
                    return_type,
                    type_parameters,
                    span,
                    ..
                } = method
                {
                    if !method_names.insert(name)
                        || !type_parameters.is_empty()
                        || return_type.is_none()
                        || !parameters.first().is_some_and(|parameter| {
                            parameter.name == "self" && parameter.ty.is_none()
                        })
                        || parameters
                            .iter()
                            .skip(1)
                            .any(|parameter| parameter.ty.is_none() || parameter.name == "self")
                    {
                        errors.push(LoweringError { message: "protocol methods require a unique name, first `self` receiver, typed operands and explicit return type; generic methods are not supported yet".into(), span: *span });
                    }
                }
            }
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    let mut result = source.clone();
    for statement in result.statements.iter_mut().flat_map(|statement| {
        if let Stmt::Extend { methods, .. } = statement {
            methods.iter_mut().collect::<Vec<_>>()
        } else {
            vec![statement]
        }
    }) {
        let Stmt::Function {
            bounds,
            witnesses,
            span,
            ..
        } = statement
        else {
            continue;
        };
        witnesses.clear();
        let mut seen_bounds = BTreeSet::new();
        for bound in bounds {
            let (name, arguments) = match &bound.protocol.kind {
                TypeExprKind::Named(name) => (name, &[][..]),
                TypeExprKind::Applied { name, arguments } => (name, arguments.as_slice()),
                _ => {
                    errors.push(LoweringError {
                        message: "expected a protocol constraint".into(),
                        span: bound.protocol.span,
                    });
                    continue;
                }
            };
            let Some(Stmt::Protocol {
                type_parameters,
                associated_types,
                methods,
                ..
            }) = declarations.get(name).copied()
            else {
                errors.push(LoweringError {
                    message: format!("unknown protocol `{name}` in generic constraint"),
                    span: bound.protocol.span,
                });
                continue;
            };
            if !seen_bounds.insert((bound.parameter.clone(), name.clone())) {
                errors.push(LoweringError { message: "duplicate protocol bound; multiple argument specializations of one bound are not supported yet".into(), span: bound.protocol.span });
                continue;
            }
            if type_parameters.len() != arguments.len() {
                errors.push(LoweringError {
                    message: format!(
                        "protocol `{name}` expects {} type arguments",
                        type_parameters.len()
                    ),
                    span: bound.protocol.span,
                });
                continue;
            }
            if !associated_types.is_empty() || methods.is_empty() {
                errors.push(LoweringError { message: "associated-type constraints and marker-only protocol bounds are not supported yet".into(), span: bound.protocol.span });
                continue;
            }
            let mut substitutions: BTreeMap<String, TypeExpr> = type_parameters
                .iter()
                .cloned()
                .zip(arguments.iter().cloned())
                .collect();
            substitutions.insert(
                "Self".into(),
                TypeExpr {
                    kind: TypeExprKind::Named(bound.parameter.clone()),
                    span: *span,
                },
            );
            for method in methods {
                let Stmt::Function {
                    name: method,
                    parameters,
                    return_type: Some(return_type),
                    ..
                } = method
                else {
                    continue;
                };
                witnesses.push(crate::ast::ProtocolWitness {
                    parameter: bound.parameter.clone(),
                    protocol: name.clone(),
                    method: method.clone(),
                    parameters: parameters
                        .iter()
                        .map(|parameter| {
                            if parameter.name == "self" {
                                substitutions["Self"].clone()
                            } else {
                                substitute(
                                    parameter.ty.as_ref().expect("protocol signature checked"),
                                    &substitutions,
                                )
                            }
                        })
                        .collect(),
                    result: substitute(return_type, &substitutions),
                });
            }
        }
    }
    result
        .statements
        .retain(|statement| !matches!(statement, Stmt::Protocol { .. }));
    for statement in &mut result.statements {
        let Stmt::Extend {
            target,
            protocol: Some(protocol),
            methods,
            span,
        } = statement
        else {
            continue;
        };
        let (name, arguments) = match &protocol.kind {
            TypeExprKind::Named(name) => (name, &[][..]),
            TypeExprKind::Applied { name, arguments } => (name, arguments.as_slice()),
            _ => {
                errors.push(LoweringError {
                    message: "expected a named protocol".into(),
                    span: *span,
                });
                continue;
            }
        };
        let Some(Stmt::Protocol {
            type_parameters,
            associated_types,
            methods: requirements,
            ..
        }) = declarations.get(name).copied()
        else {
            errors.push(LoweringError {
                message: format!("unknown protocol `{name}`"),
                span: protocol.span,
            });
            continue;
        };
        if type_parameters.len() != arguments.len() {
            errors.push(LoweringError {
                message: format!(
                    "protocol `{name}` expects {} type arguments",
                    type_parameters.len()
                ),
                span: protocol.span,
            });
            continue;
        }
        let mut substitutions: BTreeMap<String, TypeExpr> = type_parameters
            .iter()
            .cloned()
            .zip(arguments.iter().cloned())
            .collect();
        substitutions.insert("Self".into(), target.clone());
        let mut associated = BTreeSet::new();
        for member in methods.iter() {
            if let Stmt::TypeAlias {
                name: member,
                type_parameters,
                ty,
                span,
            } = member
            {
                if !associated_types.contains(member)
                    || !associated.insert(member.clone())
                    || !type_parameters.is_empty()
                {
                    errors.push(LoweringError {
                        message: format!("invalid or duplicate associated type `{member}`"),
                        span: *span,
                    });
                }
                substitutions.insert(member.clone(), ty.clone());
                substitutions.insert(format!("Self.{member}"), ty.clone());
            }
        }
        for member in associated_types {
            if !associated.contains(member) {
                errors.push(LoweringError {
                    message: format!("missing associated type `{member}` for `{name}`"),
                    span: *span,
                });
            }
        }
        let mut seen = BTreeSet::new();
        methods.retain(|method| !matches!(method, Stmt::TypeAlias { .. }));
        for method in methods.iter_mut() {
            let Stmt::Function {
                name: method_name,
                parameters,
                return_type,
                type_parameters: generics,
                span: method_span,
                ..
            } = method
            else {
                errors.push(LoweringError {
                    message: "protocol implementations require function members".into(),
                    span: *span,
                });
                continue;
            };
            let requirement = requirements.iter().find(|requirement| matches!(requirement, Stmt::Function { name, .. } if name == method_name));
            let Some(Stmt::Function {
                parameters: required,
                return_type: required_result,
                ..
            }) = requirement
            else {
                errors.push(LoweringError {
                    message: format!("`{method_name}` is not a requirement of `{name}`"),
                    span: *method_span,
                });
                continue;
            };
            if !seen.insert(method_name.clone())
                || parameters.len() != required.len()
                || !generics.is_empty()
            {
                errors.push(LoweringError {
                    message: format!(
                        "invalid signature or duplicate implementation of `{name}.{method_name}`"
                    ),
                    span: *method_span,
                });
                continue;
            }
            for (actual, expected) in parameters.iter_mut().zip(required) {
                if expected.name == "self" {
                    if actual.name != "self" || actual.ty.is_some() {
                        errors.push(LoweringError {
                            message: "protocol receiver must be unannotated `self`".into(),
                            span: actual.span,
                        });
                    }
                    continue;
                }
                let expected = expected
                    .ty
                    .as_ref()
                    .map(|ty| substitute(ty, &substitutions));
                let actual_type = actual.ty.as_ref().map(|ty| substitute(ty, &substitutions));
                if expected.is_none()
                    || actual_type.as_ref().map(canonical) != expected.as_ref().map(canonical)
                {
                    errors.push(LoweringError {
                        message: format!(
                            "parameter `{}` does not match protocol `{name}.{method_name}`",
                            actual.name
                        ),
                        span: actual.span,
                    });
                }
                actual.ty = expected;
            }
            let expected = required_result
                .as_ref()
                .map(|ty| substitute(ty, &substitutions));
            if let Some(actual) = return_type.as_ref() {
                if Some(canonical(&substitute(actual, &substitutions)))
                    != expected.as_ref().map(canonical)
                {
                    errors.push(LoweringError {
                        message: format!(
                            "return type does not match protocol `{name}.{method_name}`"
                        ),
                        span: actual.span,
                    });
                }
            }
            *return_type = expected;
            *method_name = format!("protocol#{name}#{method_name}");
        }
        for requirement in requirements {
            if let Stmt::Function { name: member, .. } = requirement {
                if !seen.contains(member) {
                    errors.push(LoweringError {
                        message: format!("missing implementation of `{name}.{member}`"),
                        span: *span,
                    });
                }
            }
        }
    }
    if errors.is_empty() {
        Ok(result)
    } else {
        Err(errors)
    }
}

fn substitute(ty: &TypeExpr, substitutions: &BTreeMap<String, TypeExpr>) -> TypeExpr {
    if let TypeExprKind::Named(name) = &ty.kind {
        if let Some(value) = substitutions.get(name) {
            let mut remaining = substitutions.clone();
            remaining.remove(name);
            return substitute(value, &remaining);
        }
    }
    let mut result = ty.clone();
    match &mut result.kind {
        TypeExprKind::Tuple(types)
        | TypeExprKind::Applied {
            arguments: types, ..
        } => {
            for ty in types {
                *ty = substitute(ty, substitutions);
            }
        }
        TypeExprKind::Function { parameters, result } => {
            for ty in parameters {
                *ty = substitute(ty, substitutions);
            }
            **result = substitute(result, substitutions);
        }
        TypeExprKind::Nullable(ty) | TypeExprKind::List(ty) | TypeExprKind::Binding(ty) => {
            **ty = substitute(ty, substitutions)
        }
        TypeExprKind::Record(fields) => {
            for field in fields {
                field.ty = substitute(&field.ty, substitutions);
            }
        }
        _ => {}
    }
    result
}

fn canonical(ty: &TypeExpr) -> TypeExpr {
    let mut result = substitute(ty, &BTreeMap::new());
    fn clear(ty: &mut TypeExpr) {
        ty.span = crate::Span { start: 0, end: 0 };
        if matches!(&ty.kind, TypeExprKind::Named(name) if name == "Unit") {
            ty.kind = TypeExprKind::Unit;
        }
        match &mut ty.kind {
            TypeExprKind::Tuple(types)
            | TypeExprKind::Applied {
                arguments: types, ..
            } => types.iter_mut().for_each(clear),
            TypeExprKind::Function { parameters, result } => {
                parameters.iter_mut().for_each(clear);
                clear(result);
            }
            TypeExprKind::Nullable(ty) | TypeExprKind::List(ty) | TypeExprKind::Binding(ty) => {
                clear(ty)
            }
            TypeExprKind::Record(fields) => {
                for field in fields {
                    field.span = crate::Span { start: 0, end: 0 };
                    clear(&mut field.ty);
                }
            }
            _ => {}
        }
    }
    clear(&mut result);
    result
}
