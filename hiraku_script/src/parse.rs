//! The canonical Hiraku Script parser, built on the crate's shared lexer.

use crate::{ast::*, span::Span};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    pub span: Span,
}

pub fn parse_program(source: &str) -> Result<Program, Vec<ParseError>> {
    let tokens = TokenAdapter::new(source).collect()?;
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
    Question,
    Bang,
    Lt,
    Gt,
    Minus,
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
}

impl<'a> TokenAdapter<'a> {
    fn new(source: &'a str) -> Self {
        Self { source, offset: 0 }
    }

    fn collect(mut self) -> Result<Vec<Token>, Vec<ParseError>> {
        let mut tokens: Vec<Token> = Vec::new();
        let mut errors = Vec::new();

        for raw in crate::lex::tokenize(self.source) {
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
                        kind: previous @ TokenKind::Equal,
                        span: previous_span,
                    }) = tokens.last_mut()
                        && previous_span.end == start
                    {
                        *previous = TokenKind::EqualEqual;
                        previous_span.end = span.end;
                        continue;
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
                } => match unescape_string(&lexeme[1..lexeme.len() - 1]) {
                    Ok(value) => TokenKind::String(value),
                    Err(message) => {
                        errors.push(ParseError { message, span });
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

fn unescape_string(source: &str) -> Result<String, String> {
    let mut value = String::new();
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
    error.map_or(Ok(value), Err)
}

struct Parser {
    tokens: Vec<Token>,
    index: usize,
    errors: Vec<ParseError>,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            index: 0,
            errors: Vec::new(),
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
            Ok(Program { statements })
        } else {
            Err(self.errors)
        }
    }

    fn parse_statement(&mut self) -> Stmt {
        if let TokenKind::Ident(name) = &self.current().kind {
            match name.as_str() {
                "fn" => return self.parse_function(),
                "type" => return self.parse_type_alias(),
                "let" | "var" => return self.parse_let(),
                "global" => return self.parse_global(),
                "if" => return self.parse_if(),
                "while" => return self.parse_while(),
                _ => {}
            }
        }
        let target = self.parse_expression();
        if self.at(TokenKind::Equal) {
            self.advance();
            let value = self.parse_expression();
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

    fn parse_function(&mut self) -> Stmt {
        let start = self.advance();
        let name = match self.advance().kind {
            TokenKind::Ident(name) => name,
            _ => {
                self.error_here("expected function name after `fn`");
                "<error>".to_string()
            }
        };
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
            name,
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
        self.expect(TokenKind::Equal, "expected `=` after type name");
        let ty = self.parse_type();
        let span = Span::join(&start.span, &ty.span);
        Stmt::TypeAlias { name, ty, span }
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
            if name == "List" && self.at(TokenKind::Lt) {
                self.advance();
                let element = self.parse_type();
                let end = self.expect(TokenKind::Gt, "expected `>` after List element type");
                TypeExpr {
                    kind: TypeExprKind::List(Box::new(element)),
                    span: Span::join(&token.span, &end.span),
                }
            } else {
                TypeExpr {
                    kind: TypeExprKind::Named(name),
                    span: token.span,
                }
            }
        };
        if self.at(TokenKind::Question) {
            let end = self.advance();
            let span = Span::join(&ty.span, &end.span);
            ty = TypeExpr {
                kind: TypeExprKind::Nullable(Box::new(ty)),
                span,
            };
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
        let mut expression = self.parse_primary();
        loop {
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
            if self.at(TokenKind::Bang) {
                let end = self.advance();
                let span = Span::join(&expression.span, &end.span);
                expression = Expr {
                    kind: ExprKind::NonNull(Box::new(expression)),
                    span,
                };
                continue;
            }
            if self.at(TokenKind::LParen) {
                let arguments = self.parse_arguments();
                let trailing_block = self.parse_optional_trailing_block();
                let end = trailing_block
                    .as_ref()
                    .map(|block| block.span.clone())
                    .unwrap_or_else(|| self.previous().span.clone());
                expression = Expr {
                    span: Span::join(&expression.span, &end),
                    kind: ExprKind::Call {
                        callee: Box::new(expression),
                        arguments,
                        trailing_block,
                    },
                };
                continue;
            }
            if allow_trailing_block && self.at(TokenKind::LBrace) {
                let block = self.parse_block();
                let span = Span::join(&expression.span, &block.span);
                expression = Expr {
                    span,
                    kind: ExprKind::Call {
                        callee: Box::new(expression),
                        arguments: Vec::new(),
                        trailing_block: Some(block),
                    },
                };
                continue;
            }
            break;
        }
        if self.at(TokenKind::EqualEqual) {
            self.advance();
            let right = self.parse_expression_mode(false);
            let span = Span::join(&expression.span, &right.span);
            expression = Expr {
                kind: ExprKind::Binary {
                    left: Box::new(expression),
                    op: BinaryOp::Equal,
                    right: Box::new(right),
                },
                span,
            };
        }
        if self.at(TokenKind::Question) && matches!(self.peek().kind, TokenKind::Colon) {
            self.advance();
            self.advance();
            let fallback = self.parse_expression_mode(false);
            let span = Span::join(&expression.span, &fallback.span);
            expression = Expr {
                kind: ExprKind::Elvis {
                    value: Box::new(expression),
                    fallback: Box::new(fallback),
                },
                span,
            };
        }
        if self.at(TokenKind::Colon) {
            self.advance();
            let right = self.parse_expression_mode(false);
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
            TokenKind::LBrace => {
                self.index -= 1;
                let block = self.parse_block();
                Expr {
                    span: block.span.clone(),
                    kind: ExprKind::Block(block),
                }
            }
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
                kind: ExprKind::Tuple(values),
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
            kind: ExprKind::Map(fields),
            span: Span { start, end },
        }
    }

    fn parse_optional_trailing_block(&mut self) -> Option<Block> {
        self.at(TokenKind::LBrace).then(|| self.parse_block())
    }

    fn parse_block(&mut self) -> Block {
        let start = self.expect(TokenKind::LBrace, "expected `{`").span.start;
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

    fn expression(source: &str) -> Expr {
        let program = parse_program(source).unwrap();
        let [Stmt::Expr(expression)] = program.statements.as_slice() else {
            panic!("expected one expression");
        };
        expression.clone()
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
                    char("ema").e("shock").fade(0.5)
                    char("ema").e("happy").fade(0.5).ease(.easeInOut)
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
        let ExprKind::Map(fields) = expression.kind else {
            panic!("expected map");
        };
        assert_eq!(fields.len(), 3);
        assert!(matches!(fields[2].value.kind, ExprKind::Map(_)));
    }

    #[test]
    fn parses_function_definitions() {
        let program = parse_program(
            r#"
                fn decorate(actor, emotion) {
                    actor.e(emotion)
                }
                decorate(char("Alice"), "happy")
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
                let 艾玛 = char("樱羽\u{827e}玛")
                艾玛.e("微笑")
            "#,
        )
        .expect("shared lexer syntax must parse");

        let Stmt::Let { name, value, .. } = &program.statements[0] else {
            panic!("expected unicode binding");
        };
        assert_eq!(name, "艾玛");
        let ExprKind::Call { arguments, .. } = &value.kind else {
            panic!("expected character call");
        };
        assert!(matches!(
            arguments[0].value.kind,
            ExprKind::String(ref value) if value == "樱羽艾玛"
        ));
    }

    #[test]
    fn parses_quoted_map_field_names() {
        let expression = expression(r#".{ regions: .{ "ema/body": (1, 2, 3, 4) } }"#);
        let ExprKind::Map(fields) = expression.kind else {
            panic!("expected map");
        };
        let ExprKind::Map(regions) = &fields[0].value.kind else {
            panic!("expected nested map");
        };
        assert_eq!(regions[0].name, "ema/body");
    }

    #[test]
    fn parses_dialogue_operator_and_ellipsis() {
        let program = parse_program(
            r#"
                let ema = char("ema")
                ema: "first"
                ...: "continued"
                "narration"
                char("ema").e("happy"): "inline"
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
    fn parses_globals_nullable_types_assignment_and_lists() {
        let program = parse_program(
            r#"
                global player: .{ name: String, health: Int } = .{ name: "", health: 123 }
                global nickname: String? = null
                global lazyName: String
                lazyName = "Alice"
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
        assert!(matches!(
            &program.statements[4],
            Stmt::Let {
                type_annotation: Some(TypeExpr {
                    kind: TypeExprKind::List(_),
                    ..
                }),
                value: Expr {
                    kind: ExprKind::List(_),
                    ..
                },
                ..
            }
        ));
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
}
