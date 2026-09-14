//! UTF-8 byte / UTF-16 editor position conversion, independent of LSP/JSON.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextPosition {
    pub line: u32,
    pub character: u32,
}
pub struct LineIndex<'a> {
    source: &'a str,
    starts: Vec<usize>,
}
impl<'a> LineIndex<'a> {
    pub fn new(source: &'a str) -> Self {
        let mut starts = vec![0];
        starts.extend(source.match_indices('\n').map(|(offset, _)| offset + 1));
        Self { source, starts }
    }
    pub fn position(&self, byte: usize) -> Option<TextPosition> {
        if byte > self.source.len() || !self.source.is_char_boundary(byte) {
            return None;
        }
        let line = self.starts.partition_point(|start| *start <= byte) - 1;
        Some(TextPosition {
            line: line as u32,
            character: self.source[self.starts[line]..byte]
                .trim_end_matches('\r')
                .encode_utf16()
                .count() as u32,
        })
    }
    pub fn offset(&self, position: TextPosition) -> Option<usize> {
        let start = *self.starts.get(position.line as usize)?;
        let end = self
            .starts
            .get(position.line as usize + 1)
            .copied()
            .unwrap_or(self.source.len());
        let line = self.source[start..end].trim_end_matches(['\r', '\n']);
        let mut units = 0;
        for (offset, character) in line.char_indices() {
            if units == position.character {
                return Some(start + offset);
            }
            units += character.len_utf16() as u32;
            if units > position.character {
                return None;
            }
        }
        (units == position.character).then_some(start + line.len())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn utf16_rejects_half_surrogates_and_roundtrips_unicode() {
        let index = LineIndex::new("a😀文\r\nBob");
        assert_eq!(
            index.offset(TextPosition {
                line: 0,
                character: 2
            }),
            None
        );
        for byte in [0, 1, 5, 8, 10, 13] {
            let position = index.position(byte).expect("boundary");
            assert_eq!(index.offset(position), Some(byte));
        }
    }
}
