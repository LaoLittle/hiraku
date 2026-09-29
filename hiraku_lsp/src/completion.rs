//! Transport-independent completion for unsaved, potentially incomplete source.
//!
//! This deliberately uses the compiler's lossless tokens rather than a second
//! lexer. The recovery index understands declaration scopes, but does not claim
//! to infer receiver types or to replace the compiler's type checker. Embedders
//! supply their own public API catalogue; there is no engine dependency here.
use hiraku_script::{
    Stmt,
    cst::{SyntaxKind, SyntaxToken, SyntaxTree},
    lex::{LiteralKind, TokenKind},
    span::Span,
};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionKind {
    Function,
    Variable,
    Type,
    Module,
    Keyword,
    Field,
}

/// A public project or host symbol. Qualified names such as `ui.open` produce
/// member completions after `ui.` and unqualified ones after `import ui.*`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionSymbol {
    pub name: String,
    pub detail: String,
    pub kind: CompletionKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    /// Plain text, not LSP snippet syntax. Existing call arguments are preserved.
    pub insert_text: String,
    pub detail: String,
    pub kind: CompletionKind,
    /// UTF-8 byte range in the exact source passed to [`complete`].
    pub replacement: Span,
}

/// Public declarations from a project module or script standard library. Only
/// syntactically valid modules contribute exports; local scopes never escape.
/// This performs no IO, compilation, linking, or execution of script code.
pub fn exported_symbols(source: &str) -> Vec<CompletionSymbol> {
    let tree = SyntaxTree::parse(source);
    let Some(ast) = &tree.ast else {
        return Vec::new();
    };
    ast.statements
        .iter()
        .filter_map(|statement| {
            let (name, kind, span, end) = match statement {
                Stmt::Function {
                    exported: true,
                    name,
                    span,
                    body,
                    ..
                } => (name, CompletionKind::Function, span, body.span.start),
                Stmt::Global {
                    name, span, value, ..
                } => (
                    name,
                    CompletionKind::Variable,
                    span,
                    value.as_ref().map_or(span.end, |value| value.span.start),
                ),
                Stmt::Struct {
                    exported: true,
                    name,
                    span,
                    ty,
                    ..
                }
                | Stmt::TypeAlias {
                    exported: true,
                    name,
                    span,
                    ty,
                    ..
                } => (name, CompletionKind::Type, span, ty.span.end),
                Stmt::Enum {
                    exported: true,
                    name,
                    span,
                    ..
                } => (name, CompletionKind::Type, span, span.end),
                _ => return None,
            };
            let detail = source[span.start..end]
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            Some(CompletionSymbol {
                name: name.clone(),
                kind,
                detail,
            })
        })
        .collect()
}

/// Complete at a UTF-8 byte cursor. Invalid offsets and string/comment interiors
/// return no candidates. This is safe to call for unfinished documents.
pub fn complete(
    source: &str,
    byte_offset: usize,
    symbols: &[CompletionSymbol],
) -> Vec<CompletionItem> {
    complete_tree(&SyntaxTree::parse(source), byte_offset, symbols)
}

/// As [`complete`], reusing a document already parsed by the editor or server.
pub fn complete_tree(
    tree: &SyntaxTree,
    byte_offset: usize,
    symbols: &[CompletionSymbol],
) -> Vec<CompletionItem> {
    let source = tree.source.as_ref();
    if byte_offset > source.len() || !source.is_char_boundary(byte_offset) {
        return Vec::new();
    }
    let Some(replacement) = replacement(tree, byte_offset) else {
        return Vec::new();
    };
    let prefix = &source[replacement.start..byte_offset];
    let tokens: Vec<_> = tree
        .tokens
        .iter()
        .filter(|token| !trivia(token.kind))
        .collect();
    let before = tokens.partition_point(|token| token.span.end <= replacement.start);
    let qualifier = qualifier(tree, &tokens, before);
    // A leading-dot expression needs a contextual receiver type. Do not offer
    // arbitrary root functions as enum variants or static methods.
    if qualifier.as_deref() == Some("") {
        return Vec::new();
    }
    // A declaration's name is being written, not a reference to an existing one.
    if qualifier.is_none()
        && before > 0
        && declaration_kind(tree.token_text(tokens[before - 1])).is_some()
    {
        return Vec::new();
    }
    let imports = imports(tree, &tokens, byte_offset);
    let mut candidates = BTreeMap::new();
    for symbol in symbols {
        if let Some(qualifier) = &qualifier {
            if let Some(name) = symbol.name.strip_prefix(&format!("{qualifier}.")) {
                add_path(&mut candidates, name, symbol, prefix, replacement);
            }
        } else {
            add_path(&mut candidates, &symbol.name, symbol, prefix, replacement);
            for import in &imports {
                if let Some(name) = import.expose(&symbol.name) {
                    add_path(&mut candidates, name, symbol, prefix, replacement);
                }
            }
        }
    }
    if qualifier.is_none() {
        for keyword in KEYWORDS {
            add(
                &mut candidates,
                keyword,
                CompletionKind::Keyword,
                "keyword",
                prefix,
                replacement,
            );
        }
        for ty in TYPES {
            add(
                &mut candidates,
                ty,
                CompletionKind::Type,
                "standard type",
                prefix,
                replacement,
            );
        }
        for declaration in declarations(tree, &tokens, byte_offset) {
            add(
                &mut candidates,
                &declaration.name,
                declaration.kind,
                &declaration.detail,
                prefix,
                replacement,
            );
        }
    }
    candidates.into_values().collect()
}

const KEYWORDS: &[&str] = &[
    "as", "else", "enum", "extend", "false", "fn", "global", "if", "import", "let", "null",
    "protocol", "return", "self", "struct", "true", "type", "var", "when", "while",
];
const TYPES: &[&str] = &[
    "Any",
    "Bool",
    "Float",
    "Int",
    "List",
    "Map",
    "Never",
    "Optional",
    "Result",
    "Self",
    "String",
    "TextTemplate",
    "UInt",
    "Unit",
];

fn trivia(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Whitespace
            | TokenKind::NewLine
            | TokenKind::LineComment { .. }
            | TokenKind::BlockComment { .. }
    )
}

fn replacement(tree: &SyntaxTree, cursor: usize) -> Option<Span> {
    // Prefer an identifier touching the cursor on the left. Token starts and
    // ends are valid edit boundaries, including immediately before `(`.
    let mut result = Span {
        start: cursor,
        end: cursor,
    };
    for token in &tree.tokens {
        if token.span.start > cursor {
            break;
        }
        let inside = token.span.start < cursor && cursor < token.span.end;
        let at_end = token.span.end == cursor;
        let blocked = match token.kind {
            TokenKind::LineComment { .. } => inside || at_end,
            TokenKind::BlockComment { terminated, .. } => inside || at_end && !terminated,
            TokenKind::Literal { kind, .. } => {
                inside
                    || at_end
                        && matches!(
                            kind,
                            LiteralKind::Str { terminated: false }
                                | LiteralKind::Char { terminated: false }
                                | LiteralKind::Byte { terminated: false }
                        )
            }
            _ => false,
        };
        if blocked {
            return None;
        }
        if token.kind == TokenKind::Ident && token.span.start <= cursor && cursor <= token.span.end
        {
            result = token.span;
        }
    }
    Some(result)
}

fn qualifier(tree: &SyntaxTree, tokens: &[&SyntaxToken], before: usize) -> Option<String> {
    let mut i = before;
    if i == 0 || tokens[i - 1].kind != TokenKind::Dot {
        return None;
    }
    let mut segments = Vec::new();
    loop {
        i -= 1; // dot
        if i == 0 || tokens[i - 1].kind != TokenKind::Ident {
            return Some(String::new());
        }
        i -= 1;
        segments.push(tree.token_text(tokens[i]));
        if i == 0 || tokens[i - 1].kind != TokenKind::Dot {
            break;
        }
    }
    segments.reverse();
    Some(segments.join("."))
}

fn add_path(
    candidates: &mut BTreeMap<String, CompletionItem>,
    name: &str,
    symbol: &CompletionSymbol,
    prefix: &str,
    replacement: Span,
) {
    if let Some((module, _)) = name.split_once('.') {
        // Do not replace a bare prefix with an entire qualified call. Complete
        // one segment at a time, matching how member selectors are parsed.
        add(
            candidates,
            module,
            CompletionKind::Module,
            "module",
            prefix,
            replacement,
        );
    } else {
        add(
            candidates,
            name,
            symbol.kind,
            &symbol.detail,
            prefix,
            replacement,
        );
    }
}

fn add(
    candidates: &mut BTreeMap<String, CompletionItem>,
    name: &str,
    kind: CompletionKind,
    detail: &str,
    prefix: &str,
    replacement: Span,
) {
    if name.starts_with(prefix) && !name.is_empty() {
        if kind == CompletionKind::Module
            && candidates
                .get(name)
                .is_some_and(|item| item.kind != CompletionKind::Module)
        {
            // A path prefix may be a type with associated functions. Preserve
            // the actual type/function catalogue entry instead of replacing its
            // documentation with a generated "module" placeholder.
            return;
        }
        candidates.insert(
            name.into(),
            CompletionItem {
                label: name.into(),
                insert_text: name.into(),
                detail: detail.into(),
                kind,
                replacement,
            },
        );
    }
}

struct Import {
    path: String,
    wildcard: bool,
}
impl Import {
    fn expose<'a>(&self, symbol: &'a str) -> Option<&'a str> {
        if self.wildcard {
            symbol.strip_prefix(&format!("{}.", self.path))
        } else if self.path == symbol {
            symbol.rsplit('.').next()
        } else {
            None
        }
    }
}
fn imports(tree: &SyntaxTree, tokens: &[&SyntaxToken], cursor: usize) -> Vec<Import> {
    let mut result = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        if tree.token_text(token) != "import" {
            continue;
        }
        if tree.nodes.iter().any(|node| {
            node.kind == SyntaxKind::Braces
                && node.span.start < token.span.start
                && token.span.end <= node.span.end
                && !(node.span.start < cursor && cursor <= node.span.end)
        }) {
            continue;
        }
        let mut path = String::new();
        let mut wildcard = false;
        for token in &tokens[i + 1..] {
            if tree.source[tokens[i].span.end..token.span.start].contains('\n') {
                break;
            }
            match token.kind {
                TokenKind::Ident => path.push_str(tree.token_text(token)),
                TokenKind::Dot => path.push('.'),
                TokenKind::Star => {
                    wildcard = true;
                    break;
                }
                _ => break,
            }
        }
        result.push(Import {
            path: path.trim_end_matches('.').into(),
            wildcard,
        });
    }
    result
}

fn declaration_kind(keyword: &str) -> Option<CompletionKind> {
    match keyword {
        "let" | "var" => Some(CompletionKind::Variable),
        "fn" => Some(CompletionKind::Function),
        "enum" | "struct" | "type" | "protocol" => Some(CompletionKind::Type),
        _ => None,
    }
}

struct Scope {
    start: usize,
    end: usize,
    depth: usize,
}
impl Scope {
    fn contains(&self, cursor: usize) -> bool {
        self.start <= cursor && cursor <= self.end
    }
}

/// Recovery deliberately indexes only declarations, not arbitrary identifiers:
/// strings, other functions' locals, record keys, and uses are never candidates.
fn declarations(
    tree: &SyntaxTree,
    tokens: &[&SyntaxToken],
    cursor: usize,
) -> Vec<CompletionSymbol> {
    let mut scopes = vec![Scope {
        start: 0,
        end: tree.source.len(),
        depth: 0,
    }];
    let mut current = vec![0];
    let mut token_scope = Vec::with_capacity(tokens.len());
    let mut brace_scope = BTreeMap::new();
    let mut pairs = BTreeMap::new();
    let mut delimiters = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        token_scope.push(*current.last().expect("root scope remains"));
        if token.kind == TokenKind::OpenBrace {
            let id = scopes.len();
            scopes.push(Scope {
                start: token.span.end,
                end: tree.source.len(),
                depth: current.len(),
            });
            current.push(id);
            brace_scope.insert(i, id);
        } else if token.kind == TokenKind::CloseBrace && current.len() > 1 {
            let id = current.pop().expect("nested scope exists");
            scopes[id].end = token.span.start;
        }
        match token.kind {
            TokenKind::OpenParen | TokenKind::OpenBracket | TokenKind::OpenBrace => {
                delimiters.push((token.kind, i));
            }
            TokenKind::CloseParen | TokenKind::CloseBracket | TokenKind::CloseBrace => {
                if let Some((opening, index)) = delimiters.last().copied()
                    && matches!(
                        (opening, token.kind),
                        (TokenKind::OpenParen, TokenKind::CloseParen)
                            | (TokenKind::OpenBracket, TokenKind::CloseBracket)
                            | (TokenKind::OpenBrace, TokenKind::CloseBrace)
                    )
                {
                    delimiters.pop();
                    pairs.insert(index, i);
                }
            }
            _ => {}
        }
    }
    let mut result = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        let scope = &scopes[token_scope[i]];
        if !scope.contains(cursor) {
            continue;
        }
        let Some(kind) = declaration_kind(tree.token_text(token)) else {
            continue;
        };
        let Some(name) = tokens
            .get(i + 1)
            .filter(|name| name.kind == TokenKind::Ident)
        else {
            continue;
        };
        let text = tree.token_text(name);
        if KEYWORDS.contains(&text) {
            continue;
        }
        if kind == CompletionKind::Variable
            && (name.span.end >= cursor
                || !binding_initialized(tree, tokens, i + 2, cursor, &pairs))
        {
            continue;
        }
        let end = tokens[i + 2..]
            .iter()
            .find(|next| {
                matches!(
                    next.kind,
                    TokenKind::Eq | TokenKind::Semi | TokenKind::OpenBrace
                ) || tree.source[name.span.end..next.span.start].contains('\n')
            })
            .map_or(name.span.end, |next| next.span.start);
        let detail = tree.source[token.span.start..end]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        result.push((
            scope.depth,
            name.span.start,
            CompletionSymbol {
                name: text.into(),
                kind,
                detail,
            },
        ));

        if kind != CompletionKind::Function {
            continue;
        }
        let Some(open) = (i + 2..tokens.len()).find(|&index| {
            matches!(
                tokens[index].kind,
                TokenKind::OpenParen | TokenKind::OpenBrace | TokenKind::Semi
            )
        }) else {
            continue;
        };
        if tokens[open].kind != TokenKind::OpenParen {
            continue;
        }
        let Some(&close) = pairs.get(&open) else {
            continue;
        };
        let Some(body) = (close + 1..tokens.len()).find(|&index| {
            matches!(tokens[index].kind, TokenKind::OpenBrace | TokenKind::Semi)
                || declaration_kind(tree.token_text(tokens[index])).is_some()
        }) else {
            continue;
        };
        let Some(&body_scope) = brace_scope.get(&body) else {
            continue;
        };
        if scopes[body_scope].contains(cursor) {
            for parameter in parameters(tree, tokens, open + 1, close, &pairs) {
                result.push((scopes[body_scope].depth, tokens[body].span.start, parameter));
            }
        }
    }
    // Kotlin-style lambda parameters belong to their brace scope, not its parent.
    for (&open, &id) in &brace_scope {
        if !scopes[id].contains(cursor) {
            continue;
        }
        let end = pairs.get(&open).copied().unwrap_or(tokens.len());
        let mut arrow = open + 1;
        while arrow + 1 < end {
            if tokens[arrow].kind == TokenKind::Minus && tokens[arrow + 1].kind == TokenKind::Gt {
                for parameter in parameters(tree, tokens, open + 1, arrow, &pairs) {
                    result.push((scopes[id].depth, tokens[open].span.start, parameter));
                }
                break;
            }
            if matches!(
                tokens[arrow].kind,
                TokenKind::Eq | TokenKind::Semi | TokenKind::OpenBrace
            ) || declaration_kind(tree.token_text(tokens[arrow])).is_some()
            {
                break;
            }
            if let Some(&close) = pairs.get(&arrow) {
                arrow = close;
            }
            arrow += 1;
        }
    }
    // Insertion order makes the nearest scope win over outer declarations.
    result.sort_by_key(|(depth, position, _)| (*depth, *position));
    result.into_iter().map(|(_, _, symbol)| symbol).collect()
}

fn binding_initialized(
    tree: &SyntaxTree,
    tokens: &[&SyntaxToken],
    start: usize,
    cursor: usize,
    pairs: &BTreeMap<usize, usize>,
) -> bool {
    let mut previous_end = tokens[start - 1].span.end;
    let mut i = start;
    while i < tokens.len() && tokens[i].span.start < cursor {
        let token = tokens[i];
        if tree.source[previous_end..token.span.start].contains('\n')
            || matches!(token.kind, TokenKind::Semi | TokenKind::CloseBrace)
        {
            return true;
        }
        if token.span.end >= cursor {
            return false;
        }
        if let Some(&close) = pairs.get(&i) {
            // Captured names also cannot refer to the binding currently being
            // initialized. Skip complete tuple/list/lambda initializers.
            if tokens[close].span.end >= cursor {
                return false;
            }
            i = close;
        } else if matches!(
            token.kind,
            TokenKind::OpenBrace | TokenKind::OpenParen | TokenKind::OpenBracket
        ) {
            return false;
        }
        previous_end = tokens[i].span.end;
        i += 1;
    }
    tree.source[previous_end..cursor].contains('\n')
}

fn parameters(
    tree: &SyntaxTree,
    tokens: &[&SyntaxToken],
    start: usize,
    end: usize,
    pairs: &BTreeMap<usize, usize>,
) -> Vec<CompletionSymbol> {
    let mut result = Vec::new();
    let mut segment = start;
    let mut i = start;
    let mut angles = 0usize;
    while i <= end {
        let at_end = i == end;
        if at_end || tokens[i].kind == TokenKind::Comma && angles == 0 {
            if segment < i && tokens[segment].kind == TokenKind::Ident {
                let name = tree.token_text(tokens[segment]);
                let has_type = segment + 1 < i && tokens[segment + 1].kind == TokenKind::Colon;
                if !KEYWORDS.contains(&name) && (has_type || segment + 1 == i) {
                    result.push(CompletionSymbol {
                        name: name.into(),
                        kind: CompletionKind::Variable,
                        detail: format!(
                            "parameter {}",
                            tree.source[tokens[segment].span.start..tokens[i - 1].span.end].trim()
                        ),
                    });
                }
            }
            segment = i + 1;
        } else {
            match tokens[i].kind {
                TokenKind::Lt => angles += 1,
                TokenKind::Gt => angles = angles.saturating_sub(1),
                _ => {}
            }
            if let Some(&close) = pairs.get(&i) {
                i = close;
            }
        }
        i += 1;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at_marker(source: &str) -> Vec<CompletionItem> {
        let cursor = source.find('|').expect("cursor marker");
        complete(&source.replacen('|', "", 1), cursor, &[])
    }
    fn names(items: &[CompletionItem]) -> Vec<&str> {
        items.iter().map(|item| item.label.as_str()).collect()
    }
    fn api() -> Vec<CompletionSymbol> {
        ["ui.open", "ui.widgets.button", "ui.widgets.text"]
            .map(|name| CompletionSymbol {
                name: name.into(),
                kind: CompletionKind::Function,
                detail: format!("fn {name}() -> Unit"),
            })
            .into()
    }

    #[test]
    fn unfinished_source_preserves_scope_and_forward_function_declarations() {
        let items = at_marker(
            "let alice = 1\nfn greet() {}\nfn elsewhere() { let bob = 2 }\nif true {\n let carol = 3\n |\n",
        );
        let labels = names(&items);
        assert!(labels.contains(&"alice"));
        assert!(labels.contains(&"carol"));
        assert!(labels.contains(&"greet"));
        assert!(!labels.contains(&"bob"));
        assert!(names(&at_marker("gre|\nfn greet() {} ")).contains(&"greet"));
        assert!(!names(&at_marker("al|\nlet alice = 1")).contains(&"alice"));
    }

    #[test]
    fn parameters_are_visible_only_in_their_body() {
        let items = at_marker("fn greet(alice: String, callback: (Int, String) -> Unit) { al|\n");
        assert_eq!(names(&items), ["alice"]);
        assert_eq!(items[0].detail, "parameter alice: String");
        assert!(!names(&at_marker("fn greet(alice: String) {}\nal|")).contains(&"alice"));
        assert_eq!(
            names(&at_marker(
                "let callback = { alice: String, bob: Int ->\n al|"
            )),
            ["alice"]
        );
        assert!(
            !names(&at_marker("let callback = { alice: String -> alice }\nal|")).contains(&"alice")
        );
    }

    #[test]
    fn nearest_scope_wins_and_closed_blocks_do_not_leak() {
        let items = at_marker("let alice: String = \"name\"\nif true { let alice: Int = 1\n al| }");
        assert_eq!(items[0].detail, "let alice: Int");
        let items = at_marker("let alice: String = \"name\"\nif true { let alice: Int = 1 }\nal|");
        assert_eq!(items[0].detail, "let alice: String");
        assert!(!names(&at_marker("if true { let bob = 1 }\nbo|")).contains(&"bob"));
    }

    #[test]
    fn binding_is_not_visible_in_its_own_initializer() {
        assert!(!names(&at_marker("let alice = al|")).contains(&"alice"));
        assert!(!names(&at_marker("let alice = { al| }")).contains(&"alice"));
        assert!(names(&at_marker("let alice = 1; al|")).contains(&"alice"));
        assert!(names(&at_marker("let alice = [1, 2]\nal|")).contains(&"alice"));
    }

    #[test]
    fn unicode_replacements_are_valid_and_replace_the_whole_identifier() {
        let source = "let 名前 = 1\n名|前";
        let items = at_marker(source);
        assert_eq!(names(&items), ["名前"]);
        let text = source.replace('|', "");
        assert_eq!(&text[items[0].replacement.range()], "名前");
        assert!(complete(&text, text.find('名').expect("unicode") + 1, &[]).is_empty());
        assert!(complete(&text, text.len() + 1, &[]).is_empty());
    }

    #[test]
    fn no_completion_in_strings_comments_or_declaration_names() {
        for source in [
            "\"al|ice\"",
            "\"alice|",
            "// alice|",
            "/* al|ice */",
            "/* alice|",
            "let al|",
            "global var al|",
            "fn al|",
            "enum Al|",
        ] {
            assert!(at_marker(source).is_empty(), "{source}");
        }
        assert!(names(&at_marker("/* closed */\nwh|")).contains(&"while"));
    }

    #[test]
    fn qualified_host_apis_and_imports_share_plain_text_edits() {
        let source = "ui.op";
        let items = complete(source, source.len(), &api());
        assert_eq!(names(&items), ["open"]);
        assert_eq!(items[0].insert_text, "open");
        assert_eq!(items[0].replacement, Span { start: 3, end: 5 });
        let source = "import ui.widgets.*\nbut";
        assert_eq!(names(&complete(source, source.len(), &api())), ["button"]);
        let source = "import ui.open\nop";
        assert_eq!(names(&complete(source, source.len(), &api())), ["open"]);
        assert_eq!(names(&complete("ui.", 3, &api())), ["open", "widgets"]);
        assert_eq!(
            names(&complete("ui.widgets.", 11, &api())),
            ["button", "text"]
        );
        assert!(complete(".op", 3, &api()).is_empty());
        let source = "fn local() { import ui.widgets.*\n}\nbut";
        assert!(complete(source, source.len(), &api()).is_empty());
    }

    #[test]
    fn repeated_calls_are_deterministic_and_do_not_mutate_source() {
        let source = "global let alice = 1\nfn bob(value: Int) {\n";
        assert_eq!(
            complete(source, source.len(), &api()),
            complete(source, source.len(), &api())
        );
    }

    #[test]
    fn every_utf8_cursor_boundary_survives_unfinished_edits() {
        for source in [
            "global let alice = { 名前: String -> 名前 }\nui.op",
            "fn greet(callback: (Int, String) -> Unit, alice: String) { let bob = [1, 2\n",
            "let alice = .{ name: \"Bob\" }\nif true { alice.na",
            "/* nested /* comment */ unfinished\n",
            "let alice = \"😀 ${unfinished(\"Bob\"",
            "import ui.widgets.*\nfn broken( ] }\n",
        ] {
            for cursor in (0..=source.len()).filter(|cursor| source.is_char_boundary(*cursor)) {
                for item in complete(source, cursor, &api()) {
                    assert!(source.is_char_boundary(item.replacement.start));
                    assert!(source.is_char_boundary(item.replacement.end));
                    assert!(item.replacement.start <= cursor && cursor <= item.replacement.end);
                }
            }
        }
    }

    #[test]
    fn exported_project_symbols_do_not_leak_private_or_nested_bindings() {
        let symbols = exported_symbols(
            "global fn greet(alice: String) { let bob = alice }\nglobal let score: Int = 1\nfn private() {}\nlet local = 2\nglobal struct Player { name: String }",
        );
        let labels: Vec<_> = symbols.iter().map(|symbol| symbol.name.as_str()).collect();
        assert_eq!(labels, ["greet", "score", "Player"]);
        assert!(symbols[0].detail.contains("greet(alice: String)"));
        assert!(!symbols[0].detail.contains("bob"));
        assert!(exported_symbols("global fn unfinished(").is_empty());
    }
}
