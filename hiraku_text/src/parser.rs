use std::collections::BTreeMap;
use typst_syntax::{
    Source,
    ast::{self, AstNode, Expr},
};

use crate::{
    Document, Ruby, TextError,
    library::{self, Function},
};

const MAX_BYTES: usize = 64 * 1024;
const MAX_DEPTH: usize = 64;
const MAX_NODES: usize = 16 * 1024;

pub fn parse(text: &str) -> Result<Document, TextError> {
    if text.len() > MAX_BYTES {
        return Err(TextError::new(
            "rich text exceeds the 64 KiB limit",
            0..text.len(),
        ));
    }
    let source = Source::detached(text);
    if let Some(error) = source.root().errors_and_warnings().0.first() {
        let range = match error.span.get() {
            typst_syntax::DiagSpanKind::Number { num, sub_range, .. } => {
                source.range(num, sub_range)
            }
            _ => None,
        }
        .unwrap_or(0..0);
        return Err(TextError::new(error.message.to_string(), range));
    }
    let mut evaluator = Evaluator {
        source: &source,
        constants: BTreeMap::from([("br".into(), Value::Content(Document::plain("\n")))]),
        nodes: 0,
    };
    let mut result = Document::default();
    evaluator.markup(
        source.root().cast().expect("Typst source root is markup"),
        &mut result,
        0,
    )?;
    Ok(result)
}

#[derive(Clone)]
enum Value {
    String(String),
    Content(Document),
    Empty,
}

struct Evaluator<'a> {
    source: &'a Source,
    constants: BTreeMap<String, Value>,
    nodes: usize,
}

impl Evaluator<'_> {
    fn error(&self, expr: Expr<'_>, message: impl Into<String>) -> TextError {
        TextError::new(
            message,
            self.source
                .find(expr.span())
                .map(|node| node.range())
                .unwrap_or(0..0),
        )
    }

    fn markup(
        &mut self,
        markup: ast::Markup<'_>,
        out: &mut Document,
        depth: usize,
    ) -> Result<(), TextError> {
        let saved = self.constants.clone();
        for expr in markup.exprs() {
            let value = self.expr(expr, depth + 1)?;
            match value {
                Value::Content(document) => out.append(&document),
                Value::String(text) => out.append(&Document::plain(text)),
                Value::Empty => {}
            }
            if out.text.len() > MAX_BYTES {
                return Err(self.error(expr, "expanded rich text exceeds the 64 KiB limit"));
            }
        }
        self.constants = saved;
        Ok(())
    }

    fn expr(&mut self, expr: Expr<'_>, depth: usize) -> Result<Value, TextError> {
        self.nodes += 1;
        if depth > MAX_DEPTH || self.nodes > MAX_NODES {
            return Err(self.error(expr, "rich text evaluation limit exceeded"));
        }
        let value = match expr {
            Expr::Text(v) => Value::String(v.get().to_string()),
            Expr::Space(v) => Value::String(v.to_untyped().leaf_text().to_string()),
            Expr::Shorthand(v) => Value::String(v.to_untyped().leaf_text().to_string()),
            Expr::SmartQuote(v) => Value::String(v.to_untyped().leaf_text().to_string()),
            Expr::Parbreak(v) => Value::String(v.to_untyped().leaf_text().to_string()),
            Expr::Linebreak(_) => Value::String("\n".into()),
            Expr::Escape(v) => Value::String(v.get().to_string()),
            Expr::Str(v) => Value::String(v.get().to_string()),
            Expr::ContentBlock(v) => Value::Content(self.body(v.body(), depth)?),
            Expr::Strong(v) => {
                let mut document = self.body(v.body(), depth)?;
                for style in &mut document.styles {
                    style.bold = true;
                }
                Value::Content(document)
            }
            Expr::Emph(v) => {
                let mut document = self.body(v.body(), depth)?;
                for style in &mut document.styles {
                    style.italic = true;
                }
                Value::Content(document)
            }
            Expr::Ident(v) => self
                .constants
                .get(v.get().as_str())
                .cloned()
                .ok_or_else(|| self.error(expr, format!("unknown text constant `{}`", v.get())))?,
            Expr::LetBinding(v) => {
                let ast::LetBindingKind::Normal(ast::Pattern::Normal(Expr::Ident(name))) = v.kind()
                else {
                    return Err(self.error(expr, "only simple text constants are allowed; functions and destructuring are disabled"));
                };
                if library::lookup(name.get().as_str()).is_some() {
                    return Err(self.error(expr, "text library functions cannot be shadowed"));
                }
                let init = v
                    .init()
                    .ok_or_else(|| self.error(expr, "text constants require an initializer"))?;
                let value = self.expr(init, depth + 1)?;
                self.constants.insert(name.get().to_string(), value);
                Value::Empty
            }
            Expr::FuncCall(call) => {
                let Expr::Ident(name) = call.callee() else {
                    return Err(
                        self.error(expr, "only calls to the Hiraku text library are allowed")
                    );
                };
                let function = library::lookup(name.get().as_str()).ok_or_else(|| self.error(expr,
                    format!("unknown Hiraku text function `{}`; the Typst standard library is disabled", name.get())))?;
                let mut arguments = Vec::new();
                for argument in call.args().items() {
                    let ast::Arg::Pos(argument) = argument else {
                        return Err(
                            self.error(expr, "named and spread text arguments are disabled")
                        );
                    };
                    arguments.push(self.expr(argument, depth + 1)?);
                }
                self.call(function, arguments, expr)?
            }
            _ => {
                return Err(self.error(
                    expr,
                    format!(
                        "Typst {:?} is not allowed in VN inline text",
                        expr.to_untyped().kind()
                    ),
                ));
            }
        };
        Ok(value)
    }

    fn body(&mut self, body: ast::Markup<'_>, depth: usize) -> Result<Document, TextError> {
        let mut document = Document::default();
        self.markup(body, &mut document, depth)?;
        Ok(document)
    }

    fn call(
        &self,
        function: Function,
        mut arguments: Vec<Value>,
        expr: Expr<'_>,
    ) -> Result<Value, TextError> {
        let arity = match function {
            Function::Linebreak => 0,
            Function::Ruby | Function::Color => 2,
            _ => 1,
        };
        if arguments.len() != arity {
            return Err(self.error(
                expr,
                format!(
                    "text function expects {arity} arguments, got {}",
                    arguments.len()
                ),
            ));
        }
        if matches!(function, Function::Linebreak) {
            return Ok(Value::Content(Document::plain("\n")));
        }
        let mut body = match arguments.pop().expect("checked nonzero arity") {
            Value::Content(body) => body,
            Value::String(text) => Document::plain(text),
            Value::Empty => return Err(self.error(expr, "text function requires text or content")),
        };
        match function {
            Function::Text => {}
            Function::Strong => {
                for style in &mut body.styles {
                    style.bold = true;
                }
            }
            Function::Emph => {
                for style in &mut body.styles {
                    style.italic = true;
                }
            }
            Function::Strike => {
                for style in &mut body.styles {
                    style.strike = true;
                }
            }
            Function::Underline => {
                for style in &mut body.styles {
                    style.underline = true;
                }
            }
            Function::Ruby | Function::Color => {
                let Value::String(parameter) = arguments.pop().expect("checked two arguments")
                else {
                    return Err(self.error(expr, "ruby/color requires a string parameter"));
                };
                if matches!(function, Function::Ruby) {
                    if parameter.is_empty()
                        || parameter.contains(['\n', '\r'])
                        || body.text.is_empty()
                        || body.text.contains(['\n', '\r'])
                        || !body.ruby.is_empty()
                    {
                        return Err(self.error(expr, "ruby requires nonempty single-line reading and base; nested ruby is disabled"));
                    }
                    body.ruby.push(Ruby {
                        start: 0,
                        end: body.styles.len() as u32,
                        reading: parameter,
                    });
                } else {
                    let hex = parameter.strip_prefix('#').unwrap_or("");
                    if !matches!(hex.len(), 6 | 8) || !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
                        return Err(self.error(expr, "color requires #RRGGBB or #RRGGBBAA"));
                    }
                    let mut color = [255; 4];
                    for (i, channel) in color.iter_mut().enumerate().take(hex.len() / 2) {
                        *channel =
                            u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).expect("validated hex");
                    }
                    for style in &mut body.styles {
                        if style.color.is_none() {
                            style.color = Some(color);
                        }
                    }
                }
            }
            Function::Linebreak => unreachable!("returned above"),
        }
        Ok(Value::Content(body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ruby_and_nested_styles_use_display_indices() {
        let document = parse("Alice: #ruby(\"reader\")[*Bob*] #color(\"#ff000080\")[#strike[!]]")
            .expect("inline text");
        assert_eq!(document.text, "Alice: Bob !");
        assert_eq!(
            document.ruby,
            [Ruby {
                start: 7,
                end: 10,
                reading: "reader".into()
            }]
        );
        assert!(document.styles[7].bold);
        assert!(document.styles[11].strike);
        assert_eq!(document.styles[11].color, Some([255, 0, 0, 128]));
    }

    #[test]
    fn tilde_quotes_and_linebreak_constants_are_literal() {
        assert_eq!(
            parse("Alice~\"Bob\"#br#br~#linebreak()end")
                .expect("linebreaks")
                .text,
            "Alice~\"Bob\"\n\n~\nend"
        );
        assert_eq!(
            parse("#let br = linebreak();Alice#br Bob")
                .expect("text constant")
                .text,
            "Alice\n Bob"
        );
        assert_eq!(parse("a\nb").expect("VN newline").text, "a\nb");
    }

    #[test]
    fn constants_are_scoped_and_not_reparsed_as_code() {
        assert_eq!(
            parse("#let name = \"#include evil\";#name")
                .expect("literal content")
                .text,
            "#include evil"
        );
        assert!(parse("#name").is_err());
        assert!(parse("#let linebreak = \"x\";#linebreak").is_err());
    }

    #[test]
    fn document_elements_and_general_code_are_rejected() {
        for source in [
            "= Heading",
            "#image(\"x\")",
            "#eval(\"x\")",
            "#import \"x\"",
            "$x$",
            "#for x in (1, 2) [x]",
            "#let f(x) = x",
            "#ruby(\"\")[Bob]",
            "#ruby(\"x\")[#ruby(\"y\")[Bob]]",
        ] {
            assert!(parse(source).is_err(), "{source}");
        }
    }

    #[test]
    fn errors_report_original_utf8_byte_offsets() {
        let source = "漢字 #image(\"x\")";
        let error = parse(source).expect_err("disabled function");
        assert!(source.is_char_boundary(error.range.start));
        assert_eq!(&source[error.range], "image(\"x\")");
    }
}
