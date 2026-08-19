use std::{collections::BTreeMap, fmt};

use hiraku_script::hks::{Block, Expr, ExprKind, Program, Span, Stmt, parse_program};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoiceLine {
    /// Stable content-derived identifier used to join story, CSV and HSON rows.
    pub id: String,
    pub speaker: String,
    pub text: String,
    pub path: String,
    pub statement_offset: usize,
    pub inserted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoiceScaffold {
    pub source: String,
    pub lines: Vec<VoiceLine>,
}

/// Editable interchange row used by the future CSV import/export commands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoiceCsvRow {
    pub id: String,
    pub character: String,
    pub text: String,
    /// Current path found in or generated for the story.
    pub path: String,
    /// Developer-editable replacement for `path`. Empty means omit the voice.
    pub rename: String,
    pub file: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CharacterVoiceManifest {
    pub char: String,
    pub voices: Vec<VoiceAsset>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct VoiceAsset {
    pub name: String,
    pub file: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VoiceScaffoldError {
    Parse(String),
}

impl fmt::Display for VoiceScaffoldError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(message) => write!(formatter, "failed to parse HKS story: {message}"),
        }
    }
}

impl std::error::Error for VoiceScaffoldError {}

impl VoiceLine {
    pub fn csv_row(&self) -> VoiceCsvRow {
        let character = if self.speaker.is_empty() {
            "narrator".to_string()
        } else {
            self.speaker.clone()
        };
        VoiceCsvRow {
            id: self.id.clone(),
            character,
            text: self.text.clone(),
            path: self.path.clone(),
            rename: self.path.clone(),
            file: format!("{}.ogg", self.id),
        }
    }
}

/// Exports the edited rows for one character using the engine-facing HSON
/// manifest convention. Rows with an empty `rename` or `file` are omitted.
pub fn export_character_hson(
    character: &str,
    rows: &[VoiceCsvRow],
) -> Result<String, hiraku_script::hson::HsonError> {
    let manifest = CharacterVoiceManifest {
        char: character.to_string(),
        voices: rows
            .iter()
            .filter(|row| row.character == character)
            .filter(|row| !row.rename.trim().is_empty() && !row.file.trim().is_empty())
            .map(|row| VoiceAsset {
                name: row.rename.clone(),
                file: row.file.clone(),
            })
            .collect(),
    };
    hiraku_script::hson::to_string(&manifest)
}

/// Adds a `voice("voice/<stable-id>")` statement before every unvoiced
/// `say`/`narrate` statement and returns the rows needed by a later CSV export.
///
/// This transformation intentionally depends only on HKS syntax. Engine playback
/// policy and asset loading do not belong in the CLI.
pub fn scaffold_story(source: &str) -> Result<VoiceScaffold, VoiceScaffoldError> {
    let program = parse_program(source).map_err(|errors| {
        VoiceScaffoldError::Parse(
            errors
                .into_iter()
                .map(|error| format!("{} at byte {}", error.message, error.span.start))
                .collect::<Vec<_>>()
                .join("; "),
        )
    })?;

    let mut collector = Collector {
        source,
        lines: Vec::new(),
        insertions: Vec::new(),
        occurrences: BTreeMap::new(),
    };
    collector.program(&program);
    collector
        .insertions
        .sort_by_key(|insertion| insertion.offset);

    let mut transformed = source.to_string();
    for insertion in collector.insertions.iter().rev() {
        transformed.insert_str(insertion.offset, &insertion.text);
    }
    Ok(VoiceScaffold {
        source: transformed,
        lines: collector.lines,
    })
}

struct Collector<'a> {
    source: &'a str,
    lines: Vec<VoiceLine>,
    insertions: Vec<Insertion>,
    occurrences: BTreeMap<(String, String), u32>,
}

struct Insertion {
    offset: usize,
    text: String,
}

impl Collector<'_> {
    fn program(&mut self, program: &Program) {
        self.statements(&program.statements);
    }

    fn statements(&mut self, statements: &[Stmt]) {
        let mut previous_voice = None;
        for statement in statements {
            if let Some(dialogue) = dialogue_statement(statement) {
                let existing_path = previous_voice.take();
                self.dialogue(statement_span(statement), dialogue, existing_path);
            } else {
                previous_voice = voice_path(statement);
            }
            self.nested(statement);
        }
    }

    fn nested(&mut self, statement: &Stmt) {
        match statement {
            Stmt::Function { body, .. } | Stmt::While { body, .. } => self.block(body),
            Stmt::If {
                then_block,
                else_block,
                ..
            } => {
                self.block(then_block);
                if let Some(else_block) = else_block {
                    self.block(else_block);
                }
            }
            Stmt::Let { value, .. } | Stmt::Expr(value) => self.expression(value),
        }
    }

    fn block(&mut self, block: &Block) {
        self.statements(&block.statements);
    }

    fn expression(&mut self, expression: &Expr) {
        match &expression.kind {
            ExprKind::Call {
                callee,
                arguments,
                trailing_block,
            } => {
                self.expression(callee);
                for argument in arguments {
                    self.expression(&argument.value);
                }
                if let Some(block) = trailing_block {
                    self.block(block);
                }
            }
            ExprKind::Member { object, .. } | ExprKind::UnaryMinus(object) => {
                self.expression(object)
            }
            ExprKind::Tuple(values) => {
                for value in values {
                    self.expression(value);
                }
            }
            ExprKind::Map(fields) => {
                for field in fields {
                    self.expression(&field.value);
                }
            }
            ExprKind::Block(block) => self.block(block),
            ExprKind::Binary { left, right, .. } => {
                self.expression(left);
                self.expression(right);
            }
            ExprKind::Ident(_)
            | ExprKind::Symbol(_)
            | ExprKind::Bool(_)
            | ExprKind::Number { .. }
            | ExprKind::String(_) => {}
        }
    }

    fn dialogue(&mut self, span: &Span, dialogue: Dialogue, existing_path: Option<String>) {
        let occurrence = self
            .occurrences
            .entry((dialogue.speaker.clone(), dialogue.text.clone()))
            .or_default();
        let id = stable_voice_id(&dialogue.speaker, &dialogue.text, *occurrence);
        *occurrence += 1;
        let generated_path = format!("voice/{id}");
        let (path, inserted) = match existing_path {
            Some(path) => (path, false),
            None => {
                let indent = indentation_at(self.source, span.start);
                self.insertions.push(Insertion {
                    offset: span.start,
                    text: format!("voice(\"{}\")\n{indent}", escape_hks(&generated_path)),
                });
                (generated_path, true)
            }
        };
        self.lines.push(VoiceLine {
            id,
            speaker: dialogue.speaker,
            text: dialogue.text,
            path,
            statement_offset: span.start,
            inserted,
        });
    }
}

struct Dialogue {
    speaker: String,
    text: String,
}

fn dialogue_statement(statement: &Stmt) -> Option<Dialogue> {
    let Stmt::Expr(expression) = statement else {
        return None;
    };
    let ExprKind::Call {
        callee, arguments, ..
    } = &expression.kind
    else {
        return None;
    };
    match callee_name(callee)?.as_str() {
        "narrate" if arguments.len() == 1 => Some(Dialogue {
            speaker: String::new(),
            text: string_argument(arguments.first()?)?,
        }),
        "say" if arguments.len() == 2 => Some(Dialogue {
            speaker: string_argument(arguments.first()?)?,
            text: string_argument(arguments.get(1)?)?,
        }),
        _ => None,
    }
}

fn voice_path(statement: &Stmt) -> Option<String> {
    let Stmt::Expr(expression) = statement else {
        return None;
    };
    let ExprKind::Call {
        callee, arguments, ..
    } = &expression.kind
    else {
        return None;
    };
    (callee_name(callee)?.as_str() == "voice" && arguments.len() == 1)
        .then(|| string_argument(&arguments[0]))?
}

fn callee_name(expression: &Expr) -> Option<String> {
    match &expression.kind {
        ExprKind::Ident(name) => Some(name.clone()),
        ExprKind::Member { object, name } => Some(format!("{}.{}", callee_name(object)?, name)),
        _ => None,
    }
}

fn string_argument(argument: &hiraku_script::hks::Argument) -> Option<String> {
    if argument.label.is_some() {
        return None;
    }
    match &argument.value.kind {
        ExprKind::String(value) => Some(value.clone()),
        _ => None,
    }
}

fn statement_span(statement: &Stmt) -> &Span {
    match statement {
        Stmt::Function { span, .. }
        | Stmt::Let { span, .. }
        | Stmt::If { span, .. }
        | Stmt::While { span, .. } => span,
        Stmt::Expr(expression) => &expression.span,
    }
}

fn indentation_at(source: &str, offset: usize) -> &str {
    let line_start = source[..offset].rfind('\n').map_or(0, |index| index + 1);
    &source[line_start..offset]
}

fn stable_voice_id(speaker: &str, text: &str, occurrence: u32) -> String {
    // FNV-1a is deliberately specified here instead of DefaultHasher, whose
    // output is not a persistence contract.
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in speaker
        .as_bytes()
        .iter()
        .copied()
        .chain([0xff])
        .chain(text.as_bytes().iter().copied())
        .chain([0xfe])
        .chain(occurrence.to_le_bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn escape_hks(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inserts_voice_before_narrate_and_say() {
        let scaffold =
            scaffold_story("narrate(\"Opening\")\n\nif true {\n    say(\"Alice\", \"Hello\")\n}\n")
                .expect("valid story must scaffold");

        assert_eq!(scaffold.lines.len(), 2);
        assert!(scaffold.source.starts_with("voice(\"voice/"));
        assert!(scaffold.source.contains("\n    voice(\"voice/"));
        assert!(scaffold.source.contains("\n    say(\"Alice\", \"Hello\")"));
        assert_eq!(scaffold.lines[0].speaker, "");
        assert_eq!(scaffold.lines[1].speaker, "Alice");
        assert!(scaffold.lines.iter().all(|line| line.inserted));
    }

    #[test]
    fn keeps_an_existing_voice_and_reports_its_path() {
        let source = "voice(\"voice/Alice/custom\")\nsay(\"Alice\", \"Hello\")\n";
        let scaffold = scaffold_story(source).expect("valid story must scaffold");

        assert_eq!(scaffold.source, source);
        assert_eq!(scaffold.lines[0].path, "voice/Alice/custom");
        assert!(!scaffold.lines[0].inserted);
    }

    #[test]
    fn traverses_seq_and_par_blocks_without_changing_their_structure() {
        let scaffold = scaffold_story(
            "seq {\n    narrate(\"one\")\n}\npar {\n    say(\"A\", \"two\")\n    say(\"B\", \"three\")\n}\n",
        )
        .expect("valid task blocks must scaffold");

        assert_eq!(scaffold.lines.len(), 3);
        assert_eq!(scaffold.source.matches("voice(\"").count(), 3);
        assert!(scaffold.source.contains("seq {\n    voice("));
        assert!(scaffold.source.contains("par {\n    voice("));
    }

    #[test]
    fn generation_is_stable_and_idempotent() {
        let once = scaffold_story("narrate(\"Same line\")\n").expect("valid story must scaffold");
        let twice = scaffold_story(&once.source).expect("generated story must remain valid");

        assert_eq!(once.source, twice.source);
        assert_eq!(once.lines[0].id, twice.lines[0].id);
        assert!(!twice.lines[0].inserted);
    }

    #[test]
    fn duplicate_text_gets_distinct_recording_identities() {
        let scaffold = scaffold_story("narrate(\"repeat\")\nnarrate(\"repeat\")\n")
            .expect("valid story must scaffold");
        assert_ne!(scaffold.lines[0].id, scaffold.lines[1].id);
    }

    #[test]
    fn exports_the_character_voice_hson_contract() {
        let scaffold = scaffold_story("say(\"alice\", \"one\")\nsay(\"alice\", \"two\")\n")
            .expect("valid story must scaffold");
        let mut rows = scaffold
            .lines
            .iter()
            .map(VoiceLine::csv_row)
            .collect::<Vec<_>>();
        rows[0].rename = "voice/scene01/hash1".to_string();
        rows[0].file = "hash1.ogg".to_string();
        rows[1].rename = "voice/scene01/hash2".to_string();
        rows[1].file = "hash2.ogg".to_string();

        let hson = export_character_hson("alice", &rows).expect("manifest must serialize");
        let manifest: CharacterVoiceManifest =
            hiraku_script::hson::from_str(&hson).expect("manifest must deserialize");
        assert_eq!(
            manifest,
            CharacterVoiceManifest {
                char: "alice".to_string(),
                voices: vec![
                    VoiceAsset {
                        name: "voice/scene01/hash1".to_string(),
                        file: "hash1.ogg".to_string(),
                    },
                    VoiceAsset {
                        name: "voice/scene01/hash2".to_string(),
                        file: "hash2.ogg".to_string(),
                    },
                ],
            }
        );
    }

    #[test]
    fn omits_rows_whose_rename_was_cleared() {
        let mut row = scaffold_story("narrate(\"unused\")\n")
            .expect("valid story must scaffold")
            .lines[0]
            .csv_row();
        row.rename.clear();

        let hson = export_character_hson("narrator", &[row]).expect("manifest must serialize");
        let manifest: CharacterVoiceManifest =
            hiraku_script::hson::from_str(&hson).expect("manifest must deserialize");
        assert!(manifest.voices.is_empty());
    }
}
