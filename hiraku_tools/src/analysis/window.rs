//! Build-only source control flow for bounded, side-effect-free lookahead.
use super::*;
use hiraku_hdp::dependencies::{ResourceGraph, ResourceNode};

type Resources = (BTreeSet<String>, BTreeSet<String>);
pub(super) fn build(
    statements: &[Stmt],
    mut resources: impl FnMut(&Stmt) -> Result<Resources, ToolError>,
) -> Result<ResourceGraph, ToolError> {
    let mut graph = ResourceGraph::default();
    graph.entry = sequence(statements, None, &mut graph, &mut resources)?;
    for statement in statements {
        if let Stmt::Function {
            attributes, name, ..
        } = statement
            && attributes
                .iter()
                .any(|a| matches!(a.name.as_str(), "main" | "ui" | "screen"))
        {
            graph.entry = graph.functions.get(name).copied();
            break;
        }
    }
    Ok(graph)
}

fn sequence(
    statements: &[Stmt],
    next: Option<usize>,
    graph: &mut ResourceGraph,
    resources: &mut impl FnMut(&Stmt) -> Result<Resources, ToolError>,
) -> Result<Option<usize>, ToolError> {
    let mut next = next;
    for stmt in statements.iter().rev() {
        if let Stmt::Function { name, body, .. } = stmt {
            if let Some(entry) = sequence(&body.statements, None, graph, resources)? {
                graph.functions.insert(name.clone(), entry);
            }
            continue;
        }
        if matches!(
            stmt,
            Stmt::Import { .. }
                | Stmt::TypeAlias { .. }
                | Stmt::Struct { .. }
                | Stmt::Enum { .. }
                | Stmt::Extend { .. }
                | Stmt::Protocol { .. }
                | Stmt::Property { .. }
        ) {
            continue;
        }
        let id = graph.nodes.len();
        let span = span(stmt);
        graph.nodes.push(ResourceNode {
            span,
            images: BTreeSet::new(),
            next: next.into_iter().collect(),
            calls: BTreeSet::new(),
        });
        let mut shallow = stmt.clone();
        let mut closures = Vec::new();
        match &mut shallow {
            Stmt::If {
                condition,
                then_block,
                else_block,
                ..
            } => {
                let yes = sequence(&then_block.statements, next, graph, resources)?;
                let no = if let Some(body) = else_block {
                    sequence(&body.statements, next, graph, resources)?
                } else {
                    next
                };
                graph.nodes[id].next = yes.into_iter().chain(no).collect();
                then_block.statements.clear();
                if let Some(body) = else_block {
                    body.statements.clear();
                }
                prune(condition, &mut closures);
            }
            Stmt::While {
                condition, body, ..
            } => {
                let enter = sequence(&body.statements, Some(id), graph, resources)?;
                graph.nodes[id].next = enter.into_iter().chain(next).collect();
                body.statements.clear();
                prune(condition, &mut closures);
            }
            Stmt::Return { value, .. } => {
                graph.nodes[id].next.clear();
                if let Some(value) = value {
                    prune(value, &mut closures);
                }
            }
            Stmt::Global { value, .. } => {
                if let Some(value) = value {
                    prune(value, &mut closures);
                }
            }
            Stmt::Expr(value) | Stmt::Let { value, .. } => prune(value, &mut closures),
            Stmt::Assign { target, value, .. } => {
                prune(target, &mut closures);
                prune(value, &mut closures);
            }
            _ => (),
        }
        let (images, calls) = resources(&shallow)?;
        graph.nodes[id].images = images;
        graph.nodes[id].calls = calls;
        // Closures are possible execution paths, not eagerly evaluated values.
        // Parent continuation is retained as another possible path.
        for closure in closures {
            if let Some(entry) = sequence(&closure.statements, None, graph, resources)? {
                graph.nodes[id].next.push(entry);
            }
        }
        next = Some(id);
    }
    Ok(next)
}

fn span(stmt: &Stmt) -> [usize; 2] {
    let span = match stmt {
        Stmt::Expr(expr) => expr.span,
        Stmt::Return { span, .. }
        | Stmt::Property { span, .. }
        | Stmt::Extend { span, .. }
        | Stmt::Protocol { span, .. }
        | Stmt::Import { span, .. }
        | Stmt::TypeAlias { span, .. }
        | Stmt::Struct { span, .. }
        | Stmt::Enum { span, .. }
        | Stmt::Function { span, .. }
        | Stmt::Let { span, .. }
        | Stmt::Global { span, .. }
        | Stmt::Assign { span, .. }
        | Stmt::If { span, .. }
        | Stmt::While { span, .. } => *span,
    };
    [span.start, span.end]
}

fn prune(expr: &mut Expr, closures: &mut Vec<Block>) {
    match &mut expr.kind {
        ExprKind::When { value, arms } => {
            prune(value, closures);
            for arm in arms {
                closures.push(arm.body.clone());
                arm.body.statements.clear();
            }
        }
        ExprKind::Lambda { body, .. } | ExprKind::Block(body) => {
            closures.push(body.clone());
            body.statements.clear();
        }
        ExprKind::Call {
            callee,
            arguments,
            trailing_block,
            ..
        } => {
            prune(callee, closures);
            for arg in arguments {
                prune(&mut arg.value, closures);
            }
            if let Some(body) = trailing_block.take() {
                closures.push(body);
            }
        }
        ExprKind::Member { object, .. }
        | ExprKind::SafeMember { object, .. }
        | ExprKind::UnaryMinus(object)
        | ExprKind::Not(object)
        | ExprKind::NonNull(object)
        | ExprKind::Binding(object)
        | ExprKind::Cast { value: object, .. } => prune(object, closures),
        ExprKind::Binary { left, right, .. }
        | ExprKind::Elvis {
            value: left,
            fallback: right,
        } => {
            prune(left, closures);
            prune(right, closures);
        }
        ExprKind::Tuple(values) | ExprKind::List(values) => {
            for value in values {
                prune(value, closures);
            }
        }
        ExprKind::StructLiteral(fields) | ExprKind::TypedStructLiteral { fields, .. } => {
            for field in fields {
                prune(&mut field.value, closures);
            }
        }
        _ => (),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branches_closures_and_function_bodies_have_independent_resource_nodes() {
        let script = r#"
global fn helper() { bg("bob") }
global let alice = char("alice")
if true { bg("alice") } else { bg("bob") }
while false { helper() }
choice { option("A") { bg("alice") }; option("B") { bg("bob") } }
"#;
        let docs = BTreeMap::from([
            ("entry.hks".into(), script.into()),
            (
                "alice.texture.hson".into(),
                ".{name: \"alice\", image: \"alice.png\"}".into(),
            ),
            (
                "bob.texture.hson".into(),
                ".{name: \"bob\", image: \"bob.png\"}".into(),
            ),
        ]);
        let manifest = analyze(&docs).expect("analyze synthetic control flow");
        let graph = &manifest.windows["entry.hks"];
        assert_eq!(manifest.exports["helper"], "entry.hks");
        let helper = graph.functions["helper"];
        assert_eq!(
            graph.nodes[helper].images,
            BTreeSet::from(["bob.png".into()])
        );
        let conditional = graph
            .nodes
            .iter()
            .find(|n| script[n.span[0]..n.span[1]].starts_with("if "))
            .expect("if node");
        assert!(
            conditional.images.is_empty(),
            "branches are not collapsed into their parent"
        );
        assert_eq!(conditional.next.len(), 2);
        let branch_images: BTreeSet<_> = conditional
            .next
            .iter()
            .flat_map(|id| graph.nodes[*id].images.iter().cloned())
            .collect();
        assert_eq!(
            branch_images,
            BTreeSet::from(["alice.png".into(), "bob.png".into()])
        );
        let choice = graph
            .nodes
            .iter()
            .find(|n| script[n.span[0]..n.span[1]].starts_with("choice "))
            .expect("choice node");
        assert!(
            choice.images.is_empty(),
            "choice branches are distinct future paths"
        );
        assert!(!choice.next.is_empty());
        let call = graph
            .nodes
            .iter()
            .find(|n| script[n.span[0]..n.span[1]] == *"helper()")
            .expect("call node");
        assert!(
            call.images.is_empty(),
            "function calls follow graph edges instead of collapsing the body"
        );
        assert!(call.calls.contains("helper"));
        assert_ne!(
            graph.entry,
            Some(helper),
            "defining a function does not execute it"
        );
    }
}
