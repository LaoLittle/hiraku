//! Lossless editor syntax, sharing the compiler lexer and parser.
//!
//! Tokens retain trivia and original spelling. Delimiter nodes survive unfinished
//! edits; valid files additionally expose top-level statement nodes. This is the
//! initial structural CST, not a second expression grammar or a replacement HIR.
use crate::{
    Program, Stmt,
    lex::{self, TokenKind},
    parse::{ParseError, parse_program},
    span::Span,
};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyntaxKind {
    Root,
    Statement,
    Parentheses,
    Brackets,
    Braces,
}
#[derive(Clone, Debug)]
pub struct SyntaxToken {
    pub kind: TokenKind,
    pub span: Span,
}
#[derive(Clone, Debug)]
pub enum SyntaxElement {
    Token(usize),
    Node(usize),
}
#[derive(Clone, Debug)]
pub struct SyntaxNode {
    pub kind: SyntaxKind,
    pub span: Span,
    pub children: Vec<SyntaxElement>,
    pub closed: bool,
}
#[derive(Clone, Debug)]
pub struct SyntaxTree {
    pub source: Arc<str>,
    pub tokens: Vec<SyntaxToken>,
    pub nodes: Vec<SyntaxNode>,
    pub errors: Vec<ParseError>,
    pub ast: Option<Program>,
}
impl SyntaxTree {
    pub fn parse(source: impl Into<Arc<str>>) -> Self {
        let source = source.into();
        let mut tree = Self {
            source: source.clone(),
            tokens: Vec::new(),
            nodes: vec![SyntaxNode {
                kind: SyntaxKind::Root,
                span: Span {
                    start: 0,
                    end: source.len(),
                },
                children: Vec::new(),
                closed: true,
            }],
            errors: Vec::new(),
            ast: None,
        };
        let mut stack = vec![0];
        let mut offset = 0;
        for raw in lex::tokenize(&source) {
            let span = Span::new(offset, raw.len);
            offset = span.end;
            let token = tree.tokens.len();
            tree.tokens.push(SyntaxToken {
                kind: raw.kind,
                span,
            });
            let opening = match raw.kind {
                TokenKind::OpenParen => Some(SyntaxKind::Parentheses),
                TokenKind::OpenBracket => Some(SyntaxKind::Brackets),
                TokenKind::OpenBrace => Some(SyntaxKind::Braces),
                _ => None,
            };
            if let Some(kind) = opening {
                let id = tree.nodes.len();
                tree.nodes.push(SyntaxNode {
                    kind,
                    span,
                    children: Vec::new(),
                    closed: false,
                });
                tree.nodes[*stack.last().expect("root remains")]
                    .children
                    .push(SyntaxElement::Node(id));
                stack.push(id);
            }
            let current = *stack.last().expect("root remains");
            tree.nodes[current]
                .children
                .push(SyntaxElement::Token(token));
            let closes = matches!(
                (tree.nodes[current].kind, raw.kind),
                (SyntaxKind::Parentheses, TokenKind::CloseParen)
                    | (SyntaxKind::Brackets, TokenKind::CloseBracket)
                    | (SyntaxKind::Braces, TokenKind::CloseBrace)
            );
            if closes {
                tree.nodes[current].span.end = span.end;
                tree.nodes[current].closed = true;
                stack.pop();
            }
        }
        for id in stack {
            tree.nodes[id].span.end = source.len();
        }
        match parse_program(&source) {
            Ok(ast) => {
                let spans = ast
                    .statements
                    .iter()
                    .map(statement_span)
                    .collect::<Vec<_>>();
                let children = std::mem::take(&mut tree.nodes[0].children);
                let mut children = children.into_iter().peekable();
                for span in spans {
                    while children
                        .peek()
                        .is_some_and(|child| tree.element_span(child).end <= span.start)
                    {
                        tree.nodes[0]
                            .children
                            .push(children.next().expect("peeked child"));
                    }
                    let mut statement = Vec::new();
                    while children.peek().is_some_and(|child| {
                        let s = tree.element_span(child);
                        s.start >= span.start && s.end <= span.end
                    }) {
                        statement.push(children.next().expect("peeked child"));
                    }
                    if !statement.is_empty() {
                        let id = tree.nodes.len();
                        tree.nodes.push(SyntaxNode {
                            kind: SyntaxKind::Statement,
                            span,
                            children: statement,
                            closed: true,
                        });
                        tree.nodes[0].children.push(SyntaxElement::Node(id));
                    }
                }
                tree.nodes[0].children.extend(children);
                tree.ast = Some(ast);
            }
            Err(errors) => tree.errors = errors,
        }
        tree
    }
    pub fn element_span(&self, element: &SyntaxElement) -> Span {
        match element {
            SyntaxElement::Token(id) => self.tokens[*id].span,
            SyntaxElement::Node(id) => self.nodes[*id].span,
        }
    }
    pub fn token_text(&self, token: &SyntaxToken) -> &str {
        &self.source[token.span.range()]
    }
    pub fn reconstructed(&self) -> String {
        self.tokens
            .iter()
            .map(|token| self.token_text(token))
            .collect()
    }
}

pub fn statement_span(statement: &Stmt) -> Span {
    match statement {
        Stmt::Expr(expr) => expr.span,
        Stmt::Return { span, .. }
        | Stmt::Const { span, .. }
        | Stmt::Property { span, .. }
        | Stmt::Extend { span, .. }
        | Stmt::Protocol { span, .. }
        | Stmt::Import { span, .. }
        | Stmt::Enum { span, .. }
        | Stmt::Struct { span, .. }
        | Stmt::TypeAlias { span, .. }
        | Stmt::Function { span, .. }
        | Stmt::Let { span, .. }
        | Stmt::Global { span, .. }
        | Stmt::Assign { span, .. }
        | Stmt::If { span, .. }
        | Stmt::While { span, .. } => *span,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lossless_with_trivia_templates_unicode_and_incomplete_input() {
        for source in [
            "// @block(entry)\r\nlet alice = \"Hi ${name ?: \"Bob\"}\"  \r\n",
            "/* comment */\nfn incomplete(\n",
            "\"こんにちは😀\"\n",
            "if true {\n  ]",
            "",
        ] {
            let tree = SyntaxTree::parse(source);
            assert_eq!(tree.reconstructed(), source);
            fn visit(tree: &SyntaxTree, id: usize, output: &mut String) {
                for element in &tree.nodes[id].children {
                    match element {
                        SyntaxElement::Node(id) => visit(tree, *id, output),
                        SyntaxElement::Token(id) => {
                            output.push_str(tree.token_text(&tree.tokens[*id]))
                        }
                    }
                }
            }
            let mut output = String::new();
            visit(&tree, 0, &mut output);
            assert_eq!(
                output, source,
                "tree traversal retains every byte exactly once"
            );
        }
        let tree = SyntaxTree::parse("if true {\n");
        assert!(!tree.errors.is_empty());
        assert!(
            tree.nodes
                .iter()
                .any(|node| node.kind == SyntaxKind::Braces && !node.closed)
        );
    }
}
