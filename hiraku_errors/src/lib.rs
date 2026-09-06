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
    if diagnostic.labels.len() == 1 {
        return write_source_excerpt(diagnostic, source_map, writer, options);
    }
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
        // Ariadne has no context-line setting. Message-free, uncolored anchors
        // include adjacent lines without extending the primary highlight.
        if let Some(source) = source_map.get(&label.source) {
            let span = normalized_span(label, source_map);
            let starts = std::iter::once(0)
                .chain(
                    source
                        .bytes()
                        .enumerate()
                        .filter_map(|(index, byte)| (byte == b'\n').then_some(index + 1)),
                )
                .collect::<Vec<_>>();
            let first = starts
                .partition_point(|start| *start <= span.start)
                .saturating_sub(1);
            let last = starts
                .partition_point(|start| *start <= span.end.saturating_sub(1).max(span.start))
                .saturating_sub(1);
            for line in [first.checked_sub(1), last.checked_add(1)]
                .into_iter()
                .flatten()
            {
                if let Some(offset) = starts.get(line).copied() {
                    let end = offset + source[offset..].chars().next().map_or(0, char::len_utf8);
                    builder = builder
                        .with_label(Label::new((label.source.clone(), offset..end)).with_order(-1));
                }
            }
        }
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

/// Single-location diagnostics use one continuous excerpt. Context lines are
/// presentation, not labels: feeding them to Ariadne creates separate groups.
fn write_source_excerpt(
    diagnostic: &Diagnostic,
    source_map: &SourceMap,
    writer: &mut impl io::Write,
    options: RenderOptions,
) -> io::Result<()> {
    let label = &diagnostic.labels[0];
    let source = source_map.get(&label.source).unwrap_or("");
    let span = normalized_span(label, source_map);
    let mut start = span.start;
    while !source.is_char_boundary(start) {
        start -= 1;
    }
    let lines = source.split('\n').collect::<Vec<_>>();
    let first = source[..start]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count();
    let end = span.end.saturating_sub(1).max(start);
    let last = source.as_bytes()[..end]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count();
    let column = source[..start]
        .rsplit('\n')
        .next()
        .unwrap_or("")
        .chars()
        .count()
        + 1;
    let from = first.saturating_sub(1);
    let to = (last + 1).min(lines.len() - 1);
    let width = (to + 1).to_string().len();
    let (color, reset) = if options.color {
        (
            match diagnostic.severity {
                Severity::Error => "\x1b[31m",
                Severity::Warning => "\x1b[33m",
                Severity::Advice => "\x1b[36m",
            },
            "\x1b[0m",
        )
    } else {
        ("", "")
    };
    if let Some(code) = &diagnostic.code {
        write!(writer, "{color}[{code}] ")?;
    }
    writeln!(
        writer,
        "{color}{}:{reset} {}",
        match diagnostic.severity {
            Severity::Error => "Error",
            Severity::Warning => "Warning",
            Severity::Advice => "Advice",
        },
        diagnostic.message
    )?;
    writeln!(
        writer,
        "{:width$} ╭─[ {}:{}:{} ]",
        "",
        label.source,
        first + 1,
        column
    )?;
    for (index, line) in lines.iter().enumerate().take(to + 1).skip(from) {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if (first..=last).contains(&index) {
            writeln!(writer, "{color}{:width$} │ {line}{reset}", index + 1)?;
        } else {
            writeln!(writer, "{:width$} │ {line}", index + 1)?;
        }
    }
    writeln!(writer, "{:width$} ╰─", "")?;
    if let Some(message) = &label.message {
        writeln!(writer, "  = {message}")?;
    }
    for note in &diagnostic.notes {
        writeln!(writer, "Note: {note}")?;
    }
    for help in &diagnostic.help {
        writeln!(writer, "Help: {help}")?;
    }
    Ok(())
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
    fn reports_include_one_context_line_on_each_side() {
        let mut sources = SourceMap::new();
        let source = "outer before\nprevious line\nbroken\nfollowing line\nouter after";
        let id = sources.insert("entry.hks", source);
        let start = source.find("broken").expect("marker exists");
        let diagnostic =
            Diagnostic::error("failure").with_label(DiagnosticLabel::primary(id, start..start + 6));
        let rendered = render_diagnostics(&[diagnostic], &sources, RenderOptions::plain());
        assert!(rendered.contains("previous line"), "{rendered}");
        assert!(rendered.contains("following line"), "{rendered}");
        assert!(!rendered.contains("outer before"), "{rendered}");
        assert!(!rendered.contains("outer after"), "{rendered}");
        assert_eq!(rendered.matches("╭─[").count(), 1, "one continuous excerpt");
        assert!(
            !rendered.contains("├─["),
            "context must not form another group"
        );
        assert!(
            rendered.contains("2 │ previous line\n3 │ broken\n4 │ following line"),
            "{rendered}"
        );
    }

    #[test]
    fn only_the_error_source_line_is_red() {
        let mut sources = SourceMap::new();
        let source = "before\nbroken\nafter";
        let id = sources.insert("entry.hks", source);
        let diagnostic =
            Diagnostic::error("failure").with_label(DiagnosticLabel::primary(id, 7..13));
        let rendered = render_diagnostics(
            &[diagnostic],
            &sources,
            RenderOptions {
                color: true,
                compact: false,
            },
        );
        assert!(
            rendered.contains("1 │ before\n\x1b[31m2 │ broken\x1b[0m\n3 │ after"),
            "{rendered:?}"
        );
    }

    #[test]
    fn context_handles_unicode_and_file_boundaries() {
        let mut sources = SourceMap::new();
        let source = "élise\nbroken\nbob";
        let id = sources.insert("data.hson", source);
        for (text, adjacent) in [("élise", "broken"), ("broken", "élise"), ("bob", "broken")] {
            let start = source.find(text).expect("fixture text exists");
            let diagnostic = Diagnostic::error("failure").with_label(DiagnosticLabel::primary(
                id.clone(),
                start..start + text.len(),
            ));
            let report = render_diagnostics(&[diagnostic], &sources, RenderOptions::plain());
            assert!(report.contains(text), "{report}");
            assert!(report.contains(adjacent), "{report}");
        }
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
