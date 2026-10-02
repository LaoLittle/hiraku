use std::{collections::BTreeSet, ops::Range};
use typst_library::foundations::{Binding, Scope, Value};
use typst_syntax::{
    Source,
    ast::{self, AstNode, Expr},
};

use crate::{
    Document, TextError, library,
    template::{Segment, Selector, TextSnapshot, TextValue, parse_selector, scalar, segments},
};

const MAX_BYTES: usize = 64 * 1024;
const MAX_DEPTH: usize = 64;
const MAX_NODES: usize = 16 * 1024;

pub fn parse(text: &str) -> Result<Document, TextError> {
    evaluate(text, None)
}

fn source(text: &str) -> Result<Source, TextError> {
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
    Ok(source)
}

pub fn parse_with_snapshot(text: &str, snapshot: &TextSnapshot) -> Result<Document, TextError> {
    evaluate(text, Some(snapshot))
}

fn evaluate(text: &str, snapshot: Option<&TextSnapshot>) -> Result<Document, TextError> {
    let source = source(text)?;
    let mut prefix = "__hiraku_data_".to_owned();
    while text.contains(&prefix) {
        prefix.push('_');
    }
    let mut lowerer = Lowerer {
        source: &source,
        snapshot,
        scope: library::SCOPE.clone(),
        constants: BTreeSet::from(["br".to_owned()]),
        nodes: 0,
        bindings: 0,
        prefix,
    };
    let (code, mappings) = lowerer.markup(source.root().cast().expect("markup"), 0)?;
    crate::runtime::evaluate(code, lowerer.scope, &mappings)
}

struct Lowerer<'a> {
    source: &'a Source,
    snapshot: Option<&'a TextSnapshot>,
    scope: Scope,
    constants: BTreeSet<String>,
    nodes: usize,
    bindings: usize,
    prefix: String,
}

pub(crate) fn selectors(text: &str) -> Result<Vec<Selector>, TextError> {
    let source = source(text)?;
    let mut output = Vec::new();
    let mut nodes = vec![(source.root(), 0)];
    let mut visited = 0;
    while let Some((node, depth)) = nodes.pop() {
        visited += 1;
        if depth > MAX_DEPTH || visited > MAX_NODES || output.len() > MAX_NODES {
            return Err(TextError::new(
                "text selector limit exceeded",
                0..text.len(),
            ));
        }
        let range = source
            .find(node.span())
            .map(|node| node.range())
            .unwrap_or(0..0);
        match node.kind() {
            typst_syntax::SyntaxKind::Str => {
                for segment in string_segments(&text[range.clone()], range.start)? {
                    if let Segment::Selector(selector) = segment {
                        output.push(selector);
                    }
                }
            }
            typst_syntax::SyntaxKind::Markup => {
                for (expr, range) in markup_exprs(&source, node.cast().expect("markup"))? {
                    if matches!(expr, Expr::Text(_)) {
                        for segment in segments(&text[range.clone()], range.start)? {
                            if let Segment::Selector(selector) = segment {
                                output.push(selector);
                            }
                        }
                    } else {
                        nodes.push((expr.to_untyped(), depth + 1));
                    }
                }
            }
            typst_syntax::SyntaxKind::CodeBlock => {
                let block = &text[range.clone()];
                output.push(parse_selector(&block[1..block.len() - 1], range)?);
            }
            typst_syntax::SyntaxKind::Text => {
                for segment in segments(node.leaf_text().as_str(), range.start)? {
                    if let Segment::Selector(selector) = segment {
                        output.push(selector);
                    }
                }
            }
            _ => nodes.extend(node.children().rev().map(|node| (node, depth + 1))),
        }
    }
    Ok(output)
}

fn markup_exprs<'a>(
    source: &Source,
    markup: ast::Markup<'a>,
) -> Result<Vec<(Expr<'a>, std::ops::Range<usize>)>, TextError> {
    let mut items = markup.exprs();
    let mut output = Vec::new();
    while let Some(expr) = items.next() {
        let mut range = source
            .find(expr.span())
            .map(|node| node.range())
            .unwrap_or(0..0);
        if matches!(expr, Expr::Text(_)) {
            loop {
                match segments(&source.text()[range.clone()], range.start) {
                    Err(error) if error.message == "unclosed text selector" => {
                        let next = items.next().ok_or_else(|| error.clone())?;
                        if !matches!(
                            next,
                            Expr::Text(_)
                                | Expr::Space(_)
                                | Expr::SmartQuote(_)
                                | Expr::Shorthand(_)
                        ) {
                            return Err(error);
                        }
                        range.end = source
                            .find(next.span())
                            .map(|node| node.range().end)
                            .unwrap_or(range.end);
                    }
                    Err(error) => return Err(error),
                    Ok(_) => break,
                }
            }
        }
        output.push((expr, range));
    }
    Ok(output)
}

fn string_segments(token: &str, base: usize) -> Result<Vec<Segment>, TextError> {
    let source = &token[1..token.len() - 1];
    let mut output = Vec::new();
    let mut offset = 0;
    let mut start = 0;
    let literal = |part: &str| -> Result<Segment, TextError> {
        let code = typst_syntax::parse_code(&format!(
            "\"{}\"",
            part.replace("{{", "{").replace("}}", "}")
        ));
        let value = code
            .children()
            .find_map(|node| node.cast::<ast::Str>())
            .ok_or_else(|| TextError::new("invalid text string", base..base + token.len()))?;
        Ok(Segment::Markup(value.get().to_string()))
    };
    while offset < source.len() {
        let rest = &source[offset..];
        if rest.starts_with("\\u{") {
            offset += rest.find('}').map(|end| end + 1).unwrap_or(rest.len());
        } else if rest.starts_with('\\') {
            offset += 1;
            if let Some(ch) = source[offset..].chars().next() {
                offset += ch.len_utf8();
            }
        } else if rest.starts_with("{{") || rest.starts_with("}}") {
            offset += 2;
        } else if rest.starts_with('{') {
            if start < offset {
                output.push(literal(&source[start..offset])?);
            }
            let end = rest.find('}').ok_or_else(|| {
                TextError::new(
                    "unclosed text selector",
                    base + 1 + offset..base + token.len() - 1,
                )
            })?;
            output.push(Segment::Selector(parse_selector(
                &rest[1..end],
                base + 1 + offset..base + 1 + offset + end + 1,
            )?));
            offset += end + 1;
            start = offset;
        } else {
            offset += rest.chars().next().expect("UTF-8 suffix").len_utf8();
        }
    }
    if start < source.len() {
        output.push(literal(&source[start..])?);
    }
    Ok(output)
}

impl Lowerer<'_> {
    fn range(&self, expr: Expr<'_>) -> Range<usize> {
        self.source
            .find(expr.span())
            .map(|node| node.range())
            .unwrap_or(0..0)
    }

    fn error(&self, expr: Expr<'_>, message: impl Into<String>) -> TextError {
        TextError::new(message, self.range(expr))
    }

    fn bind(&mut self, value: Value) -> String {
        let name = format!("{}{}", self.prefix, self.bindings);
        self.bindings += 1;
        self.scope
            .bind(name.clone().into(), Binding::detached(value));
        name
    }

    fn scalar(&mut self, value: &TextValue, range: Range<usize>) -> Result<String, TextError> {
        if let TextValue::String(value) = value
            && value.len() > MAX_BYTES
        {
            return Err(TextError::new(
                "text selector exceeds the 64 KiB limit",
                range,
            ));
        }
        let native = match value {
            TextValue::Int(value) => Value::Int(*value),
            TextValue::Bool(value) => Value::Bool(*value),
            TextValue::Float(value) if value.is_finite() => Value::Float((*value).into()),
            TextValue::String(value) => Value::Str(value.as_str().into()),
            TextValue::UInt(value) => match i64::try_from(*value) {
                Ok(value) => Value::Int(value),
                Err(_) => Value::Str(value.to_string().into()),
            },
            _ => {
                return Err(TextError::new(
                    "text selector must resolve to a finite scalar value",
                    range,
                ));
            }
        };
        Ok(self.bind(native))
    }

    fn fragments(&mut self, segments: Vec<Segment>) -> Result<String, TextError> {
        let mut output = String::new();
        for segment in segments {
            let value = match segment {
                Segment::Markup(text) => self.bind(Value::Str(text.into())),
                Segment::Selector(selector) => {
                    let snapshot = self.snapshot.ok_or_else(|| {
                        TextError::new(
                            "text selectors require a data snapshot",
                            selector.range.clone(),
                        )
                    })?;
                    self.scalar(snapshot.resolve(&selector)?, selector.range.clone())?
                }
            };
            output.push_str(&format!("#text({value});"));
        }
        Ok(output)
    }

    fn markup(
        &mut self,
        markup: ast::Markup<'_>,
        depth: usize,
    ) -> Result<(String, Vec<(Range<usize>, Range<usize>)>), TextError> {
        let saved = self.constants.clone();
        let mut output = String::new();
        let mut mappings = Vec::new();
        for (expr, range) in markup_exprs(self.source, markup)? {
            let start = output.len();
            let code = if matches!(expr, Expr::Text(_)) {
                self.check(expr, depth)?;
                let text = &self.source.text()[range.clone()];
                if self.snapshot.is_some() {
                    self.fragments(segments(text, range.start)?)?
                } else {
                    let value = self.bind(Value::Str(text.into()));
                    format!("#text({value});")
                }
            } else {
                let code = self.expr(expr, depth + 1)?;
                if matches!(expr, Expr::LetBinding(_)) {
                    format!("#{code};")
                } else {
                    format!("#text({code});")
                }
            };
            output.push_str(&code);
            mappings.push((start..output.len(), range));
        }
        self.constants = saved;
        Ok((output, mappings))
    }

    fn check(&mut self, expr: Expr<'_>, depth: usize) -> Result<(), TextError> {
        self.nodes += 1;
        if depth > MAX_DEPTH || self.nodes > MAX_NODES {
            return Err(self.error(expr, "rich text evaluation limit exceeded"));
        }
        Ok(())
    }

    fn expr(&mut self, expr: Expr<'_>, depth: usize) -> Result<String, TextError> {
        self.check(expr, depth)?;
        Ok(match expr {
            Expr::Text(value) => self.bind(Value::Str(value.get().as_str().into())),
            Expr::Space(value) => {
                self.bind(Value::Str(value.to_untyped().leaf_text().as_str().into()))
            }
            Expr::Shorthand(value) => {
                self.bind(Value::Str(value.to_untyped().leaf_text().as_str().into()))
            }
            Expr::SmartQuote(value) => {
                self.bind(Value::Str(value.to_untyped().leaf_text().as_str().into()))
            }
            Expr::Parbreak(value) => {
                self.bind(Value::Str(value.to_untyped().leaf_text().as_str().into()))
            }
            Expr::ListItem(value)
                if value.body().exprs().all(|expr| matches!(expr, Expr::Space(_))) =>
            {
                let range = self.range(expr);
                self.bind(Value::Str(self.source.text()[range].into()))
            }
            Expr::Linebreak(_) => "linebreak()".into(),
            Expr::Escape(value) => self.bind(Value::Str(value.get().to_string().into())),
            Expr::Str(value) => {
                let mut text = String::new();
                if let Some(snapshot) = self.snapshot {
                    let range = self.range(expr);
                    for segment in string_segments(&self.source.text()[range.clone()], range.start)?
                    {
                        text.push_str(&match segment {
                            Segment::Markup(text) => text,
                            Segment::Selector(selector) => {
                                scalar(snapshot.resolve(&selector)?, selector.range.clone())?
                            }
                        });
                    }
                } else {
                    text = value.get().to_string();
                }
                self.bind(Value::Str(text.into()))
            }
            Expr::Int(value) => self.bind(Value::Int(value.get())),
            Expr::Float(value) => self.bind(Value::Float(value.get().into())),
            Expr::Bool(value) => self.bind(Value::Bool(value.get())),
            Expr::CodeBlock(_) => {
                let range = self.range(expr);
                let text = &self.source.text()[range.clone()];
                let selector = parse_selector(&text[1..text.len() - 1], range)?;
                let snapshot = self
                    .snapshot
                    .ok_or_else(|| self.error(expr, "text selectors require a data snapshot"))?;
                self.scalar(snapshot.resolve(&selector)?, selector.range.clone())?
            }
            Expr::ContentBlock(value) => format!("[{}]", self.markup(value.body(), depth + 1)?.0),
            Expr::Strong(value) => format!("strong[{}]", self.markup(value.body(), depth + 1)?.0),
            Expr::Emph(value) => format!("emph[{}]", self.markup(value.body(), depth + 1)?.0),
            Expr::Ident(value) => {
                let name = value.get();
                if !self.constants.contains(name.as_str()) {
                    return Err(self.error(expr, format!("unknown text constant `{name}`")));
                }
                name.to_string()
            }
            Expr::LetBinding(value) => {
                let ast::LetBindingKind::Normal(ast::Pattern::Normal(Expr::Ident(name))) =
                    value.kind()
                else {
                    return Err(self.error(expr, "only simple text constants are allowed; functions and destructuring are disabled"));
                };
                let name = name.get().to_string();
                if library::contains(&name) && name != "br" {
                    return Err(self.error(expr, "text library functions cannot be shadowed"));
                }
                let init = value
                    .init()
                    .ok_or_else(|| self.error(expr, "text constants require an initializer"))?;
                let init = self.expr(init, depth + 1)?;
                self.constants.insert(name.clone());
                format!("let {name} = {init}")
            }
            Expr::FuncCall(call) => {
                let Expr::Ident(name) = call.callee() else {
                    return Err(
                        self.error(expr, "only calls to the Hiraku text library are allowed")
                    );
                };
                let name = name.get();
                if !library::contains(name.as_str()) || name == "br" {
                    return Err(self.error(expr, format!("unknown Hiraku text function `{name}`; the Typst standard library is disabled")));
                }
                let mut args = Vec::new();
                for arg in call.args().items() {
                    let ast::Arg::Pos(arg) = arg else {
                        return Err(
                            self.error(expr, "named and spread text arguments are disabled")
                        );
                    };
                    args.push(self.expr(arg, depth + 1)?);
                }
                format!("{name}({})", args.join(","))
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
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Ruby;

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
    fn timetable_text_preserves_ranges_and_multiple_linebreaks() {
        let source = "6:00～10:00#br;Morning#br;#br;10:00～12:00#br;Activities#br;(Schedule)#br;#br;22:00～6:00#br;Night";
        let document = parse(source).expect("plain timetable text");
        assert!(document.text.contains("6:00～10:00"));
        assert!(document.text.contains("Morning"));
        assert!(document.text.contains("Night"));
    }

    #[test]
    fn tilde_quotes_and_linebreak_constants_are_literal() {
        assert_eq!(
            parse("Alice#br;Bob").expect("linebreak delimiter").text,
            "Alice\nBob"
        );
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
    fn empty_list_markers_are_literal_ui_labels() {
        for source in ["-", "- ", "-\n-", "#text(\"-\")"] {
            let expected = if source.starts_with('#') { "-" } else { source };
            assert_eq!(parse(source).expect("literal label").text, expected);
            assert_eq!(
                parse_with_snapshot(source, &TextSnapshot::default())
                    .expect("literal template label").text,
                expected,
            );
        }
        assert!(parse("- Alice").is_err());
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
