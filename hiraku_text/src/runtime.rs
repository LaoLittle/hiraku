use std::{ops::Range, sync::LazyLock};

use comemo::Track;
use typst_library::{
    Library, LibraryBuilder, World,
    diag::{FileError, FileResult, bail},
    engine::{Route, Sink, Traced},
    foundations::{
        Binding, Bytes, Content, Datetime, Duration, Module, NativeRuleMap, Scope, SequenceElem,
    },
    model::{EmphElem, StrongElem},
    routines::Routines,
    text::{Font, FontBook, LinebreakElem, StrikeElem, TextElem, UnderlineElem},
};
use typst_syntax::{DiagSpanKind, FileId, Source, Span};
use typst_utils::LazyHash;

use crate::{
    Document, Ruby, TextError,
    library::{ColorElem, RubyElem},
};

static ROUTINES: Routines = Routines {
    rules: NativeRuleMap::new,
    eval_string: typst_eval::eval_string,
    eval_closure: typst_eval::eval_closure,
    realize: |_, _, _, _, _, _| bail!(Span::detached(), "VN text does not support Typst layout"),
    layout_frame: |_, _, _, _, _| bail!(Span::detached(), "VN text does not support Typst layout"),
    html_module: || Module::anonymous(Scope::new()),
    html_mathml_body: |_, _| None,
    html_span_filled: |content, _| content,
};

static LIBRARY: LazyLock<Library> = LazyLock::new(|| {
    let mut library = LibraryBuilder::from_routines(&ROUTINES).build();
    library.global = Module::anonymous(crate::library::SCOPE.clone());
    library.math = Module::anonymous(Scope::new());
    library.std = Binding::detached(Module::anonymous(Scope::new()));
    library
});

struct InlineWorld {
    library: LazyHash<Library>,
    book: LazyHash<FontBook>,
    source: Source,
}

impl World for InlineWorld {
    fn library(&self) -> &LazyHash<Library> {
        &self.library
    }
    fn book(&self) -> &LazyHash<FontBook> {
        &self.book
    }
    fn main(&self) -> FileId {
        self.source.id()
    }
    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.source.id() {
            Ok(self.source.clone())
        } else {
            Err(FileError::AccessDenied)
        }
    }
    fn file(&self, _id: FileId) -> FileResult<Bytes> {
        Err(FileError::AccessDenied)
    }
    fn font(&self, _index: usize) -> Option<Font> {
        None
    }
    fn today(&self, _offset: Option<Duration>) -> Option<Datetime> {
        None
    }
}

pub(crate) fn evaluate(
    code: String,
    scope: Scope,
    mappings: &[(Range<usize>, Range<usize>)],
) -> Result<Document, TextError> {
    let mut library = LIBRARY.clone();
    library.global = Module::anonymous(scope);
    let world = InlineWorld {
        library: LazyHash::new(library),
        book: LazyHash::new(FontBook::new()),
        source: Source::detached(code),
    };
    let traced = Traced::default();
    let route = Route::default();
    let mut sink = Sink::new();
    let world_ref: &dyn World = &world;
    let result = typst_eval::eval(
        world_ref.track(),
        &world.library,
        traced.track(),
        sink.track_mut(),
        route.track(),
        &world.source,
    );
    comemo::evict(1);
    let module = result.map_err(|errors| {
        let error = &errors[0];
        let range = match error.span.get() {
            DiagSpanKind::Number { num, sub_range, .. } => world.source.range(num, sub_range),
            _ => None,
        }
        .unwrap_or(0..0);
        TextError::new(error.message.to_string(), original_range(range, mappings))
    })?;
    document(&module.content(), 0).map_err(|mut error| {
        error.range = original_range(error.range, mappings);
        error
    })
}

fn original_range(range: Range<usize>, mappings: &[(Range<usize>, Range<usize>)]) -> Range<usize> {
    mappings
        .iter()
        .find(|(generated, _)| generated.start <= range.start && range.start < generated.end)
        .map(|(_, original)| original.clone())
        .unwrap_or(0..0)
}

fn document(content: &Content, depth: usize) -> Result<Document, TextError> {
    if depth > 64 {
        return Err(TextError::new("rich text nesting limit exceeded", 0..0));
    }
    if content.is_empty() {
        return Ok(Document::default());
    }
    if let Some(sequence) = content.to_packed::<SequenceElem>() {
        let mut result = Document::default();
        for child in &sequence.children {
            result.append(&document(child, depth + 1)?);
            if result.text.len() > 64 * 1024 {
                return Err(TextError::new(
                    "expanded rich text exceeds the 64 KiB limit",
                    0..0,
                ));
            }
        }
        return Ok(result);
    }
    if let Some(text) = content.to_packed::<TextElem>() {
        return Ok(Document::plain(text.text.to_string()));
    }
    if content.to_packed::<LinebreakElem>().is_some() {
        return Ok(Document::plain("\n"));
    }
    let (body, style) = if let Some(elem) = content.to_packed::<StrongElem>() {
        (&elem.body, 0)
    } else if let Some(elem) = content.to_packed::<EmphElem>() {
        (&elem.body, 1)
    } else if let Some(elem) = content.to_packed::<StrikeElem>() {
        (&elem.body, 2)
    } else if let Some(elem) = content.to_packed::<UnderlineElem>() {
        (&elem.body, 3)
    } else if let Some(elem) = content.to_packed::<RubyElem>() {
        let mut result = document(&elem.body, depth + 1)?;
        if result.text.is_empty() || result.text.contains(['\n', '\r']) || !result.ruby.is_empty() {
            return Err(TextError::new(
                "ruby requires nonempty single-line base; nested ruby is disabled",
                0..0,
            ));
        }
        result.ruby.push(Ruby {
            start: 0,
            end: result.styles.len() as u32,
            reading: elem.reading.to_string(),
        });
        return Ok(result);
    } else if let Some(elem) = content.to_packed::<ColorElem>() {
        let mut result = document(&elem.body, depth + 1)?;
        let hex = &elem.rgba[1..];
        let mut color = [255; 4];
        for (i, channel) in color.iter_mut().enumerate().take(hex.len() / 2) {
            *channel = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
                .map_err(|_| TextError::new("invalid color", 0..0))?;
        }
        for style in &mut result.styles {
            if style.color.is_none() {
                style.color = Some(color);
            }
        }
        return Ok(result);
    } else {
        return Err(TextError::new("unsupported Typst content in VN text", 0..0));
    };
    let mut result = document(body, depth + 1)?;
    for value in &mut result.styles {
        match style {
            0 => value.bold = true,
            1 => value.italic = true,
            2 => value.strike = true,
            _ => value.underline = true,
        }
    }
    Ok(result)
}
