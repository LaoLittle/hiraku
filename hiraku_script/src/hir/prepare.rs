//! Declaration normalization. Methods become ordinary functions with an
//! explicitly typed first parameter for instance methods. Static methods have
//! no receiver; values retain their native representation.
use super::LoweringError;
use crate::{Program, Stmt, TypeExprKind};
#[path = "constants.rs"]
mod constants;

pub(super) fn prepare(source: &Program) -> Result<Program, Vec<LoweringError>> {
    static CORE: std::sync::OnceLock<Program> = std::sync::OnceLock::new();
    let core = CORE.get_or_init(|| {
        crate::parse_program(include_str!("../std/core.hks"))
            .expect("the bundled script core library must parse")
    });
    // Import only referenced standard declarations and their dependencies. The
    // resulting bytecode remains self-contained without copying unused helpers.
    let mut needed = crate::normalize_program_symbols(source, None)
        .symbols()
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    // Optional is also referenced by nullable syntax and native Option<T>
    // signatures, which need not contain its source-level name.
    needed.insert("Optional".to_string());
    let mut selected = std::collections::BTreeSet::new();
    let declared = source
        .statements
        .iter()
        .filter_map(|statement| match statement {
            Stmt::Function { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect::<std::collections::BTreeSet<_>>();
    loop {
        let mut changed = false;
        for (index, declaration) in core.statements.iter().enumerate() {
            let referenced = match declaration {
                Stmt::Enum { name, .. }
                | Stmt::TypeAlias { name, .. }
                | Stmt::Struct { name, .. } => needed.contains(name),
                Stmt::Function { name, .. } => {
                    needed.contains(name) && !declared.contains(name.as_str())
                }
                Stmt::Impl { methods, .. } => methods.iter().any(
                    |method| matches!(method, Stmt::Function { name, .. } if needed.contains(name)),
                ),
                _ => false,
            };
            if referenced && selected.insert(index) {
                let unit = Program {
                    statements: vec![declaration.clone()],
                    warnings: Vec::new(),
                };
                needed.extend(
                    crate::normalize_program_symbols(&unit, None)
                        .symbols()
                        .iter()
                        .cloned(),
                );
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut statements = Vec::new();
    let mut errors = Vec::new();
    for statement in source.statements.iter().chain(
        core.statements
            .iter()
            .enumerate()
            .filter_map(|(index, statement)| selected.contains(&index).then_some(statement)),
    ) {
        let Stmt::Impl {
            target,
            methods,
            span,
        } = statement
        else {
            if let Stmt::Function { name, span, .. } = statement {
                if name.starts_with("__builtin_") {
                    errors.push(LoweringError {
                        message: "the `__builtin_` namespace is reserved for compiler intrinsics"
                            .into(),
                        span: *span,
                    });
                }
            }
            statements.push(statement.clone());
            continue;
        };
        let TypeExprKind::Named(owner) = &target.kind else {
            errors.push(LoweringError {
                message: "impl currently requires a concrete named type".into(),
                span: *span,
            });
            continue;
        };
        for method in methods {
            if let Stmt::Const {
                name,
                type_annotation,
                value,
                span,
                ..
            } = method
            {
                let value = match constants::evaluate(value, &Default::default()) {
                    Ok(value) => value,
                    Err(error) => {
                        errors.push(error);
                        continue;
                    }
                };
                statements.push(Stmt::Function {
                    attributes: Vec::new(),
                    exported: false,
                    name: format!("{owner}::get#{name}"),
                    type_parameters: Vec::new(),
                    parameters: Vec::new(),
                    return_type: type_annotation.clone(),
                    body: crate::Block {
                        statements: vec![Stmt::Expr(value)],
                        span: *span,
                    },
                    span: *span,
                });
                continue;
            }
            if let Stmt::Property {
                name,
                ty,
                getter,
                setter,
                span,
            } = method
            {
                let receiver = crate::FunctionParameter {
                    name: "self".into(),
                    ty: Some(target.clone()),
                    span: *span,
                };
                statements.push(Stmt::Function {
                    attributes: Vec::new(),
                    exported: false,
                    name: format!("{owner}::get#{name}"),
                    type_parameters: Vec::new(),
                    parameters: vec![receiver.clone()],
                    return_type: Some(ty.clone()),
                    body: getter.clone(),
                    span: *span,
                });
                if let Some((parameter, body)) = setter {
                    statements.push(Stmt::Function {
                        attributes: Vec::new(),
                        exported: false,
                        name: format!("{owner}::set#{name}"),
                        type_parameters: Vec::new(),
                        parameters: vec![
                            receiver,
                            crate::FunctionParameter {
                                name: parameter.clone(),
                                ty: Some(ty.clone()),
                                span: *span,
                            },
                        ],
                        return_type: Some(crate::TypeExpr {
                            kind: TypeExprKind::Unit,
                            span: *span,
                        }),
                        body: body.clone(),
                        span: *span,
                    });
                }
                continue;
            }
            let mut method = method.clone();
            let Stmt::Function {
                name, parameters, ..
            } = &mut method
            else {
                continue;
            };
            let has_self = parameters.iter().any(|parameter| parameter.name == "self");
            if has_self
                && !parameters
                    .first()
                    .is_some_and(|parameter| parameter.name == "self" && parameter.ty.is_none())
            {
                errors.push(LoweringError {
                    message: "an instance method must start with an unannotated `self` parameter"
                        .into(),
                    span: *span,
                });
                continue;
            }
            if has_self {
                parameters[0].ty = Some(target.clone());
            }
            *name = format!("{owner}::{name}");
            statements.push(method);
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    constants::normalize(&mut statements)?;
    Ok(Program {
        statements,
        warnings: source.warnings.clone(),
    })
}
