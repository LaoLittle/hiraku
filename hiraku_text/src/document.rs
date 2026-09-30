use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextStyle {
    pub color: Option<[u8; 4]>,
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub underline: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ruby {
    pub start: u32,
    pub end: u32,
    pub reading: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    pub text: String,
    pub ruby: Vec<Ruby>,
    pub styles: Vec<TextStyle>,
}

impl Document {
    pub fn plain(text: impl Into<String>) -> Self {
        let text = text.into();
        let styles = vec![TextStyle::default(); text.chars().count()];
        Self {
            text,
            ruby: Vec::new(),
            styles,
        }
    }

    pub(crate) fn append(&mut self, other: &Self) {
        let offset = self.styles.len() as u32;
        self.text.push_str(&other.text);
        self.styles.extend_from_slice(&other.styles);
        self.ruby.extend(other.ruby.iter().map(|ruby| Ruby {
            start: ruby.start + offset,
            end: ruby.end + offset,
            reading: ruby.reading.clone(),
        }));
    }
}
