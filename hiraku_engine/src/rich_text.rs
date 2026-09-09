//! Presentation markup, independent of HKS parsing and Bevy entities.
//! `{ruby:reading}base{/ruby}` annotates a nonempty, single-line base.
//! Doubled braces escape literal braces; unknown braces remain ordinary text.

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Ruby {
    pub start: usize,
    pub end: usize,
    pub reading: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RichText {
    pub text: String,
    pub ruby: Vec<Ruby>,
}

pub(crate) fn parse(source: &str) -> Result<RichText, String> {
    let mut result = RichText::default();
    let mut offset = 0;
    let mut count = 0;
    let mut active: Option<(usize, String)> = None;
    while offset < source.len() {
        let rest = &source[offset..];
        if rest.starts_with("{{") || rest.starts_with("}}") {
            result.text.push(rest.as_bytes()[0] as char);
            count += 1;
            offset += 2;
        } else if let Some(header) = rest.strip_prefix("{ruby:") {
            if active.is_some() {
                return Err(format!("nested ruby at byte {offset}"));
            }
            let end = header
                .find('}')
                .ok_or_else(|| format!("unclosed ruby header at byte {offset}"))?;
            let reading = &header[..end];
            if reading.is_empty() || reading.contains(['\n', '\r', '{']) {
                return Err(format!(
                    "ruby reading must be nonempty and single-line at byte {offset}"
                ));
            }
            active = Some((count, reading.into()));
            offset += 6 + end + 1;
        } else if rest.starts_with("{/ruby}") {
            let (start, reading) = active
                .take()
                .ok_or_else(|| format!("unexpected ruby close at byte {offset}"))?;
            if start == count {
                return Err(format!("ruby base must not be empty at byte {offset}"));
            }
            result.ruby.push(Ruby {
                start,
                end: count,
                reading,
            });
            offset += 7;
        } else {
            let ch = rest.chars().next().expect("nonempty UTF-8 suffix");
            if active.is_some() && matches!(ch, '\n' | '\r') {
                return Err(format!("ruby base must be single-line at byte {offset}"));
            }
            result.text.push(ch);
            count += 1;
            offset += ch.len_utf8();
        }
    }
    if active.is_some() {
        return Err("missing {/ruby}".into());
    }
    Ok(result)
}

pub(crate) fn character_count(source: &str) -> usize {
    if !source.contains('{') && !source.contains('}') {
        return source.chars().count();
    }
    parse(source).map_or_else(
        |_| source.chars().count(),
        |value| value.text.chars().count(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ruby_uses_base_character_indices_not_markup_or_reading_bytes() {
        let value = parse("Alice: {ruby:kanji}漢字{/ruby}!").expect("valid ruby");
        assert_eq!(value.text, "Alice: 漢字!");
        assert_eq!(
            value.ruby,
            [Ruby {
                start: 7,
                end: 9,
                reading: "kanji".into()
            }]
        );
        assert_eq!(character_count("{ruby:reading}字{/ruby}"), 1);
    }
    #[test]
    fn braces_and_plain_strings_are_unambiguous() {
        assert_eq!(
            parse("{{ruby:Bob}} {ordinary}")
                .expect("literal braces")
                .text,
            "{ruby:Bob} {ordinary}"
        );
        for invalid in [
            "{ruby:}Bob{/ruby}",
            "{ruby:Bob}{/ruby}",
            "{ruby:Bob}Alice",
            "{/ruby}",
            "{ruby:Bob}A\nB{/ruby}",
            "{ruby:Bob}{ruby:Alice}A{/ruby}{/ruby}",
        ] {
            assert!(parse(invalid).is_err(), "{invalid}");
        }
    }
}
