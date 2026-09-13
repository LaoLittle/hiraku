use super::ToolError;
mod window;
use hiraku_hdp::dependencies::{DEPENDENCY_VERSION, DependencyManifest};
use hiraku_script::{
    Block, Expr, ExprKind, Stmt,
    hson::{self, HsonValue},
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Default)]
struct Facts {
    strings: BTreeSet<String>,
    actors: BTreeSet<String>,
    paths: BTreeSet<String>,
    symbols: BTreeSet<String>,
    dynamic: BTreeSet<String>,
    values: BTreeMap<String, ValueSet>,
    calls: Vec<(String, Vec<ValueSet>)>,
    returns: ValueSet,
}
impl Facts {
    fn merge(&mut self, other: &Self) {
        self.strings.extend(other.strings.iter().cloned());
        self.actors.extend(other.actors.iter().cloned());
        self.paths.extend(other.paths.iter().cloned());
        self.symbols.extend(other.symbols.iter().cloned());
        self.dynamic.extend(other.dynamic.iter().cloned());
    }
}

/// Includes both branches and closures; follows function/global references,
/// not navigation edges. Unknown computed resource names remain demand-loaded
/// and are recorded explicitly. Source graphs allow bounded runtime lookahead.
pub fn analyze(documents: &BTreeMap<String, String>) -> Result<DependencyManifest, ToolError> {
    let mut textures = BTreeMap::<String, BTreeSet<String>>::new();
    let mut characters = BTreeMap::<String, BTreeSet<String>>::new();
    for (path, source) in documents
        .iter()
        .filter(|(p, _)| p.ends_with(".texture.hson"))
    {
        let value = hson::parse(source).map_err(|e| format!("{path}: {e}"))?;
        let map = value
            .as_map()
            .ok_or_else(|| format!("{path}: expected object"))?;
        let image = map
            .get("image")
            .and_then(HsonValue::as_str)
            .ok_or_else(|| format!("{path}: missing image"))?;
        let image = resolve(path, image)?;
        if let Some(name) = map.get("name").and_then(HsonValue::as_str) {
            textures
                .entry(name.into())
                .or_default()
                .insert(image.clone());
        }
        if let Some(regions) = map.get("regions").and_then(HsonValue::as_map) {
            for name in regions.keys() {
                textures
                    .entry(name.clone())
                    .or_default()
                    .insert(image.clone());
            }
        }
    }
    for (path, source) in documents.iter().filter(|(p, _)| p.ends_with(".char.hson")) {
        let value = hson::parse(source).map_err(|e| format!("{path}: {e}"))?;
        let map = value
            .as_map()
            .ok_or_else(|| format!("{path}: expected object"))?;
        let Some(name) = map.get("name").and_then(HsonValue::as_str) else {
            continue;
        };
        let images = characters.entry(name.into()).or_default();
        if let Some(parts) = map.get("parts").and_then(HsonValue::as_map) {
            for part in parts.values().filter_map(HsonValue::as_map) {
                if let Some(texture) = part.get("texture").and_then(HsonValue::as_str) {
                    images.extend(
                        textures
                            .get(texture)
                            .ok_or_else(|| format!("{path}: unknown texture {texture}"))?
                            .iter()
                            .cloned(),
                    );
                }
                if let Some(image) = part.get("path").and_then(HsonValue::as_str) {
                    images.insert(resolve(path, image)?);
                }
            }
        }
    }
    let mut scripts = BTreeMap::new();
    let mut definitions = BTreeMap::<String, Facts>::new();
    let mut definition_owners = BTreeMap::<String, String>::new();
    let mut local_definitions = BTreeMap::<(String, String), Facts>::new();
    let mut function_names = BTreeSet::new();
    let mut exports = BTreeMap::new();
    let programs: BTreeMap<_, _> = documents
        .iter()
        .filter(|(p, _)| p.ends_with(".hks"))
        .map(|(path, source)| {
            hiraku_script::parse_program(source)
                .map(|p| (path.clone(), p))
                .map_err(|e| format!("{path}: {e:?}"))
        })
        .collect::<Result<_, _>>()?;
    let ui_files: BTreeSet<_> = programs.iter().filter(|(path, program)| {
        is_ui(path) || program.statements.iter().any(|stmt| matches!(stmt,
            Stmt::Function { attributes, .. } if attributes.iter().any(|a| a.name == "ui" || a.name == "screen")))
    }).map(|(path, _)| path.clone()).collect();
    for (path, program) in &programs {
        let mut facts = Facts::default();
        for stmt in &program.statements {
            match stmt {
                Stmt::Function {
                    name,
                    attributes,
                    exported,
                    ..
                } => {
                    function_names.insert((path.clone(), name.clone()));
                    if *exported && !ui_files.contains(path) {
                        exports.insert(name.clone(), path.clone());
                    }
                    let mut body = Facts::default();
                    visit_stmt(stmt, &mut body);
                    local_definitions.insert((path.clone(), name.clone()), body.clone());
                    if *exported && !ui_files.contains(path) {
                        definitions.entry(name.clone()).or_default().merge(&body);
                        definition_owners.insert(name.clone(), path.clone());
                    }
                    if attributes
                        .iter()
                        .any(|a| a.name == "ui" || a.name == "screen" || a.name == "main")
                    {
                        facts.merge(&body);
                    }
                }
                Stmt::Global { name, .. }
                | Stmt::Const {
                    name,
                    exported: true,
                    ..
                } => {
                    let mut body = Facts::default();
                    visit_stmt(stmt, &mut body);
                    local_definitions.insert((path.clone(), name.clone()), body.clone());
                    if !ui_files.contains(path) {
                        definitions.entry(name.clone()).or_default().merge(&body);
                        definition_owners.insert(name.clone(), path.clone());
                    }
                    facts.merge(&body);
                }
                _ => visit_stmt(stmt, &mut facts),
            }
        }
        scripts.insert(path.clone(), facts);
    }
    let mut result = DependencyManifest {
        windows: BTreeMap::new(),
        exports,
        image_bytes: BTreeMap::new(),
        version: DEPENDENCY_VERSION,
        scripts: BTreeMap::new(),
        resident: BTreeSet::new(),
        conservative: BTreeMap::new(),
    };
    for (path, facts) in scripts {
        let mut scope = BTreeMap::new();
        for (module, program) in &programs {
            if module == &path {
                scope.insert(module.clone(), program.clone());
            } else if !ui_files.contains(module) {
                let mut exports = program.clone();
                exports.statements.retain(|s| {
                    matches!(
                        s,
                        Stmt::Function { exported: true, .. }
                            | Stmt::Global { .. }
                            | Stmt::Const { exported: true, .. }
                    )
                });
                scope.insert(module.clone(), exports);
            }
        }
        let values = value_flow(&scope);
        let resolve = |mut facts: Facts, follow_functions: bool| -> Result<_, ToolError> {
            let mut visited = BTreeSet::new();
            let mut pending: BTreeSet<_> = facts
                .symbols
                .iter()
                .map(|name| (path.clone(), name.clone()))
                .collect();
            while let Some((module, name)) = pending.pop_first() {
                if !visited.insert((module.clone(), name.clone())) {
                    continue;
                }
                let definition = local_definitions
                    .get(&(module.clone(), name.clone()))
                    .map(|body| (&module, body))
                    .or_else(|| {
                        definitions
                            .get(&name)
                            .map(|body| (&definition_owners[&name], body))
                    });
                if let Some((owner, body)) = definition {
                    if !follow_functions && function_names.contains(&(owner.clone(), name.clone()))
                    {
                        continue;
                    }
                    pending.extend(
                        body.symbols
                            .iter()
                            .map(|symbol| (owner.clone(), symbol.clone())),
                    );
                    facts.merge(body);
                }
            }
            let mut images = BTreeSet::new();
            for name in &facts.strings {
                if let Some(paths) = textures.get(name) {
                    images.extend(paths.iter().cloned());
                }
            }
            for image in &facts.paths {
                images.insert(resolve(&path, image)?);
            }
            for actor in &facts.actors {
                if let Some(paths) = characters.get(actor) {
                    images.extend(paths.iter().cloned());
                }
            }
            let mut unresolved = BTreeSet::new();
            for query in &facts.dynamic {
                let (_, symbol) = query.split_once(':').expect("internal resource query");
                if let Some(value) = values
                    .get(symbol)
                    .filter(|v| !v.unknown && !v.strings.is_empty())
                {
                    for name in &value.strings {
                        if let Some(paths) = textures.get(name).or_else(|| characters.get(name)) {
                            images.extend(paths.iter().cloned());
                        }
                        if is_image(name) && !name.contains("://") {
                            images.insert(resolve(&path, name)?);
                        }
                    }
                } else {
                    // Unknown references are demand-loaded by the runtime, for
                    // stories as well as UI. An optimization must never turn one
                    // dynamic parameter into a preload of the entire game.
                    unresolved.insert(query.clone());
                }
            }
            Ok((images, unresolved))
        };
        let graph = window::build(&programs[&path].statements, |stmt| {
            let mut facts = Facts::default();
            visit_stmt(stmt, &mut facts);
            let calls = facts.calls.iter().map(|(name, _)| name.clone()).collect();
            let (images, _) = resolve(facts, false)?;
            Ok((images, calls))
        })?;
        result.windows.insert(path.clone(), graph);
        let (images, unresolved) = resolve(facts, true)?;
        if !unresolved.is_empty() {
            result.conservative.insert(path.clone(), unresolved);
        }
        if ui_files.contains(&path) {
            result.resident.extend(images.iter().cloned());
        }
        result.scripts.insert(path, images);
    }
    // Native calls have no script body to traverse. Keep only resolvable
    // script edges, reducing package metadata and runtime graph work.
    for graph in result.windows.values_mut() {
        for node in &mut graph.nodes {
            node.calls.retain(|name| {
                graph.functions.contains_key(name) || result.exports.contains_key(name)
            });
        }
    }
    Ok(result)
}

fn is_image(path: &str) -> bool {
    [".png", ".jpg", ".jpeg", ".webp", ".uastc.ktx2"]
        .iter()
        .any(|e| path.ends_with(e))
}
fn is_ui(path: &str) -> bool {
    path.ends_with(".ui.hks") || path.split('/').any(|p| p == "ui")
}

#[derive(Clone, Default, PartialEq, Eq)]
struct ValueSet {
    strings: BTreeSet<String>,
    refs: BTreeSet<String>,
    unknown: bool,
}
impl ValueSet {
    fn merge(&mut self, other: &Self) {
        self.strings.extend(other.strings.iter().cloned());
        self.refs.extend(other.refs.iter().cloned());
        self.unknown |= other.unknown;
    }
}
fn value_set(value: &Expr) -> ValueSet {
    let mut set = ValueSet::default();
    match &value.kind {
        ExprKind::String(s) if !s.contains("${") => {
            set.strings.insert(s.clone());
        }
        ExprKind::Ident(s) => {
            set.refs.insert(s.clone());
        }
        ExprKind::Call { callee, .. } if callable(callee) == "storage.thumbnail" => {
            // Runtime-generated images belong to storage, not the package.
            set.strings.insert("save-thumbnail://*".into());
        }
        ExprKind::Call { callee, .. } => {
            set.refs.insert(format!("return:{}", callable(callee)));
        }
        ExprKind::Elvis { value, fallback } => {
            set.merge(&value_set(value));
            set.merge(&value_set(fallback));
        }
        ExprKind::Cast { value, .. } | ExprKind::NonNull(value) => set.merge(&value_set(value)),
        ExprKind::Bool(_)
        | ExprKind::Number { .. }
        | ExprKind::Null
        | ExprKind::Unit
        | ExprKind::Symbol(_) => (),
        _ => set.unknown = true,
    }
    set
}

/// Finite string-set propagation through assignments, function arguments and
/// returns. Union at branches/loops deliberately over-approximates execution.
/// Native/dynamic results remain unknown rather than guessing resource names.
fn value_flow(programs: &BTreeMap<String, hiraku_script::Program>) -> BTreeMap<String, ValueSet> {
    let mut values = BTreeMap::<String, ValueSet>::new();
    let mut functions = BTreeMap::<String, Vec<String>>::new();
    let mut calls = Vec::new();
    for program in programs.values() {
        for stmt in &program.statements {
            let mut facts = Facts::default();
            visit_stmt(stmt, &mut facts);
            if let Stmt::Function {
                name,
                parameters,
                body,
                ..
            } = stmt
            {
                functions.insert(
                    name.clone(),
                    parameters.iter().map(|p| p.name.clone()).collect(),
                );
                if let Some(Stmt::Expr(tail)) = body.statements.last() {
                    facts.returns.merge(&value_set(tail));
                }
                values
                    .entry(format!("return:{name}"))
                    .or_default()
                    .merge(&facts.returns);
            }
            for (name, value) in facts.values {
                values.entry(name).or_default().merge(&value);
            }
            calls.extend(facts.calls);
        }
    }
    for (name, arguments) in calls {
        if let Some(parameters) = functions.get(&name) {
            for (parameter, value) in parameters.iter().zip(arguments) {
                values.entry(parameter.clone()).or_default().merge(&value);
            }
        }
    }
    loop {
        let previous = values.clone();
        for value in values.values_mut() {
            for reference in value.refs.clone() {
                if let Some(referenced) = previous.get(&reference) {
                    value.merge(referenced);
                } else {
                    value.unknown = true;
                }
            }
        }
        if values == previous {
            break;
        }
    }
    values
}
fn resolve(base: &str, path: &str) -> Result<String, ToolError> {
    if path.contains("://") {
        return Ok(path.into());
    }
    let parent = base.rsplit_once('/').map(|p| p.0).unwrap_or("");
    let joined = if path.starts_with('/') {
        path.trim_start_matches('/').into()
    } else {
        format!("{parent}/{path}")
    };
    let mut parts = Vec::new();
    for part in joined.split('/') {
        match part {
            "" | "." => (),
            ".." => {
                if parts.pop().is_none() {
                    return Err(format!("resource escapes package: {base} -> {path}").into());
                }
            }
            p => parts.push(p),
        }
    }
    Ok(parts.join("/"))
}
fn block(block: &Block, out: &mut Facts) {
    for stmt in &block.statements {
        visit_stmt(stmt, out);
    }
}
fn visit_stmt(stmt: &Stmt, out: &mut Facts) {
    match stmt {
        Stmt::Let { name, value, .. } | Stmt::Const { name, value, .. } => {
            out.values
                .entry(name.clone())
                .or_default()
                .merge(&value_set(value));
        }
        Stmt::Global {
            name,
            value: Some(value),
            ..
        } => {
            out.values
                .entry(name.clone())
                .or_default()
                .merge(&value_set(value));
        }
        Stmt::Assign {
            target:
                Expr {
                    kind: ExprKind::Ident(name),
                    ..
                },
            value,
            ..
        } => {
            out.values
                .entry(name.clone())
                .or_default()
                .merge(&value_set(value));
        }
        Stmt::Return {
            value: Some(value), ..
        } => out.returns.merge(&value_set(value)),
        _ => (),
    }
    match stmt {
        Stmt::Return { value, .. } | Stmt::Global { value, .. } => {
            if let Some(v) = value {
                expr(v, out);
            }
        }
        Stmt::Const { value, .. } | Stmt::Let { value, .. } | Stmt::Expr(value) => expr(value, out),
        Stmt::Assign { target, value, .. } => {
            expr(target, out);
            expr(value, out);
        }
        Stmt::Property { getter, setter, .. } => {
            block(getter, out);
            if let Some((_, b)) = setter {
                block(b, out);
            }
        }
        Stmt::Extend { methods, .. } | Stmt::Protocol { methods, .. } => {
            for s in methods {
                visit_stmt(s, out);
            }
        }
        Stmt::Function { body, .. } => block(body, out),
        Stmt::If {
            condition,
            then_block,
            else_block,
            ..
        } => {
            expr(condition, out);
            block(then_block, out);
            if let Some(b) = else_block {
                block(b, out);
            }
        }
        Stmt::While {
            condition, body, ..
        } => {
            expr(condition, out);
            block(body, out);
        }
        Stmt::Import { .. } | Stmt::TypeAlias { .. } | Stmt::Struct { .. } | Stmt::Enum { .. } => {
            ()
        }
    }
}
fn callable(expr: &Expr) -> String {
    match &expr.kind {
        ExprKind::Ident(n) => n.clone(),
        ExprKind::Member { object, name } => format!("{}.{name}", callable(object)),
        _ => String::new(),
    }
}
fn expr(value: &Expr, out: &mut Facts) {
    match &value.kind {
        ExprKind::When { value, arms } => {
            expr(value, out);
            for arm in arms {
                block(&arm.body, out);
            }
        }
        ExprKind::String(s) => {
            out.strings.insert(s.clone());
        }
        ExprKind::Ident(s) => {
            out.symbols.insert(s.clone());
        }
        ExprKind::Member { object, .. } | ExprKind::SafeMember { object, .. } => expr(object, out),
        ExprKind::Binding(v)
        | ExprKind::UnaryMinus(v)
        | ExprKind::Not(v)
        | ExprKind::NonNull(v)
        | ExprKind::Cast { value: v, .. } => expr(v, out),
        ExprKind::Elvis { value, fallback } => {
            expr(value, out);
            expr(fallback, out);
        }
        ExprKind::Binary { left, right, .. } => {
            expr(left, out);
            expr(right, out);
        }
        ExprKind::Call {
            callee,
            arguments,
            trailing_block,
            ..
        } => {
            let name = callable(callee);
            if name == "char" {
                if let Some(hiraku_script::Argument {
                    value:
                        Expr {
                            kind: ExprKind::String(actor),
                            ..
                        },
                    ..
                }) = arguments.first()
                {
                    out.actors.insert(actor.clone());
                }
            }
            out.calls.push((
                name.clone(),
                arguments.iter().map(|a| value_set(&a.value)).collect(),
            ));
            let index = match name.as_str() {
                "char" | "bg" | "cg" | "image" | "ui.widgets.image" => Some(0),
                "scene.picture" => Some(1),
                _ => None,
            };
            if let Some(index) = index {
                if let Some(arg) = arguments.get(index) {
                    if let ExprKind::String(path) = &arg.value.kind {
                        if is_image(path) && !path.contains("${") {
                            out.paths.insert(path.clone());
                        }
                    }
                    if !matches!(&arg.value.kind, ExprKind::String(s) if !s.contains("${")) {
                        let symbol = match &arg.value.kind {
                            ExprKind::Ident(s) => s.clone(),
                            ExprKind::Call { callee, .. } => format!("return:{}", callable(callee)),
                            _ => "*".into(),
                        };
                        out.dynamic.insert(format!(
                            "{}:{symbol}",
                            if name == "char" { "char" } else { "texture" }
                        ));
                    }
                }
            }
            expr(callee, out);
            for a in arguments {
                expr(&a.value, out);
            }
            if let Some(b) = trailing_block {
                block(b, out);
            }
        }
        ExprKind::Tuple(v) | ExprKind::List(v) => {
            for v in v {
                expr(v, out);
            }
        }
        ExprKind::StructLiteral(fields) | ExprKind::TypedStructLiteral { fields, .. } => {
            for f in fields {
                expr(&f.value, out);
            }
        }
        ExprKind::Lambda { body, .. } | ExprKind::Block(body) => block(body, out),
        _ => (),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn open_ended_ui_images_do_not_make_the_texture_catalog_resident() {
        let docs = BTreeMap::from([
            (
                "ui.texture.hson".into(),
                ".{ name: \"ui/frame\", image: \"ui.png\" }".into(),
            ),
            (
                "alice.texture.hson".into(),
                ".{ name: \"alice\", image: \"alice.png\" }".into(),
            ),
            (
                "bob.texture.hson".into(),
                ".{ name: \"bob\", image: \"bob.png\" }".into(),
            ),
            (
                "ui/view.ui.hks".into(),
                "@ui fn view(name: String) { image(\"ui/frame\"); image(name) }".into(),
            ),
        ]);
        let manifest = analyze(&docs).expect("analyze dynamic UI");
        assert_eq!(manifest.resident, BTreeSet::from(["ui.png".into()]));
        assert!(!manifest.scripts["ui/view.ui.hks"].contains("alice.png"));
        assert!(!manifest.scripts["ui/view.ui.hks"].contains("bob.png"));
    }
    #[test]
    fn parameterized_ui_and_returned_images_do_not_pin_unrelated_textures() {
        let docs = BTreeMap::from([
            ("atlas.texture.hson".into(), ".{ name: \"ui/button\", image: \"ui.png\" }".into()),
            ("alice.texture.hson".into(), ".{ name: \"alice\", image: \"alice.png\" }".into()),
            ("bob.texture.hson".into(), ".{ name: \"bob\", image: \"bob.png\" }".into()),
            ("ui/main.ui.hks".into(), "fn art(name: String) { image(name) }; fn selected() -> String { if true { return \"alice\" }; return \"ui/button\" }; @ui fn main() { art(selected()); art(storage.thumbnail(\"slot\")!); text(\"missing.png\") }".into()),
        ]);
        let manifest = analyze(&docs).expect("analyze parameter flow");
        assert_eq!(
            manifest.resident,
            BTreeSet::from(["ui.png".into(), "alice.png".into()])
        );
        assert!(manifest.conservative.is_empty());
    }

    #[test]
    fn dialogue_character_names_do_not_preload_character_art() {
        let docs = BTreeMap::from([
            (
                "alice.char.hson".into(),
                ".{ name: \"alice\", parts: .{ body: .{ path: \"alice.png\" } } }".into(),
            ),
            (
                "ui/name.ui.hks".into(),
                "if speaker == \"alice\" { text(\"Alice\") }".into(),
            ),
        ]);
        assert!(analyze(&docs).expect("name UI").resident.is_empty());
    }

    #[test]
    fn exported_function_resolves_helpers_in_its_own_module() {
        let docs = BTreeMap::from([
            (
                "atlas.texture.hson".into(),
                ".{ name: \"room\", image: \"room.png\" }".into(),
            ),
            (
                "shared.hks".into(),
                "fn helper() { bg(\"room\") }; global fn enter() { helper() }".into(),
            ),
            (
                "scene.hks".into(),
                "fn helper() { \"Not an image\" }; enter()".into(),
            ),
        ]);
        assert!(
            analyze(&docs).expect("module-local helper").scripts["scene.hks"].contains("room.png")
        );
    }

    #[test]
    fn follows_actor_and_function_not_goto_and_deduplicates_atlas() {
        let docs = BTreeMap::from([
            ("textures/atlas.texture.hson".into(), ".{ image: \"atlas.png\", regions: .{ \"alice/body\": (0,0,1,1), \"ui/button\": (1,0,1,1) } }".into()),
            ("characters/alice.char.hson".into(), ".{ name: \"alice\", parts: .{ body: .{ texture: \"alice/body\" } } }".into()),
            ("common.hks".into(), "global let alice = char(\"alice\"); global fn entrance() { alice.show() }".into()),
            ("first.hks".into(), "entrance(); story.goto(\"second.hks\")".into()),
            ("second.hks".into(), "\"Hello\"".into()),
            ("ui/title.ui.hks".into(), "image(\"ui/button\")".into()),
        ]);
        let manifest = analyze(&docs).expect("analyze synthetic assets");
        assert_eq!(
            manifest.scripts["first.hks"],
            BTreeSet::from(["textures/atlas.png".into()])
        );
        assert!(manifest.scripts["second.hks"].is_empty());
        assert_eq!(manifest.resident, manifest.scripts["first.hks"]);
        assert_eq!(manifest, analyze(&docs).expect("deterministic"));
    }
    #[test]
    fn computed_resource_is_explicitly_conservative() {
        let docs = BTreeMap::from([
            (
                "atlas.texture.hson".into(),
                ".{ name: \"background\", image: \"atlas.png\" }".into(),
            ),
            ("scene.hks".into(), "bg(getBackground())".into()),
        ]);
        let manifest = analyze(&docs).expect("analyze computed reference");
        assert!(manifest.scripts["scene.hks"].is_empty());
        assert!(manifest.conservative["scene.hks"].contains("texture:return:getBackground"));
    }
}
