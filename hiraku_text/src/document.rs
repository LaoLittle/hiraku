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
    pub fn to_markup(&self) -> String {
        let chars: Vec<char> = self.text.chars().collect();
        let mut output = String::new();
        let mut index = 0;
        while index < chars.len() {
            if let Some(ruby) = self.ruby.iter().find(|ruby| {
                ruby.start as usize == index
                    && ruby.end as usize <= chars.len()
                    && ruby.end > ruby.start
            }) {
                output.push_str("#ruby(");
                quoted(&mut output, &ruby.reading);
                output.push_str(")[");
                self.write_runs(&chars, index, ruby.end as usize, &mut output);
                output.push(']');
                index = ruby.end as usize;
            } else {
                let end = self
                    .ruby
                    .iter()
                    .filter_map(|ruby| {
                        ((ruby.start as usize) > index).then_some(ruby.start as usize)
                    })
                    .min()
                    .unwrap_or(chars.len())
                    .min(chars.len());
                self.write_runs(&chars, index, end, &mut output);
                index = end;
            }
        }
        output
    }

    fn write_runs(&self, chars: &[char], mut index: usize, end: usize, output: &mut String) {
        while index < end {
            let style = self.styles.get(index).copied().unwrap_or_default();
            let mut next = index + 1;
            while next < end && self.styles.get(next).copied().unwrap_or_default() == style {
                next += 1;
            }
            let mut wrappers = 0;
            for (enabled, function) in [
                (style.bold, "strong"),
                (style.italic, "emph"),
                (style.strike, "strike"),
                (style.underline, "underline"),
            ] {
                if enabled {
                    output.push('#');
                    output.push_str(function);
                    output.push('[');
                    wrappers += 1;
                }
            }
            if let Some([r, g, b, a]) = style.color {
                use std::fmt::Write;
                write!(output, "#color(\"#{r:02x}{g:02x}{b:02x}{a:02x}\")[").expect("String write");
                wrappers += 1;
            }
            output.push_str("#text(");
            quoted(output, &chars[index..next].iter().collect::<String>());
            output.push(')');
            output.extend(std::iter::repeat_n(']', wrappers));
            index = next;
        }
    }
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

fn quoted(output: &mut String, value: &str) {
    use std::fmt::Write;
    output.push('"');
    for ch in value.chars() {
        match ch {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '{' => output.push_str("\\u{7b}"),
            '}' => output.push_str("\\u{7d}"),
            ch if ch.is_control() => {
                write!(output, "\\u{{{:x}}}", ch as u32).expect("String write")
            }
            ch => output.push(ch),
        }
    }
    output.push('"');
}
