//! PostgreSQL's array text, as `array_out` prints it and `array_in` reads
//! it, rendered as JSON array text.

use alloc::borrow::Cow;
use alloc::string::{String, ToString};

use super::{JsonArray, MAX_DIMENSIONS, lower_bound_refusal};
use crate::encoding::Value;
use crate::wire::error::DecodeError;

/// Render PostgreSQL array text such as `{1,NULL,3}`, `{"a b",c}` or
/// `{{1,2},{3,4}}` as JSON array text, each non-null element's text,
/// unquoted and unescaped, decoded by `element`.
///
/// # Errors
///
/// [`DecodeError::WrongPayloadKind`] for text `array_in` would refuse,
/// [`DecodeError::UnrepresentableArray`] for a lower bound other than 1
/// or a blob element, and any error of `element`.
pub(crate) fn decode_text_array<'a, S, B>(
    column: &str,
    text: &'a str,
    element: impl FnMut(Cow<'a, str>) -> Result<Value<S, B>, DecodeError>,
) -> Result<Value<S, B>, DecodeError>
where
    S: From<String> + AsRef<str>,
{
    Parser {
        text,
        pos: 0,
        json: JsonArray::new(column),
        element,
        lengths: [None; MAX_DIMENSIONS],
        deepest: 0,
        leaf_depth: None,
        leaves: 0,
    }
    .run()
}

/// Whitespace `array_in` skips around elements and braces.
const fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

struct Parser<'a, 'c, F> {
    text: &'a str,
    pos: usize,
    json: JsonArray<'c>,
    element: F,
    /// Length of every array at each depth, once one has closed.
    lengths: [Option<usize>; MAX_DIMENSIONS],
    /// Deepest nesting opened so far, 1 for the outer braces.
    deepest: usize,
    /// Depth of the elements, once one is read.
    leaf_depth: Option<usize>,
    leaves: usize,
}

impl<'a, S, B, F> Parser<'a, '_, F>
where
    S: From<String> + AsRef<str>,
    F: FnMut(Cow<'a, str>) -> Result<Value<S, B>, DecodeError>,
{
    fn malformed(&self, actual: &'static str) -> DecodeError {
        DecodeError::WrongPayloadKind {
            column: self.json.column.to_string(),
            expected: "PostgreSQL array text",
            actual,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.pos).copied()
    }

    fn skip_space(&mut self) {
        while self.peek().is_some_and(is_space) {
            self.pos += 1;
        }
    }

    fn run(mut self) -> Result<Value<S, B>, DecodeError> {
        self.skip_space();
        let declared = self.decoration()?;
        if self.peek() != Some(b'{') {
            return Err(self.malformed("text not opening with {"));
        }
        self.pos += 1;
        self.level(1)?;
        self.skip_space();
        if self.pos != self.text.len() {
            return Err(self.malformed("text after the closing }"));
        }
        if let Some((lengths, ndim)) = declared {
            let parsed = self.lengths[..self.deepest]
                .iter()
                .map(|length| u32::try_from(length.unwrap_or(0)).ok());
            if ndim != self.deepest || !parsed.eq(lengths[..ndim].iter().map(|&l| Some(l))) {
                return Err(self.malformed("dimensions differing from their decoration"));
            }
        }
        if self.leaves == 0 {
            let mut empty = JsonArray::new(self.json.column);
            empty.open();
            empty.close();
            return Ok(empty.finish());
        }
        Ok(self.json.finish())
    }

    /// Read the optional dimension decoration `[1:2][1:3]=`, returning the
    /// lengths it declares and their count.
    fn decoration(&mut self) -> Result<Option<([u32; MAX_DIMENSIONS], usize)>, DecodeError> {
        if self.peek() != Some(b'[') {
            return Ok(None);
        }
        let mut lengths = [0; MAX_DIMENSIONS];
        let mut ndim = 0;
        while self.peek() == Some(b'[') {
            if ndim == MAX_DIMENSIONS {
                return Err(self.malformed("more than 6 dimensions"));
            }
            self.pos += 1;
            let first = self.bound()?;
            let upper = if self.peek() == Some(b':') {
                if first != 1 {
                    return Err(lower_bound_refusal(self.json.column));
                }
                self.pos += 1;
                self.bound()?
            } else {
                first
            };
            if self.peek() != Some(b']') || upper < 1 {
                return Err(self.malformed("a malformed dimension decoration"));
            }
            self.pos += 1;
            lengths[ndim] = upper.unsigned_abs();
            ndim += 1;
            self.skip_space();
        }
        if self.peek() != Some(b'=') {
            return Err(self.malformed("a dimension decoration without ="));
        }
        self.pos += 1;
        self.skip_space();
        Ok(Some((lengths, ndim)))
    }

    /// Read one signed bound of a dimension decoration.
    fn bound(&mut self) -> Result<i32, DecodeError> {
        let start = self.pos;
        if matches!(self.peek(), Some(b'-' | b'+')) {
            self.pos += 1;
        }
        while self.peek().is_some_and(|b| b.is_ascii_digit()) {
            self.pos += 1;
        }
        self.text[start..self.pos]
            .parse()
            .map_err(|_| self.malformed("a malformed dimension decoration"))
    }

    /// Read one array level whose `{` at `depth` has been consumed.
    fn level(&mut self, depth: usize) -> Result<(), DecodeError> {
        self.json.open();
        self.deepest = self.deepest.max(depth);
        self.skip_space();
        let mut count = 0;
        if self.peek() == Some(b'}') {
            self.pos += 1;
        } else {
            loop {
                self.skip_space();
                match self.peek() {
                    Some(b'{') => {
                        if depth == MAX_DIMENSIONS {
                            return Err(self.malformed("more than 6 dimensions"));
                        }
                        if self.leaf_depth.is_some_and(|leaf| leaf <= depth) {
                            return Err(self.malformed("elements and sub-arrays mixed"));
                        }
                        self.pos += 1;
                        self.level(depth + 1)?;
                    }
                    Some(_) => {
                        if self.deepest > depth {
                            return Err(self.malformed("elements and sub-arrays mixed"));
                        }
                        self.leaf_depth = Some(depth);
                        self.scalar()?;
                        self.leaves += 1;
                    }
                    None => return Err(self.malformed("an unterminated array")),
                }
                count += 1;
                self.skip_space();
                match self.peek() {
                    Some(b',') => self.pos += 1,
                    Some(b'}') => {
                        self.pos += 1;
                        break;
                    }
                    _ => return Err(self.malformed("an element followed by neither , nor }")),
                }
            }
        }
        self.json.close();
        let length = &mut self.lengths[depth - 1];
        match *length {
            None => *length = Some(count),
            Some(previous) if previous != count => {
                return Err(self.malformed("sub-arrays of different lengths"));
            }
            Some(_) => {}
        }
        Ok(())
    }

    /// Read one element, quoted or not, and append its decoded value once
    /// the delimiter after it is known to be well formed.
    fn scalar(&mut self) -> Result<(), DecodeError> {
        let content = if self.peek() == Some(b'"') {
            self.pos += 1;
            let content = self.quoted()?;
            self.skip_space();
            Some(content)
        } else {
            self.unquoted()?
        };
        if !matches!(self.peek(), Some(b',' | b'}')) {
            return Err(self.malformed("an element followed by neither , nor }"));
        }
        let value = match content {
            Some(content) => (self.element)(content)?,
            None => Value::Null,
        };
        self.json.push(&value)
    }

    /// The content of a quoted element whose opening `"` is consumed.
    fn quoted(&mut self) -> Result<Cow<'a, str>, DecodeError> {
        let text = self.text;
        let bytes = text.as_bytes();
        let mut owned: Option<String> = None;
        let mut segment = self.pos;
        loop {
            match bytes.get(self.pos) {
                Some(b'"') => break,
                Some(b'\\') if self.pos + 1 < bytes.len() => {
                    owned
                        .get_or_insert_with(String::new)
                        .push_str(&text[segment..self.pos]);
                    segment = self.pos + 1;
                    self.pos += 2;
                }
                Some(_) => self.pos += 1,
                None => return Err(self.malformed("an unterminated quoted element")),
            }
        }
        let tail = &text[segment..self.pos];
        self.pos += 1;
        Ok(match owned {
            Some(mut owned) => {
                owned.push_str(tail);
                Cow::Owned(owned)
            }
            None => Cow::Borrowed(tail),
        })
    }

    /// The content of an unquoted element, without its trailing whitespace,
    /// or `None` for `NULL`.
    fn unquoted(&mut self) -> Result<Option<Cow<'a, str>>, DecodeError> {
        let text = self.text;
        let bytes = text.as_bytes();
        let start = self.pos;
        let mut owned: Option<String> = None;
        let mut segment = start;
        let mut end = start;
        loop {
            match bytes.get(self.pos) {
                Some(b',' | b'}') => break,
                Some(b'{' | b'"') => {
                    return Err(self.malformed("an unquoted element holding { or \""));
                }
                Some(b'\\') => {
                    let escaped = text[self.pos + 1..]
                        .chars()
                        .next()
                        .ok_or_else(|| self.malformed("an unterminated array"))?;
                    owned
                        .get_or_insert_with(String::new)
                        .push_str(&text[segment..self.pos]);
                    segment = self.pos + 1;
                    self.pos += 1 + escaped.len_utf8();
                    end = self.pos;
                }
                Some(&b) => {
                    self.pos += 1;
                    if !is_space(b) {
                        end = self.pos;
                    }
                }
                None => return Err(self.malformed("an unterminated array")),
            }
        }
        match owned {
            Some(mut owned) => {
                owned.push_str(&text[segment..end]);
                Ok(Some(Cow::Owned(owned)))
            }
            None if end == start => Err(self.malformed("an empty unquoted element")),
            None if text[start..end].eq_ignore_ascii_case("NULL") => Ok(None),
            None => Ok(Some(Cow::Borrowed(&text[start..end]))),
        }
    }
}
