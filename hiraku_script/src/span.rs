use std::ops::Range;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    #[inline]
    pub const fn new(pos: usize, len: u32) -> Self {
        Self {
            start: pos,
            end: pos + len as usize,
        }
    }

    pub const fn new_with_end(pos: usize, end: usize) -> Self {
        assert!(end >= pos);
        Self { start: pos, end }
    }

    #[inline]
    pub const fn pos(self) -> usize {
        self.start
    }

    #[inline]
    pub const fn end(self) -> usize {
        self.end
    }

    #[inline]
    pub const fn range(self) -> Range<usize> {
        self.start..self.end
    }

    #[inline]
    pub fn str_slice(self, str: &str) -> &str {
        &str[self.range()]
    }

    pub(crate) const fn join(left: &Self, right: &Self) -> Self {
        Self {
            start: left.start,
            end: right.end,
        }
    }
}
