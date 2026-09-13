//! String interpolation is ordinary, resumable expression bytecode.
use super::*;

impl<'hir, 'manifest> Lowerer<'hir, 'manifest> {
    pub(super) fn lower_string(&mut self, text: &str, span: Span) -> &'hir HirExpr<'hir> {
        let mut remaining = text;
        let mut pieces = Vec::new();
        while let Some(start) = remaining.find("${") {
            pieces.push(self.alloc_expression(
                HirExprKind::Literal(HirLiteral::String(
                    self.arena.alloc_str(&remaining[..start]),
                )),
                ScriptType::String,
                span,
            ));
            let expression = &remaining[start + 2..];
            let Some(end) = crate::template::expression_end(expression) else {
                self.error("unterminated string interpolation", span);
                break;
            };
            match crate::parse::parse_interpolation(&expression[..end], span) {
                Ok(program) => match program.statements.as_slice() {
                    [Stmt::Expr(expression)] => {
                        let value = self.lower_expression(expression);
                        pieces.push(self.alloc_expression(
                            HirExprKind::Intrinsic {
                                operation: crate::intrinsics::Intrinsic::ValueToString,
                                argument: value,
                            },
                            ScriptType::String,
                            span,
                        ));
                    }
                    _ => self.error("string interpolation requires exactly one expression", span),
                },
                Err(errors) => {
                    for error in errors {
                        self.error(error.message, error.span);
                    }
                }
            }
            remaining = &expression[end + 1..];
        }
        pieces.push(self.alloc_expression(
            HirExprKind::Literal(HirLiteral::String(self.arena.alloc_str(remaining))),
            ScriptType::String,
            span,
        ));
        let mut result = pieces[0];
        for piece in &pieces[1..] {
            result = self.alloc_expression(
                HirExprKind::Binary {
                    left: result,
                    op: BinaryOp::Add,
                    right: piece,
                },
                ScriptType::String,
                span,
            );
        }
        result
    }
}
