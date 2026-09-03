//! The canonical Hiraku Script parser, built on the crate's shared lexer.

use crate::{ast::*, span::Span};
use hiraku_errors::{Diagnostic, DiagnosticLabel, SourceId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    pub span: Span,
}

impl ParseError {
    pub fn diagnostic(&self, source: SourceId) -> Diagnostic {
        Diagnostic::error(&self.message)
            .with_code("HKS-PARSE")
            .with_label(DiagnosticLabel::primary(source, self.span.range()))
    }
}

impl SyntaxWarning {
    pub fn diagnostic(&self, source: SourceId) -> Diagnostic {
        Diagnostic::warning(&self.message)
            .with_code("HKS-SYNTAX")
            .with_label(DiagnosticLabel::primary(source, self.span.range()))
    }
}

pub fn parse_program(source: &str) -> Result<Program, Vec<ParseError>> {
    parse_program_with_template_expressions(source, true)
}

pub(crate) fn parse_literal_program(source: &str) -> Result<Program, Vec<ParseError>> {
    parse_program_with_template_expressions(source, false)
}

fn parse_program_with_template_expressions(
    source: &str,
    template_expressions: bool,
) -> Result<Program, Vec<ParseError>> {
    let tokens = TokenAdapter::new(source, template_expressions).collect()?;
    Parser::new(tokens).parse_program()
}

#[derive(Clone, Debug, PartialEq)]
struct Token {
    kind: TokenKind,
    span: Span,
}

#[derive(Clone, Debug, PartialEq)]
enum TokenKind {
    Ident(String),
    Number(f64, NumberUnit),
    String(String),
    Ellipsis,
    Newline,
    Semi,
    Dot,
    Comma,
    Colon,
    Equal,
    EqualEqual,
    PlusEqual,
    MinusEqual,
    StarEqual,
    SlashEqual,
    BangEqual,
    LessEqual,
    GreaterEqual,
    Question,
    Bang,
    Lt,
    Gt,
    Plus,
    Minus,
    Star,
    Slash,
    Dollar,
    At,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Eof,
}

/// Adds source values and spans to the shared lexer's structural tokens.
struct TokenAdapter<'a> {
    source: &'a str,
    offset: usize,
    template_expressions: bool,
}

impl<'a> TokenAdapter<'a> {
    fn new(source: &'a str, template_expressions: bool) -> Self {
        Self {
            source,
            offset: 0,
            template_expressions,
        }
    }

    fn collect(mut self) -> Result<Vec<Token>, Vec<ParseError>> {
        let mut tokens: Vec<Token> = Vec::new();
        let mut errors = Vec::new();

        for raw in
            crate::lex::tokenize_with_template_expressions(self.source, self.template_expressions)
        {
            let start = self.offset;
            self.offset += raw.len as usize;
            let span = Span {
                start,
                end: self.offset,
            };
            let lexeme = &self.source[start..self.offset];
            use crate::lex::{LiteralKind, TokenKind as RawToken};

            let kind = match raw.kind {
                RawToken::Whitespace
                | RawToken::LineComment { .. }
                | RawToken::BlockComment {
                    terminated: true, ..
                } => continue,
                RawToken::BlockComment {
                    terminated: false, ..
                } => {
                    errors.push(ParseError {
                        message: "unterminated block comment".to_string(),
                        span,
                    });
                    continue;
                }
                RawToken::NewLine => TokenKind::Newline,
                RawToken::Semi => TokenKind::Semi,
                RawToken::Dot => {
                    if tokens.len() >= 2
                        && matches!(tokens[tokens.len() - 1].kind, TokenKind::Dot)
                        && matches!(tokens[tokens.len() - 2].kind, TokenKind::Dot)
                        && tokens[tokens.len() - 2].span.end == tokens[tokens.len() - 1].span.start
                        && tokens[tokens.len() - 1].span.end == start
                    {
                        let first = tokens.remove(tokens.len() - 2);
                        tokens.pop();
                        tokens.push(Token {
                            kind: TokenKind::Ellipsis,
                            span: Span {
                                start: first.span.start,
                                end: span.end,
                            },
                        });
                        continue;
                    }
                    TokenKind::Dot
                }
                RawToken::Comma => TokenKind::Comma,
                RawToken::Colon => TokenKind::Colon,
                RawToken::Minus => TokenKind::Minus,
                RawToken::Plus => TokenKind::Plus,
                RawToken::Star => TokenKind::Star,
                RawToken::Slash => TokenKind::Slash,
                RawToken::Dollar => TokenKind::Dollar,
                RawToken::At => TokenKind::At,
                RawToken::Question => TokenKind::Question,
                RawToken::Bang => TokenKind::Bang,
                RawToken::Lt => TokenKind::Lt,
                RawToken::Gt => TokenKind::Gt,
                RawToken::OpenParen => TokenKind::LParen,
                RawToken::CloseParen => TokenKind::RParen,
                RawToken::OpenBrace => TokenKind::LBrace,
                RawToken::CloseBrace => TokenKind::RBrace,
                RawToken::OpenBracket => TokenKind::LBracket,
                RawToken::CloseBracket => TokenKind::RBracket,
                RawToken::Ident => TokenKind::Ident(lexeme.to_string()),
                RawToken::Eq => {
                    if let Some(Token {
                        kind: previous,
                        span: previous_span,
                    }) = tokens.last_mut()
                        && previous_span.end == start
                    {
                        let combined = match previous {
                            TokenKind::Equal => Some(TokenKind::EqualEqual),
                            TokenKind::Plus => Some(TokenKind::PlusEqual),
                            TokenKind::Minus => Some(TokenKind::MinusEqual),
                            TokenKind::Star => Some(TokenKind::StarEqual),
                            TokenKind::Slash => Some(TokenKind::SlashEqual),
                            TokenKind::Bang => Some(TokenKind::BangEqual),
                            TokenKind::Lt => Some(TokenKind::LessEqual),
                            TokenKind::Gt => Some(TokenKind::GreaterEqual),
                            _ => None,
                        };
                        if let Some(combined) = combined {
                            *previous = combined;
                            previous_span.end = span.end;
                            continue;
                        }
                    }
                    TokenKind::Equal
                }
                RawToken::Percent => {
                    if let Some(Token {
                        kind: TokenKind::Number(_, unit),
                        span: number_span,
                    }) = tokens.last_mut()
                        && number_span.end == start
                    {
                        *unit = NumberUnit::Percent;
                        number_span.end = span.end;
                        continue;
                    }
                    errors.push(ParseError {
                        message: "`%` must follow a numeric literal".to_string(),
                        span,
                    });
                    continue;
                }
                RawToken::Literal {
                    kind: LiteralKind::Int { empty_int, .. },
                    suffix_start,
                } if !empty_int => match lexeme[..suffix_start as usize].parse::<f64>() {
                    Ok(value) => TokenKind::Number(value, NumberUnit::Scalar),
                    Err(_) => {
                        errors.push(ParseError {
                            message: "invalid numeric literal".to_string(),
                            span,
                        });
                        continue;
                    }
                },
                RawToken::Literal {
                    kind: LiteralKind::Float { .. },
                    suffix_start,
                } => match lexeme[..suffix_start as usize].parse::<f64>() {
                    Ok(value) => TokenKind::Number(value, NumberUnit::Scalar),
                    Err(_) => {
                        errors.push(ParseError {
                            message: "invalid numeric literal".to_string(),
                            span,
                        });
                        continue;
                    }
                },
                RawToken::Literal {
                    kind: LiteralKind::Str { terminated: true },
                    ..
                } => match lexeme
                    .strip_prefix('"')
                    .and_then(|value| value.strip_suffix('"'))
                {
                    Some(inner) => match unescape_string(inner, self.template_expressions) {
                        Ok(value) => TokenKind::String(value),
                        Err(message) => {
                            errors.push(ParseError { message, span });
                            continue;
                        }
                    },
                    None => {
                        errors.push(ParseError {
                            message: "unterminated string literal".to_string(),
                            span,
                        });
                        continue;
                    }
                },
                RawToken::Literal {
                    kind: LiteralKind::Str { terminated: false },
                    ..
                } => {
                    errors.push(ParseError {
                        message: "unterminated string literal".to_string(),
                        span,
                    });
                    continue;
                }
                _ => {
                    errors.push(ParseError {
                        message: format!("unexpected token `{lexeme}`"),
                        span,
                    });
                    continue;
                }
            };
            tokens.push(Token { kind, span });
        }

        tokens.push(Token {
            kind: TokenKind::Eof,
            span: Span {
                start: self.offset,
                end: self.offset,
            },
        });
        if errors.is_empty() {
            Ok(tokens)
        } else {
            Err(errors)
        }
    }
}

fn unescape_string(source: &str, template_expressions: bool) -> Result<String, String> {
    if !template_expressions {
        let mut value = String::new();
        unescape_string_segment(source, &mut value)?;
        return Ok(value);
    }
    let mut value = String::new();
    let mut literal_start = 0;
    let mut cursor = 0;
    while let Some(relative_start) = source[cursor..].find("${") {
        let start = cursor + relative_start;
        unescape_string_segment(&source[literal_start..start], &mut value)?;
        let end = template_expression_end(source, start + 2)
            .ok_or_else(|| "unterminated template expression".to_string())?;
        value.push_str(&source[start..=end]);
        cursor = end + 1;
        literal_start = cursor;
    }
    unescape_string_segment(&source[literal_start..], &mut value)?;
    Ok(value)
}

fn unescape_string_segment(source: &str, value: &mut String) -> Result<(), String> {
    let mut error = None;
    crate::lex::unescape::unescape_literal(
        source,
        crate::lex::unescape::Mode::Str,
        &mut |_, character| match character {
            Ok(character) => value.push(character),
            Err(reason) if reason.is_fatal() => error = Some(format!("invalid escape: {reason:?}")),
            Err(_) => {}
        },
    );
    error.map_or(Ok(()), Err)
}

fn template_expression_end(source: &str, start: usize) -> Option<usize> {
    let mut braces = 1usize;
    let mut quote = None;
    let mut escaped = false;
    for (relative, character) in source[start..].char_indices() {
        let index = start + relative;
        if let Some(delimiter) = quote {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == delimiter {
                quote = None;
            }
            continue;
        }
        match character {
            '"' | '\'' => quote = Some(character),
            '{' => braces += 1,
            '}' => {
                braces -= 1;
                if braces == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

struct Parser {
    tokens: Vec<Token>,
    index: usize,
    errors: Vec<ParseError>,
    warnings: Vec<SyntaxWarning>,
}

fn binary_expression(left: Expr, op: BinaryOp, right: Expr) -> Expr {
    let span = Span::join(&left.span, &right.span);
    Expr {
        kind: ExprKind::Binary {
            left: Box::new(left),
            op,
            right: Box::new(right),
        },
        span,
    }
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            index: 0,
            errors: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn parse_program(mut self) -> Result<Program, Vec<ParseError>> {
        let mut statements = Vec::new();
        self.skip_separators();
        while !self.at(TokenKind::Eof) {
            statements.push(self.parse_statement());
            self.skip_separators();
        }
        if self.errors.is_empty() {
            Ok(Program {
                statements,
                warnings: self.warnings,
            })
        } else {
            Err(self.errors)
        }
    }

    fn parse_statement(&mut self) -> Stmt {
        if self.at(TokenKind::At) {
            return self.parse_attributed_statement();
        }
        if let TokenKind::Ident(name) = &self.current().kind {
            match name.as_str() {
                "import" => return self.parse_import(),
                "fn" => return self.parse_function(false),
                "type" => return self.parse_type_alias(),
                "let" | "var" => return self.parse_let(),
                "global" => return self.parse_global(),
                "if" => return self.parse_if(),
                "while" => return self.parse_while(),
                _ => {}
            }
        }
        let target = self.parse_expression();
        let assignment = match self.current().kind {
            TokenKind::Equal => Some(None),
            TokenKind::PlusEqual => Some(Some(BinaryOp::Add)),
            TokenKind::MinusEqual => Some(Some(BinaryOp::Subtract)),
            TokenKind::StarEqual => Some(Some(BinaryOp::Multiply)),
            TokenKind::SlashEqual => Some(Some(BinaryOp::Divide)),
            _ => None,
        };
        if let Some(operator) = assignment {
            self.advance();
            let value = self.parse_expression();
            let value = if let Some(operator) = operator {
                Expr {
                    span: Span::join(&target.span, &value.span),
                    kind: ExprKind::Binary {
                        left: Box::new(target.clone()),
                        op: operator,
                        right: Box::new(value),
                    },
                }
            } else {
                value
            };
            let span = Span::join(&target.span, &value.span);
            Stmt::Assign {
                target,
                value,
                span,
            }
        } else {
            Stmt::Expr(target)
        }
    }

    fn parse_attributed_statement(&mut self) -> Stmt {
        let mut attributes = Vec::new();
        while self.at(TokenKind::At) {
            let start = self.advance().span;
            let token = self.advance();
            let name = match token.kind {
                TokenKind::Ident(name) => name,
                _ => {
                    self.errors.push(ParseError {
                        message: "expected attribute name after `@`".to_string(),
                        span: token.span,
                    });
                    "<error>".to_string()
                }
            };
            attributes.push(Attribute {
                name,
                span: Span::join(&start, &token.span),
            });
            self.skip_separators();
        }

        let mut statement = self.parse_statement();
        match &mut statement {
            Stmt::Function {
                attributes: target, ..
            } => *target = attributes,
            _ => self.errors.push(ParseError {
                message: "attributes are currently supported only on functions".to_string(),
                span: attributes
                    .first()
                    .map(|attribute| attribute.span)
                    .unwrap_or(self.current().span),
            }),
        }
        statement
    }

    fn parse_import(&mut self) -> Stmt {
        let start = self.advance();
        let mut path = Vec::new();
        let mut wildcard = false;
        let mut end = start.span;
        loop {
            match self.advance() {
                Token {
                    kind: TokenKind::Ident(name),
                    span,
                } => {
                    path.push(name);
                    end = span;
                }
                token => {
                    self.errors.push(ParseError {
                        message: "expected an identifier in import path".to_string(),
                        span: token.span,
                    });
                    break;
                }
            }
            if !self.at(TokenKind::Dot) {
                break;
            }
            self.advance();
            if self.at(TokenKind::Star) {
                end = self.advance().span;
                wildcard = true;
                break;
            }
        }
        if path.is_empty() {
            self.error_here("import path cannot be empty");
        }
        Stmt::Import {
            path,
            wildcard,
            span: Span::join(&start.span, &end),
        }
    }

    fn parse_function(&mut self, exported: bool) -> Stmt {
        let start = self.advance();
        let name = match self.advance().kind {
            TokenKind::Ident(name) => name,
            _ => {
                self.error_here("expected function name after `fn`");
                "<error>".to_string()
            }
        };
        let type_parameters = self.parse_type_parameter_names();
        self.expect(TokenKind::LParen, "expected `(` after function name");
        let mut parameters = Vec::new();
        self.skip_newlines();
        while !self.at(TokenKind::RParen) && !self.at(TokenKind::Eof) {
            let parameter_start = self.current().span.clone();
            let name = match self.advance().kind {
                TokenKind::Ident(parameter) => parameter,
                _ => {
                    self.error_here("expected parameter name");
                    "<error>".to_string()
                }
            };
            let ty = if self.at(TokenKind::Colon) {
                self.advance();
                Some(self.parse_type())
            } else {
                None
            };
            let end = ty
                .as_ref()
                .map(|ty| ty.span.clone())
                .unwrap_or_else(|| parameter_start.clone());
            parameters.push(FunctionParameter {
                name,
                ty,
                span: Span::join(&parameter_start, &end),
            });
            self.skip_newlines();
            if self.at(TokenKind::Comma) {
                self.advance();
                self.skip_newlines();
            } else {
                break;
            }
        }
        self.expect(TokenKind::RParen, "expected `)` after parameters");
        let return_type = if self.at(TokenKind::Minus) && self.peek().kind == TokenKind::Gt {
            self.advance();
            self.advance();
            Some(self.parse_type())
        } else {
            None
        };
        let body = self.parse_block();
        let span = Span::join(&start.span, &body.span);
        Stmt::Function {
            attributes: Vec::new(),
            exported,
            name,
            type_parameters,
            parameters,
            return_type,
            body,
            span,
        }
    }

    fn parse_type_alias(&mut self) -> Stmt {
        let start = self.advance();
        let name = match self.advance().kind {
            TokenKind::Ident(name) => name,
            _ => {
                self.error_here("expected type name after `type`");
                "<error>".to_string()
            }
        };
        let type_parameters = self.parse_type_parameter_names();
        self.expect(TokenKind::Equal, "expected `=` after type name");
        let ty = self.parse_type();
        let span = Span::join(&start.span, &ty.span);
        Stmt::TypeAlias {
            name,
            type_parameters,
            ty,
            span,
        }
    }

    fn parse_type_parameter_names(&mut self) -> Vec<String> {
        if !self.at(TokenKind::Lt) {
            return Vec::new();
        }
        self.advance();
        let mut parameters = Vec::new();
        while !self.at(TokenKind::Gt) && !self.at(TokenKind::Eof) {
            match self.advance().kind {
                TokenKind::Ident(name) => {
                    if parameters.contains(&name) {
                        self.errors.push(ParseError {
                            message: format!("duplicate type parameter `{name}`"),
                            span: self.previous().span,
                        });
                    } else {
                        parameters.push(name);
                    }
                }
                _ => self.error_here("expected type parameter name"),
            }
            if self.at(TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        self.expect(TokenKind::Gt, "expected `>` after type parameters");
        parameters
    }

    fn parse_if(&mut self) -> Stmt {
        let start = self.advance();
        let condition = self.parse_condition_expression();
        let then_block = self.parse_block();
        self.skip_separators();
        let else_block = if matches!(self.current().kind, TokenKind::Ident(ref name) if name == "else")
        {
            self.advance();
            Some(self.parse_block())
        } else {
            None
        };
        let end = else_block
            .as_ref()
            .map(|block| block.span.clone())
            .unwrap_or_else(|| then_block.span.clone());
        Stmt::If {
            condition,
            then_block,
            else_block,
            span: Span::join(&start.span, &end),
        }
    }

    fn parse_while(&mut self) -> Stmt {
        let start = self.advance();
        let condition = self.parse_condition_expression();
        let body = self.parse_block();
        let span = Span::join(&start.span, &body.span);
        Stmt::While {
            condition,
            body,
            span,
        }
    }

    fn parse_let(&mut self) -> Stmt {
        let start = self.advance();
        let mutable = matches!(start.kind, TokenKind::Ident(ref name) if name == "var");
        let name = match self.advance().kind {
            TokenKind::Ident(name) => name,
            _ => {
                self.error_here("expected variable name");
                "<error>".to_string()
            }
        };
        let type_annotation = if self.at(TokenKind::Colon) {
            self.advance();
            Some(self.parse_type())
        } else {
            None
        };
        self.expect(TokenKind::Equal, "expected `=` after variable name");
        let value = self.parse_expression();
        let span = Span::join(&start.span, &value.span);
        Stmt::Let {
            mutable,
            name,
            type_annotation,
            value,
            span,
        }
    }

    fn parse_global(&mut self) -> Stmt {
        let start = self.advance();
        if matches!(&self.current().kind, TokenKind::Ident(name) if name == "fn") {
            return self.parse_function(true);
        }
        let name = match self.advance().kind {
            TokenKind::Ident(name) => name,
            _ => {
                self.error_here("expected global variable name");
                "<error>".to_string()
            }
        };
        let type_annotation = if self.at(TokenKind::Colon) {
            self.advance();
            Some(self.parse_type())
        } else {
            None
        };
        let value = if self.at(TokenKind::Equal) {
            self.advance();
            Some(self.parse_expression())
        } else {
            None
        };
        if type_annotation.is_none() && value.is_none() {
            self.error_here("a global requires a type or initializer");
        }
        let end = value
            .as_ref()
            .map(|value| value.span.clone())
            .or_else(|| type_annotation.as_ref().map(|ty| ty.span.clone()))
            .unwrap_or_else(|| start.span.clone());
        Stmt::Global {
            name,
            type_annotation,
            value,
            span: Span::join(&start.span, &end),
        }
    }

    fn parse_type(&mut self) -> TypeExpr {
        let start = self.current().span.clone();
        let mut ty = if self.at(TokenKind::Dot) && matches!(self.peek().kind, TokenKind::LBrace) {
            self.advance();
            self.advance();
            let mut fields = Vec::new();
            self.skip_separators();
            while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
                let field_start = self.current().span.clone();
                let name = match self.advance().kind {
                    TokenKind::Ident(name) => name,
                    _ => {
                        self.error_here("expected record field name");
                        "<error>".to_string()
                    }
                };
                self.expect(TokenKind::Colon, "expected `:` after record field name");
                let field_type = self.parse_type();
                let span = Span::join(&field_start, &field_type.span);
                fields.push(TypeField {
                    name,
                    ty: field_type,
                    span,
                });
                if self.at(TokenKind::Comma) {
                    self.advance();
                }
                self.skip_separators();
            }
            let end = self
                .expect(TokenKind::RBrace, "expected `}` after record type")
                .span;
            TypeExpr {
                kind: TypeExprKind::Record(fields),
                span: Span::join(&start, &end),
            }
        } else {
            let token = self.advance();
            let TokenKind::Ident(name) = token.kind else {
                self.error_here("expected type name");
                return TypeExpr {
                    kind: TypeExprKind::Named("<error>".to_string()),
                    span: token.span,
                };
            };
            if self.at(TokenKind::Lt) {
                self.advance();
                let mut arguments = Vec::new();
                while !self.at(TokenKind::Gt) && !self.at(TokenKind::Eof) {
                    arguments.push(self.parse_type());
                    if self.at(TokenKind::Comma) {
                        self.advance();
                    } else {
                        break;
                    }
                }
                let end = self.expect(TokenKind::Gt, "expected `>` after type arguments");
                TypeExpr {
                    kind: TypeExprKind::Applied { name, arguments },
                    span: Span::join(&token.span, &end.span),
                }
            } else {
                TypeExpr {
                    kind: TypeExprKind::Named(name),
                    span: token.span,
                }
            }
        };
        let mut nullable_suffixes = 0usize;
        while self.at(TokenKind::Question) {
            let end = self.advance();
            nullable_suffixes += 1;
            if nullable_suffixes == 1 {
                let span = Span::join(&ty.span, &end.span);
                ty = TypeExpr {
                    kind: TypeExprKind::Nullable(Box::new(ty)),
                    span,
                };
            } else {
                self.warnings.push(SyntaxWarning {
                    message: "repeated `?` is normalized to one Optional layer; write `Optional<Optional<T>>` for nested optionals".into(),
                    span: end.span,
                });
                ty.span.end = end.span.end;
            }
        }
        ty
    }

    fn parse_expression(&mut self) -> Expr {
        self.parse_expression_mode(true)
    }

    fn parse_condition_expression(&mut self) -> Expr {
        self.parse_expression_mode(false)
    }

    fn parse_expression_mode(&mut self, allow_trailing_block: bool) -> Expr {
        self.parse_colon(allow_trailing_block)
    }

    fn parse_colon(&mut self, allow_trailing_block: bool) -> Expr {
        let mut expression = self.parse_elvis(allow_trailing_block);
        if self.at(TokenKind::Colon) {
            self.advance();
            let right = self.parse_colon(false);
            let span = Span::join(&expression.span, &right.span);
            expression = Expr {
                kind: ExprKind::Binary {
                    left: Box::new(expression),
                    op: BinaryOp::Colon,
                    right: Box::new(right),
                },
                span,
            };
        }
        expression
    }

    fn parse_elvis(&mut self, allow_trailing_block: bool) -> Expr {
        let mut expression = self.parse_equality(allow_trailing_block);
        if self.at(TokenKind::Question) && matches!(self.peek().kind, TokenKind::Colon) {
            self.advance();
            self.advance();
            let fallback = self.parse_elvis(false);
            let span = Span::join(&expression.span, &fallback.span);
            expression = Expr {
                kind: ExprKind::Elvis {
                    value: Box::new(expression),
                    fallback: Box::new(fallback),
                },
                span,
            };
        }
        expression
    }

    fn parse_equality(&mut self, allow_trailing_block: bool) -> Expr {
        let mut expression = self.parse_comparison(allow_trailing_block);
        loop {
            let operator = match self.current().kind {
                TokenKind::EqualEqual => BinaryOp::Equal,
                TokenKind::BangEqual => BinaryOp::NotEqual,
                _ => break,
            };
            self.advance();
            let right = self.parse_comparison(false);
            expression = binary_expression(expression, operator, right);
        }
        expression
    }

    fn parse_comparison(&mut self, allow_trailing_block: bool) -> Expr {
        let mut expression = self.parse_additive(allow_trailing_block);
        loop {
            let operator = match self.current().kind {
                TokenKind::Lt => BinaryOp::Less,
                TokenKind::LessEqual => BinaryOp::LessEqual,
                TokenKind::Gt => BinaryOp::Greater,
                TokenKind::GreaterEqual => BinaryOp::GreaterEqual,
                _ => break,
            };
            self.advance();
            let right = self.parse_additive(false);
            expression = binary_expression(expression, operator, right);
        }
        expression
    }

    fn parse_additive(&mut self, allow_trailing_block: bool) -> Expr {
        let mut expression = self.parse_multiplicative(allow_trailing_block);
        loop {
            let operator = match self.current().kind {
                TokenKind::Plus => BinaryOp::Add,
                TokenKind::Minus => BinaryOp::Subtract,
                _ => break,
            };
            self.advance();
            let right = self.parse_multiplicative(false);
            expression = binary_expression(expression, operator, right);
        }
        expression
    }

    fn parse_multiplicative(&mut self, allow_trailing_block: bool) -> Expr {
        let mut expression = self.parse_postfix(allow_trailing_block);
        loop {
            let operator = match self.current().kind {
                TokenKind::Star => BinaryOp::Multiply,
                TokenKind::Slash => BinaryOp::Divide,
                _ => break,
            };
            self.advance();
            let right = self.parse_postfix(false);
            expression = binary_expression(expression, operator, right);
        }
        expression
    }

    fn parse_postfix(&mut self, allow_trailing_block: bool) -> Expr {
        let mut expression = self.parse_primary();
        loop {
            self.skip_member_continuation_newlines();
            if self.at(TokenKind::Dot) && matches!(self.peek().kind, TokenKind::LBrace) {
                let ExprKind::Ident(type_name) = &expression.kind else {
                    self.error_here("typed record constructor must start with a type name");
                    break;
                };
                let type_name = type_name.clone();
                self.advance();
                let map = self.parse_map(expression.span.start);
                let ExprKind::StructLiteral(fields) = map.kind else {
                    unreachable!("parse_map always returns a map expression")
                };
                expression = Expr {
                    kind: ExprKind::TypedStructLiteral { type_name, fields },
                    span: map.span,
                };
                continue;
            }
            if self.at(TokenKind::Dot) && matches!(self.peek().kind, TokenKind::Ident(_)) {
                self.advance();
                let name = match self.advance().kind {
                    TokenKind::Ident(name) => name,
                    _ => unreachable!(),
                };
                let span = Span::join(&expression.span, &self.previous().span);
                expression = Expr {
                    kind: ExprKind::Member {
                        object: Box::new(expression),
                        name,
                    },
                    span,
                };
                continue;
            }
            if self.at(TokenKind::Question) && matches!(self.peek().kind, TokenKind::Dot) {
                self.advance();
                self.advance();
                let name = match self.advance().kind {
                    TokenKind::Ident(name) => name,
                    _ => {
                        self.error_here("expected member name after `?.`");
                        "<error>".to_string()
                    }
                };
                let span = Span::join(&expression.span, &self.previous().span);
                expression = Expr {
                    kind: ExprKind::SafeMember {
                        object: Box::new(expression),
                        name,
                    },
                    span,
                };
                continue;
            }
            if matches!(&self.current().kind, TokenKind::Ident(name) if name == "as") {
                self.advance();
                let mode = if self.at(TokenKind::Question) {
                    self.advance();
                    CastMode::Optional
                } else if self.at(TokenKind::Bang) {
                    self.advance();
                    CastMode::Forced
                } else {
                    CastMode::Static
                };
                let ty = self.parse_type();
                let span = Span::join(&expression.span, &ty.span);
                expression = Expr {
                    kind: ExprKind::Cast {
                        value: Box::new(expression),
                        ty,
                        mode,
                    },
                    span,
                };
                continue;
            }
            if self.at(TokenKind::Bang) {
                let end = self.advance();
                let span = Span::join(&expression.span, &end.span);
                expression = Expr {
                    kind: ExprKind::NonNull(Box::new(expression)),
                    span,
                };
                continue;
            }
            let type_arguments = self.try_parse_call_type_arguments();
            if self.at(TokenKind::LParen) {
                let mut arguments = self.parse_arguments();
                let trailing = self.parse_optional_trailing_callable();
                let end = trailing
                    .as_ref()
                    .map(|expression| expression.span.clone())
                    .unwrap_or_else(|| self.previous().span.clone());
                let trailing_block = match trailing {
                    Some(Expr {
                        kind: ExprKind::Block(block),
                        ..
                    }) => Some(block),
                    Some(
                        lambda @ Expr {
                            kind: ExprKind::Lambda { .. },
                            ..
                        },
                    ) => {
                        let span = lambda.span;
                        arguments.push(Argument {
                            label: None,
                            value: lambda,
                            span,
                        });
                        None
                    }
                    Some(_) => unreachable!("a trailing callable is a block or lambda"),
                    None => None,
                };
                expression = Expr {
                    span: Span::join(&expression.span, &end),
                    kind: ExprKind::Call {
                        callee: Box::new(expression),
                        type_arguments,
                        arguments,
                        trailing_block,
                    },
                };
                continue;
            }
            if allow_trailing_block && self.at(TokenKind::LBrace) {
                let trailing = self
                    .parse_optional_trailing_callable()
                    .expect("the trailing brace was checked");
                let span = Span::join(&expression.span, &trailing.span);
                let (arguments, trailing_block) = match trailing {
                    Expr {
                        kind: ExprKind::Block(block),
                        ..
                    } => (Vec::new(), Some(block)),
                    lambda @ Expr {
                        kind: ExprKind::Lambda { .. },
                        ..
                    } => {
                        let argument_span = lambda.span;
                        (
                            vec![Argument {
                                label: None,
                                value: lambda,
                                span: argument_span,
                            }],
                            None,
                        )
                    }
                    _ => unreachable!("a trailing callable is a block or lambda"),
                };
                expression = Expr {
                    span,
                    kind: ExprKind::Call {
                        callee: Box::new(expression),
                        type_arguments: Vec::new(),
                        arguments,
                        trailing_block,
                    },
                };
                continue;
            }
            break;
        }
        expression
    }

    fn try_parse_call_type_arguments(&mut self) -> Vec<TypeExpr> {
        if !self.at(TokenKind::Lt) {
            return Vec::new();
        }
        let saved_index = self.index;
        let saved_errors = self.errors.len();
        let saved_warnings = self.warnings.len();
        self.advance();
        let mut arguments = Vec::new();
        while !self.at(TokenKind::Gt) && !self.at(TokenKind::Eof) {
            arguments.push(self.parse_type());
            if self.at(TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        if arguments.is_empty() || !self.at(TokenKind::Gt) {
            self.index = saved_index;
            self.errors.truncate(saved_errors);
            self.warnings.truncate(saved_warnings);
            return Vec::new();
        }
        self.advance();
        if !self.at(TokenKind::LParen) {
            self.index = saved_index;
            self.errors.truncate(saved_errors);
            self.warnings.truncate(saved_warnings);
            return Vec::new();
        }
        arguments
    }

    fn parse_primary(&mut self) -> Expr {
        let token = self.advance();
        match token.kind {
            TokenKind::Ident(name) if name == "null" => Expr {
                kind: ExprKind::Null,
                span: token.span,
            },
            TokenKind::Ellipsis => Expr {
                kind: ExprKind::Ellipsis,
                span: token.span,
            },
            TokenKind::Ident(name) if name == "true" || name == "false" => Expr {
                kind: ExprKind::Bool(name == "true"),
                span: token.span,
            },
            TokenKind::Ident(name) => Expr {
                kind: ExprKind::Ident(name),
                span: token.span,
            },
            TokenKind::Number(value, unit) => Expr {
                kind: ExprKind::Number { value, unit },
                span: token.span,
            },
            TokenKind::String(value) => Expr {
                kind: ExprKind::String(value),
                span: token.span,
            },
            TokenKind::Dollar => {
                let value = if self.at(TokenKind::LBrace) {
                    self.advance();
                    let value = self.parse_expression();
                    let end = self
                        .expect(TokenKind::RBrace, "expected `}` after binding expression")
                        .span;
                    let span = Span::join(&token.span, &end);
                    return Expr {
                        kind: ExprKind::Binding(Box::new(value)),
                        span,
                    };
                } else {
                    let ident = self.advance();
                    match ident.kind {
                        TokenKind::Ident(name) => Expr {
                            kind: ExprKind::Ident(name),
                            span: ident.span,
                        },
                        _ => {
                            return self.bad_expression(
                                ident.span,
                                "expected an identifier or `{` after `$`",
                            );
                        }
                    }
                };
                let span = Span::join(&token.span, &value.span);
                Expr {
                    kind: ExprKind::Binding(Box::new(value)),
                    span,
                }
            }
            TokenKind::Minus => {
                let value = self.parse_primary();
                let span = Span::join(&token.span, &value.span);
                Expr {
                    kind: ExprKind::UnaryMinus(Box::new(value)),
                    span,
                }
            }
            TokenKind::Dot if self.at(TokenKind::LBrace) => self.parse_map(token.span.start),
            TokenKind::Dot => match self.advance().kind {
                TokenKind::Ident(name) => Expr {
                    span: Span::join(&token.span, &self.previous().span),
                    kind: ExprKind::Symbol(name),
                },
                _ => self.bad_expression(token.span, "expected symbol name or `{` after `.`"),
            },
            TokenKind::LParen => self.parse_tuple(token.span.start),
            TokenKind::LBracket => self.parse_list(token.span.start),
            TokenKind::LBrace => self.parse_braced_expression(token.span.start),
            _ => self.bad_expression(token.span, "expected expression"),
        }
    }

    fn parse_arguments(&mut self) -> Vec<Argument> {
        self.expect(TokenKind::LParen, "expected `(`");
        let mut arguments = Vec::new();
        self.skip_newlines();
        while !self.at(TokenKind::RParen) && !self.at(TokenKind::Eof) {
            let start = self.current().span.clone();
            let label = if let TokenKind::Ident(name) = self.current().kind.clone()
                && matches!(self.peek().kind, TokenKind::Colon)
            {
                self.advance();
                self.advance();
                Some(name)
            } else {
                None
            };
            let value = self.parse_expression();
            let span = Span::join(&start, &value.span);
            arguments.push(Argument { label, value, span });
            self.skip_newlines();
            if self.at(TokenKind::Comma) {
                self.advance();
                self.skip_newlines();
            } else {
                break;
            }
        }
        self.expect(TokenKind::RParen, "expected `)` after arguments");
        arguments
    }

    fn parse_tuple(&mut self, start: usize) -> Expr {
        let mut values = Vec::new();
        self.skip_newlines();
        if self.at(TokenKind::RParen) {
            let end = self.advance().span.end;
            return Expr {
                kind: ExprKind::Unit,
                span: Span { start, end },
            };
        }
        loop {
            values.push(self.parse_expression());
            self.skip_newlines();
            if self.at(TokenKind::Comma) {
                self.advance();
                self.skip_newlines();
                continue;
            }
            break;
        }
        let end = self
            .expect(TokenKind::RParen, "expected `)` after tuple")
            .span
            .end;
        Expr {
            kind: ExprKind::Tuple(values),
            span: Span { start, end },
        }
    }

    fn parse_list(&mut self, start: usize) -> Expr {
        let mut values = Vec::new();
        self.skip_newlines();
        while !self.at(TokenKind::RBracket) && !self.at(TokenKind::Eof) {
            values.push(self.parse_expression());
            self.skip_newlines();
            if self.at(TokenKind::Comma) {
                self.advance();
                self.skip_newlines();
            } else {
                break;
            }
        }
        let end = self
            .expect(TokenKind::RBracket, "expected `]` after list")
            .span
            .end;
        Expr {
            kind: ExprKind::List(values),
            span: Span { start, end },
        }
    }

    fn parse_map(&mut self, start: usize) -> Expr {
        self.expect(TokenKind::LBrace, "expected `{` after `.`");
        let mut fields = Vec::new();
        self.skip_separators();
        while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
            let field_start = self.current().span.clone();
            let name = match self.advance().kind {
                TokenKind::Ident(name) | TokenKind::String(name) => name,
                _ => {
                    self.error_here("expected map field name");
                    "<error>".to_string()
                }
            };
            self.expect(TokenKind::Colon, "expected `:` after map field name");
            let value = self.parse_expression();
            let span = Span::join(&field_start, &value.span);
            fields.push(MapField { name, value, span });
            self.skip_newlines();
            if self.at(TokenKind::Comma) {
                self.advance();
            }
            self.skip_separators();
        }
        let end = self
            .expect(TokenKind::RBrace, "expected `}` after map")
            .span
            .end;
        Expr {
            kind: ExprKind::StructLiteral(fields),
            span: Span { start, end },
        }
    }

    fn parse_optional_trailing_callable(&mut self) -> Option<Expr> {
        if !self.at(TokenKind::LBrace) {
            return None;
        }
        let start = self.advance().span.start;
        Some(self.parse_braced_expression(start))
    }

    fn parse_block(&mut self) -> Block {
        let start = self.expect(TokenKind::LBrace, "expected `{`").span.start;
        self.parse_block_contents(start)
    }

    fn parse_block_contents(&mut self, start: usize) -> Block {
        let mut statements = Vec::new();
        self.skip_separators();
        while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
            statements.push(self.parse_statement());
            self.skip_separators();
        }
        let end = self
            .expect(TokenKind::RBrace, "expected `}` after block")
            .span
            .end;
        Block {
            statements,
            span: Span { start, end },
        }
    }

    fn parse_braced_expression(&mut self, start: usize) -> Expr {
        let checkpoint = self.index;
        let error_checkpoint = self.errors.len();
        self.skip_newlines();
        let mut parameters = Vec::new();
        let mut lambda = self.at(TokenKind::Minus) && self.peek().kind == TokenKind::Gt;
        while !lambda
            && matches!(self.current().kind, TokenKind::Ident(_))
            && matches!(self.peek().kind, TokenKind::Colon)
        {
            let parameter_start = self.current().span.clone();
            let TokenKind::Ident(name) = self.advance().kind else {
                unreachable!("the lambda parameter lookahead requires an identifier")
            };
            self.advance();
            let ty = self.parse_type();
            let span = Span::join(&parameter_start, &ty.span);
            parameters.push(FunctionParameter {
                name,
                ty: Some(ty),
                span,
            });
            self.skip_newlines();
            if self.at(TokenKind::Comma) {
                self.advance();
                self.skip_newlines();
                continue;
            }
            lambda = self.at(TokenKind::Minus) && self.peek().kind == TokenKind::Gt;
            break;
        }
        if lambda {
            self.advance();
            self.advance();
            let body = self.parse_block_contents(start);
            return Expr {
                span: body.span,
                kind: ExprKind::Lambda { parameters, body },
            };
        }

        self.index = checkpoint;
        self.errors.truncate(error_checkpoint);
        let block = self.parse_block_contents(start);
        Expr {
            span: block.span,
            kind: ExprKind::Block(block),
        }
    }

    fn skip_separators(&mut self) {
        while matches!(self.current().kind, TokenKind::Newline | TokenKind::Semi) {
            self.advance();
        }
    }

    fn skip_newlines(&mut self) {
        while self.at(TokenKind::Newline) {
            self.advance();
        }
    }

    /// A leading `.` makes the following physical line an unambiguous continuation
    /// of the previous expression. Semicolons remain hard statement boundaries.
    fn skip_member_continuation_newlines(&mut self) {
        let mut next = self.index;
        while matches!(self.tokens[next].kind, TokenKind::Newline) {
            next += 1;
        }
        if next > self.index && matches!(self.tokens[next].kind, TokenKind::Dot) {
            self.index = next;
        }
    }

    fn at(&self, kind: TokenKind) -> bool {
        std::mem::discriminant(&self.current().kind) == std::mem::discriminant(&kind)
    }

    fn current(&self) -> &Token {
        &self.tokens[self.index]
    }

    fn peek(&self) -> &Token {
        &self.tokens[(self.index + 1).min(self.tokens.len() - 1)]
    }

    fn previous(&self) -> &Token {
        &self.tokens[self.index.saturating_sub(1)]
    }

    fn advance(&mut self) -> Token {
        let token = self.current().clone();
        if !matches!(token.kind, TokenKind::Eof) {
            self.index += 1;
        }
        token
    }

    fn expect(&mut self, kind: TokenKind, message: &str) -> Token {
        if self.at(kind) {
            self.advance()
        } else {
            self.error_here(message);
            self.advance()
        }
    }

    fn bad_expression(&mut self, span: Span, message: &str) -> Expr {
        self.errors.push(ParseError {
            message: message.to_string(),
            span: span.clone(),
        });
        Expr {
            kind: ExprKind::Ident("<error>".to_string()),
            span,
        }
    }

    fn error_here(&mut self, message: &str) {
        self.errors.push(ParseError {
            message: message.to_string(),
            span: self.current().span.clone(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncated_unicode_source_reports_errors_without_panicking() {
        let source = "narrate(\"welcome ${name ?: \"guest\"} 🚀\")";
        let mut boundaries = source
            .char_indices()
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        boundaries.push(source.len());
        for end in boundaries {
            let _ = parse_program(&source[..end]);
        }
    }

    fn expression(source: &str) -> Expr {
        let program = parse_program(source).unwrap();
        let [Stmt::Expr(expression)] = program.statements.as_slice() else {
            panic!("expected one expression");
        };
        expression.clone()
    }

    #[test]
    fn parses_unit_literal_separately_from_tuples() {
        assert_eq!(expression("()").kind, ExprKind::Unit);
        assert!(matches!(expression("(1, 2)").kind, ExprKind::Tuple(values) if values.len() == 2));
    }

    #[test]
    fn parses_typed_lambda_parameters_without_confusing_dialogue_colons() {
        let lambda = expression("{ index: Int, label: String -> label }");
        let ExprKind::Lambda { parameters, body } = lambda.kind else {
            panic!("expected a typed lambda");
        };
        assert_eq!(parameters.len(), 2);
        assert_eq!(parameters[0].name, "index");
        assert_eq!(parameters[1].name, "label");
        assert_eq!(body.statements.len(), 1);

        assert!(matches!(
            expression("{ speaker: \"line\" }").kind,
            ExprKind::Block(_)
        ));

        let trailing = expression("render { index: Int, label: String -> label }");
        let ExprKind::Call {
            arguments,
            trailing_block,
            ..
        } = trailing.kind
        else {
            panic!("expected a call with a trailing lambda");
        };
        assert!(trailing_block.is_none());
        assert!(matches!(
            arguments.as_slice(),
            [Argument {
                value: Expr {
                    kind: ExprKind::Lambda { .. },
                    ..
                },
                ..
            }]
        ));
    }

    #[test]
    fn parses_camera_named_arguments_and_symbol_positions() {
        let expression = expression("camera.zoom(1.2, at: .center, duration: 0.5, ease: .easeOut)");
        let ExprKind::Call { arguments, .. } = expression.kind else {
            panic!("expected call");
        };
        assert_eq!(arguments.len(), 4);
        assert_eq!(arguments[1].label.as_deref(), Some("at"));
        assert_eq!(
            arguments[1].value.kind,
            ExprKind::Symbol("center".to_string())
        );
        assert_eq!(
            arguments[3].value.kind,
            ExprKind::Symbol("easeOut".to_string())
        );
    }

    #[test]
    fn parses_member_call_chains_across_lines() {
        let expression = expression(
            r#"camera()
                .offset(10, 20, 30)
                .projection(.perspective)
                .time(0.5)"#,
        );
        let ExprKind::Call { callee, .. } = expression.kind else {
            panic!("expected final call");
        };
        assert!(matches!(
            callee.kind,
            ExprKind::Member { ref name, .. } if name == "time"
        ));
    }

    #[test]
    fn parses_tuple_and_percent_positions() {
        let expression = expression("camera.zoom(1.2, at: (20%, 30%))");
        let ExprKind::Call { arguments, .. } = expression.kind else {
            panic!("expected call");
        };
        let ExprKind::Tuple(values) = &arguments[1].value.kind else {
            panic!("expected position tuple");
        };
        assert!(matches!(
            values[0].kind,
            ExprKind::Number {
                value: 20.0,
                unit: NumberUnit::Percent
            }
        ));
        assert!(matches!(
            values[1].kind,
            ExprKind::Number {
                value: 30.0,
                unit: NumberUnit::Percent
            }
        ));
    }

    #[test]
    fn parses_seq_and_par_trailing_blocks() {
        let program = parse_program(
            r#"
                let handle = seq {
                    char("alice").e("shock").fade(0.5)
                    char("alice").e("happy").fade(0.5).ease(.easeInOut)
                }
                par {
                    camera.zoom(1.2)
                }
                wait(handle)
            "#,
        )
        .unwrap();
        let Stmt::Let { value, .. } = &program.statements[0] else {
            panic!("expected let");
        };
        assert!(matches!(
            value.kind,
            ExprKind::Call {
                trailing_block: Some(_),
                ..
            }
        ));
        assert_eq!(program.statements.len(), 3);
    }

    #[test]
    fn parses_nested_map_literals() {
        let expression =
            expression(".{ field1: \"string\", field2: 0.2, field3: .{ nested: true } }");
        let ExprKind::StructLiteral(fields) = expression.kind else {
            panic!("expected map");
        };
        assert_eq!(fields.len(), 3);
        assert!(matches!(fields[2].value.kind, ExprKind::StructLiteral(_)));
    }

    #[test]
    fn parses_function_definitions() {
        let program = parse_program(
            r#"
                fn decorate(actor, emotion) {
                    actor.e(emotion)
                }
                decorate(char("alice"), "happy")
            "#,
        )
        .unwrap();
        let Stmt::Function {
            name,
            parameters,
            body,
            ..
        } = &program.statements[0]
        else {
            panic!("expected function definition")
        };
        assert_eq!(name, "decorate");
        assert_eq!(
            parameters
                .iter()
                .map(|parameter| parameter.name.as_str())
                .collect::<Vec<_>>(),
            ["actor", "emotion"]
        );
        assert_eq!(body.statements.len(), 1);
    }

    #[test]
    fn parses_exported_global_functions() {
        let program = parse_program("global fn greet(name: String) { name }")
            .expect("global function must parse");
        assert!(matches!(
            &program.statements[0],
            Stmt::Function { exported: true, name, .. } if name == "greet"
        ));
    }

    #[test]
    fn parses_function_attributes_without_embedding_ui_semantics() {
        let program = parse_program("@entry\nglobal fn app(title: String) -> String { title }")
            .expect("a function attribute must parse");
        let Stmt::Function {
            attributes,
            exported,
            name,
            ..
        } = &program.statements[0]
        else {
            panic!("expected an attributed function")
        };
        assert!(*exported);
        assert_eq!(name, "app");
        assert_eq!(attributes.len(), 1);
        assert_eq!(attributes[0].name, "entry");
    }

    #[test]
    fn parses_wildcard_module_imports() {
        let program = parse_program("import ui.widgets.*\nbutton(\"Continue\")")
            .expect("module import must parse");
        assert!(matches!(
            &program.statements[0],
            Stmt::Import { path, wildcard: true, .. }
                if path == &["ui".to_string(), "widgets".to_string()]
        ));
    }

    #[test]
    fn parses_type_aliases_and_typed_function_signatures() {
        let program = parse_program(
            r#"
                type Player = .{ name: String, health: Int }
                fn health(player: Player) -> Int { player.health }
            "#,
        )
        .expect("typed declarations must parse");
        assert!(
            matches!(program.statements[0], Stmt::TypeAlias { ref name, .. } if name == "Player")
        );
        let Stmt::Function {
            parameters,
            return_type: Some(return_type),
            ..
        } = &program.statements[1]
        else {
            panic!("expected typed function")
        };
        assert!(parameters[0].ty.is_some());
        assert!(matches!(return_type.kind, TypeExprKind::Named(ref name) if name == "Int"));
    }

    #[test]
    fn shared_lexer_handles_unicode_escapes_and_nested_comments() {
        let program = parse_program(
            r#"
                /* outer /* nested */ comment */
                let café_actor = char("alice \u{1f338}")
                café_actor.e("smile")
            "#,
        )
        .expect("shared lexer syntax must parse");

        let Stmt::Let { name, value, .. } = &program.statements[0] else {
            panic!("expected unicode binding");
        };
        assert_eq!(name, "café_actor");
        let ExprKind::Call { arguments, .. } = &value.kind else {
            panic!("expected character call");
        };
        assert!(matches!(
            arguments[0].value.kind,
            ExprKind::String(ref value) if value == "alice 🌸"
        ));
    }

    #[test]
    fn parses_quoted_map_field_names() {
        let expression = expression(r#".{ regions: .{ "alice/body": (1, 2, 3, 4) } }"#);
        let ExprKind::StructLiteral(fields) = expression.kind else {
            panic!("expected map");
        };
        let ExprKind::StructLiteral(regions) = &fields[0].value.kind else {
            panic!("expected nested map");
        };
        assert_eq!(regions[0].name, "alice/body");
    }

    #[test]
    fn parses_dialogue_operator_and_ellipsis() {
        let program = parse_program(
            r#"
                let alice = char("alice")
                alice: "first"
                ...: "continued"
                "narration"
                char("alice").e("happy"): "inline"
            "#,
        )
        .expect("dialogue sugar must parse");
        assert_eq!(program.statements.len(), 5);
        let Stmt::Expr(expression) = &program.statements[2] else {
            panic!("expected continued dialogue expression")
        };
        assert!(matches!(
            &expression.kind,
            ExprKind::Binary {
                left,
                op: BinaryOp::Colon,
                right,
            } if matches!(left.kind, ExprKind::Ellipsis)
                && matches!(right.kind, ExprKind::String(ref value) if value == "continued")
        ));
        assert!(matches!(
            &program.statements[3],
            Stmt::Expr(Expr { kind: ExprKind::String(value), .. }) if value == "narration"
        ));
    }

    #[test]
    fn parses_arithmetic_comparison_precedence_and_compound_assignment() {
        let program = parse_program(
            r#"
                let value = 1 + 2 * 3
                while value < 8 {
                    value += 1
                    value -= 1
                    value *= 2
                    value /= 2
                }
            "#,
        )
        .expect("common arithmetic syntax must parse");

        let Stmt::Let { value, .. } = &program.statements[0] else {
            panic!("expected let binding")
        };
        assert!(matches!(
            &value.kind,
            ExprKind::Binary {
                op: BinaryOp::Add,
                right,
                ..
            } if matches!(right.kind, ExprKind::Binary { op: BinaryOp::Multiply, .. })
        ));
        let Stmt::While {
            condition, body, ..
        } = &program.statements[1]
        else {
            panic!("expected while loop")
        };
        assert!(matches!(
            condition.kind,
            ExprKind::Binary {
                op: BinaryOp::Less,
                ..
            }
        ));
        assert_eq!(body.statements.len(), 4);
        for (statement, operator) in body.statements.iter().zip([
            BinaryOp::Add,
            BinaryOp::Subtract,
            BinaryOp::Multiply,
            BinaryOp::Divide,
        ]) {
            assert!(matches!(
                statement,
                Stmt::Assign {
                    value: Expr {
                        kind: ExprKind::Binary { op, .. },
                        ..
                    },
                    ..
                } if *op == operator
            ));
        }
    }

    #[test]
    fn parses_globals_nullable_types_assignment_and_lists() {
        let program = parse_program(
            r#"
                global player: .{ name: String, health: Int } = .{ name: "", health: 123 }
                global nickname: String? = null
                global lazyName: String
                lazyName = "alice"
                let values: List<Int> = [1, 2, 3]
                let shown = nickname ?: "fallback"
                nickname?.length
                nickname!
            "#,
        )
        .expect("typed globals and null-safety syntax must parse");
        assert!(matches!(
            &program.statements[0],
            Stmt::Global {
                type_annotation: Some(TypeExpr {
                    kind: TypeExprKind::Record(_),
                    ..
                }),
                value: Some(_),
                ..
            }
        ));
        assert!(matches!(&program.statements[3], Stmt::Assign { .. }));
        let Stmt::Let {
            type_annotation:
                Some(TypeExpr {
                    kind: TypeExprKind::Applied { name, arguments },
                    ..
                }),
            value: Expr {
                kind: ExprKind::List(_),
                ..
            },
            ..
        } = &program.statements[4]
        else {
            panic!("expected a typed list literal")
        };
        assert_eq!(name, "List");
        assert_eq!(arguments.len(), 1);
        assert!(matches!(
            &program.statements[5],
            Stmt::Let {
                value: Expr {
                    kind: ExprKind::Elvis { .. },
                    ..
                },
                ..
            }
        ));
        assert!(matches!(
            &program.statements[6],
            Stmt::Expr(Expr {
                kind: ExprKind::SafeMember { .. },
                ..
            })
        ));
        assert!(matches!(
            &program.statements[7],
            Stmt::Expr(Expr {
                kind: ExprKind::NonNull(_),
                ..
            })
        ));
    }

    #[test]
    fn parses_static_optional_and_forced_casts() {
        let program = parse_program(
            "let a = value as Float\nlet b = value as? String\nlet c = value as! Int\nlet d: Optional<String> = null",
        )
        .expect("cast expressions and explicit Optional types must parse");
        for (statement, expected) in program.statements[..3].iter().zip([
            CastMode::Static,
            CastMode::Optional,
            CastMode::Forced,
        ]) {
            let Stmt::Let {
                value:
                    Expr {
                        kind: ExprKind::Cast { mode, .. },
                        ..
                    },
                ..
            } = statement
            else {
                panic!("expected cast initializer")
            };
            assert_eq!(*mode, expected);
        }
        let Stmt::Let {
            type_annotation:
                Some(TypeExpr {
                    kind: TypeExprKind::Applied { name, arguments },
                    ..
                }),
            ..
        } = &program.statements[3]
        else {
            panic!("expected an applied Optional type")
        };
        assert_eq!(name, "Optional");
        assert_eq!(arguments.len(), 1);
    }

    #[test]
    fn nullable_suffixes_normalize_but_explicit_optional_types_nest() {
        let program = parse_program(
            "let flat: String???? = null\nlet nested: Optional<Optional<String>> = .some(null)",
        )
        .expect("optional syntax parses");
        assert_eq!(program.warnings.len(), 3);
        let Stmt::Let {
            type_annotation: Some(flat),
            ..
        } = &program.statements[0]
        else {
            panic!("expected annotated local")
        };
        assert!(matches!(flat.kind, TypeExprKind::Nullable(_)));
        let Stmt::Let {
            type_annotation: Some(nested),
            ..
        } = &program.statements[1]
        else {
            panic!("expected annotated local")
        };
        assert!(
            matches!(nested.kind, TypeExprKind::Applied { ref name, .. } if name == "Optional")
        );
    }

    #[test]
    fn parses_generic_functions_and_type_aliases() {
        let program = parse_program(
            "type Player<T> = .{ name: T, score: Int }\nfn identity<T>(value: T) -> T { value }",
        )
        .expect("generic declarations parse");
        assert!(matches!(
            &program.statements[0],
            Stmt::TypeAlias { type_parameters, .. } if type_parameters == &["T"]
        ));
        assert!(matches!(
            &program.statements[1],
            Stmt::Function { type_parameters, .. } if type_parameters == &["T"]
        ));
    }

    #[test]
    fn parses_explicit_generic_call_arguments_without_confusing_comparison() {
        let program = parse_program("identity<Optional<String>>(null)\n1 < 2")
            .expect("generic calls and comparisons parse independently");
        let Stmt::Expr(Expr {
            kind: ExprKind::Call { type_arguments, .. },
            ..
        }) = &program.statements[0]
        else {
            panic!("expected a generic call")
        };
        assert_eq!(type_arguments.len(), 1);
        assert!(matches!(
            type_arguments[0].kind,
            TypeExprKind::Applied { ref name, .. } if name == "Optional"
        ));
        assert!(matches!(
            program.statements[1],
            Stmt::Expr(Expr {
                kind: ExprKind::Binary {
                    op: BinaryOp::Less,
                    ..
                },
                ..
            })
        ));
    }

    #[test]
    fn parses_explicit_binding_shorthand_and_expression_forms() {
        let program =
            parse_program("let name = \"alice\"\nobserve($name)\nobserve(${name == \"alice\"})")
                .expect("binding syntax parses");
        for statement in &program.statements[1..] {
            let Stmt::Expr(Expr {
                kind: ExprKind::Call { arguments, .. },
                ..
            }) = statement
            else {
                panic!("expected call statement")
            };
            assert!(matches!(arguments[0].value.kind, ExprKind::Binding(_)));
        }
    }

    #[test]
    fn binding_shorthand_consumes_only_one_identifier() {
        let program = parse_program("observe($dialogue.text)").expect("binding syntax parses");
        let Stmt::Expr(Expr {
            kind: ExprKind::Call { arguments, .. },
            ..
        }) = &program.statements[0]
        else {
            panic!("expected call statement")
        };
        let ExprKind::Member { object, name } = &arguments[0].value.kind else {
            panic!("member access must remain outside shorthand binding")
        };
        assert_eq!(name, "text");
        assert!(matches!(
            object.kind,
            ExprKind::Binding(ref value)
                if matches!(value.kind, ExprKind::Ident(ref name) if name == "dialogue")
        ));
    }
}
