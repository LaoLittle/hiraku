//! Domain-independent, source-aware diagnostics for Hiraku tools.

use std::{collections::BTreeMap, fmt, io, ops::Range};

use ariadne::{Color, Config, IndexType, Label, Report, ReportKind, sources};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceId(String);

impl SourceId {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SourceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Advice,
}

impl Severity {
    fn name(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Advice => "advice",
        }
    }

    fn color(self) -> Color {
        match self {
            Self::Error => Color::Red,
            Self::Warning => Color::Yellow,
            Self::Advice => Color::Fixed(147),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticLabel {
    pub source: SourceId,
    pub span: Range<usize>,
    pub message: Option<String>,
    pub primary: bool,
}

impl DiagnosticLabel {
    pub fn primary(source: SourceId, span: Range<usize>) -> Self {
        Self {
            source,
            span,
            message: None,
            primary: true,
        }
    }

    pub fn secondary(source: SourceId, span: Range<usize>) -> Self {
        Self {
            source,
            span,
            message: None,
            primary: false,
        }
    }

    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: Option<String>,
    pub message: String,
    pub labels: Vec<DiagnosticLabel>,
    pub notes: Vec<String>,
    pub help: Vec<String>,
}

impl Diagnostic {
    pub fn new(severity: Severity, message: impl Into<String>) -> Self {
        Self {
            severity,
            code: None,
            message: message.into(),
            labels: Vec::new(),
            notes: Vec::new(),
            help: Vec::new(),
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self::new(Severity::Error, message)
    }

    pub fn warning(message: impl Into<String>) -> Self {
        Self::new(Severity::Warning, message)
    }

    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    pub fn with_label(mut self, label: DiagnosticLabel) -> Self {
        self.labels.push(label);
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help.push(help.into());
        self
    }
}

#[derive(Clone, Debug, Default)]
pub struct SourceMap {
    sources: BTreeMap<SourceId, String>,
}

impl SourceMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, name: impl Into<String>, source: impl Into<String>) -> SourceId {
        let id = SourceId::new(name);
        self.sources.insert(id.clone(), source.into());
        id
    }

    pub fn get(&self, id: &SourceId) -> Option<&str> {
        self.sources.get(id).map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderOptions {
    pub color: bool,
    pub compact: bool,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self::plain()
    }
}

impl RenderOptions {
    pub const fn plain() -> Self {
        Self {
            color: false,
            compact: false,
        }
    }

    /// Selects colored output only for an interactive native terminal.
    ///
    /// Tests, redirected logs, `NO_COLOR`, and WebAssembly remain plain.
    pub fn terminal() -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        {
            use std::io::IsTerminal as _;

            let force_color = std::env::var_os("CLICOLOR_FORCE").is_some_and(|value| value != "0");
            Self {
                color: force_color
                    || (std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none()),
                compact: false,
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            Self::plain()
        }
    }
}

pub fn render_diagnostics(
    diagnostics: &[Diagnostic],
    source_map: &SourceMap,
    options: RenderOptions,
) -> String {
    let mut output = Vec::new();
    write_diagnostics(diagnostics, source_map, &mut output, options)
        .expect("writing diagnostics to a byte buffer cannot fail");
    String::from_utf8(output).expect("Ariadne diagnostics are valid UTF-8")
}

/// Writes an already rendered diagnostic directly to standard error.
///
/// Multi-line compiler diagnostics should use this instead of passing ANSI
/// output through a structured logger, which may escape control characters.
pub fn emit_rendered_diagnostic(context: &str, diagnostic: &str) -> io::Result<()> {
    let stderr = io::stderr();
    let mut stderr = stderr.lock();
    write_rendered_diagnostic(&mut stderr, context, diagnostic)
}

pub fn write_rendered_diagnostic(
    mut writer: impl io::Write,
    context: &str,
    diagnostic: &str,
) -> io::Result<()> {
    writeln!(writer, "{context}")?;
    write!(writer, "{diagnostic}")?;
    if !diagnostic.ends_with('\n') {
        writeln!(writer)?;
    }
    Ok(())
}

pub fn write_diagnostics(
    diagnostics: &[Diagnostic],
    source_map: &SourceMap,
    mut writer: impl io::Write,
    options: RenderOptions,
) -> io::Result<()> {
    for (index, diagnostic) in diagnostics.iter().enumerate() {
        if index > 0 {
            writeln!(writer)?;
        }
        write_diagnostic(diagnostic, source_map, &mut writer, options)?;
    }
    Ok(())
}

fn write_diagnostic(
    diagnostic: &Diagnostic,
    source_map: &SourceMap,
    writer: &mut impl io::Write,
    options: RenderOptions,
) -> io::Result<()> {
    let Some(primary) = diagnostic
        .labels
        .iter()
        .find(|label| label.primary)
        .or_else(|| diagnostic.labels.first())
    else {
        writeln!(
            writer,
            "{}: {}",
            diagnostic.severity.name(),
            diagnostic.message
        )?;
        for note in &diagnostic.notes {
            writeln!(writer, "note: {note}")?;
        }
        for help in &diagnostic.help {
            writeln!(writer, "help: {help}")?;
        }
        return Ok(());
    };

    let primary_span = normalized_span(primary, source_map);
    let kind = match diagnostic.severity {
        Severity::Error => ReportKind::Error,
        Severity::Warning => ReportKind::Warning,
        Severity::Advice => ReportKind::Advice,
    };
    let mut builder = Report::build(kind, (primary.source.clone(), primary_span))
        .with_message(&diagnostic.message)
        .with_config(
            Config::new()
                .with_color(options.color)
                .with_compact(options.compact)
                .with_index_type(IndexType::Byte),
        );
    if let Some(code) = &diagnostic.code {
        builder = builder.with_code(code);
    }
    for (index, label) in diagnostic.labels.iter().enumerate() {
        let mut rendered = Label::new((label.source.clone(), normalized_span(label, source_map)))
            .with_color(if label.primary {
                diagnostic.severity.color()
            } else {
                Color::Cyan
            })
            .with_order(index as i32);
        if let Some(message) = &label.message {
            rendered = rendered.with_message(message);
        }
        builder = builder.with_label(rendered);
    }
    for note in &diagnostic.notes {
        builder = builder.with_note(note);
    }
    for help in &diagnostic.help {
        builder = builder.with_help(help);
    }
    let cache = sources(
        source_map
            .sources
            .iter()
            .map(|(id, source)| (id.clone(), source.clone())),
    );
    builder.finish().write(cache, writer)
}

fn normalized_span(label: &DiagnosticLabel, source_map: &SourceMap) -> Range<usize> {
    let source_len = source_map.get(&label.source).map_or(0, str::len);
    let start = label.span.start.min(source_len);
    let mut end = label.span.end.max(start).min(source_len);
    if end == start && start < source_len {
        end += source_map
            .get(&label.source)
            .and_then(|source| source[start..].chars().next())
            .map_or(1, char::len_utf8);
    }
    start..end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_unicode_source_using_byte_spans() {
        let mut sources = SourceMap::new();
        let source = "let café = 1\nwhile café {\n}\n";
        let id = sources.insert("scripts/loop.hks", source);
        let start = source.find("café {").expect("test source contains binding");
        let diagnostic = Diagnostic::error("condition expects Bool, got Int")
            .with_code("HKS-COMPILE")
            .with_label(
                DiagnosticLabel::primary(id, start..start + "café".len())
                    .with_message("this expression has type Int"),
            )
            .with_help("compare the value to produce a Bool");
        let rendered = render_diagnostics(&[diagnostic], &sources, RenderOptions::default());
        assert!(rendered.contains("[HKS-COMPILE]"));
        assert!(rendered.contains("scripts/loop.hks:2:7"));
        assert!(rendered.contains("this expression has type Int"));
        assert!(rendered.contains("Help: compare the value to produce a Bool"));
    }

    #[test]
    fn plain_rendering_does_not_emit_ansi_sequences() {
        let mut sources = SourceMap::new();
        let id = sources.insert("sample.hks", "invalid");
        let diagnostic =
            Diagnostic::error("invalid expression").with_label(DiagnosticLabel::primary(id, 0..7));
        let rendered = render_diagnostics(&[diagnostic], &sources, RenderOptions::plain());
        assert!(!rendered.contains("\u{1b}["));
    }

    #[test]
    fn direct_diagnostic_output_preserves_ansi_sequences() {
        let mut output = Vec::new();
        write_rendered_diagnostic(
            &mut output,
            "failed to compile script:",
            "\u{1b}[31merror\u{1b}[0m",
        )
        .expect("writing a diagnostic to memory succeeds");

        let output = String::from_utf8(output).expect("diagnostic output is UTF-8");
        assert_eq!(
            output,
            "failed to compile script:\n\u{1b}[31merror\u{1b}[0m\n"
        );
    }
}
