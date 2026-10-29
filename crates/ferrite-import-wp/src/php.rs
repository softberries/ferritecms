//! A reader for PHP's `serialize()` format, as WordPress stores arrays in meta.

use thiserror::Error;

/// A PHP value. Arrays keep their order; keys are `Int` or `Str`.
#[derive(Debug, Clone, PartialEq)]
pub enum PhpValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Array(Vec<(PhpValue, PhpValue)>),
}

impl PhpValue {
    /// The value under string key `key` of an array.
    pub fn get(&self, key: &str) -> Option<&PhpValue> {
        self.entries()
            .iter()
            .find(|(k, _)| matches!(k, PhpValue::Str(s) if s == key))
            .map(|(_, v)| v)
    }

    /// An array's entries in order; empty for any other value.
    pub fn entries(&self) -> &[(PhpValue, PhpValue)] {
        match self {
            PhpValue::Array(entries) => entries,
            _ => &[],
        }
    }

    /// A string as a string, an integer in decimal; `None` otherwise.
    pub fn as_text(&self) -> Option<String> {
        match self {
            PhpValue::Str(s) => Some(s.clone()),
            PhpValue::Int(i) => Some(i.to_string()),
            _ => None,
        }
    }

    /// PHP truthiness: `false`, `0`, `0.0`, `""`, `"0"`, an empty array and `null` are
    /// false.
    pub fn truthy(&self) -> bool {
        match self {
            PhpValue::Null => false,
            PhpValue::Bool(b) => *b,
            PhpValue::Int(i) => *i != 0,
            PhpValue::Float(f) => *f != 0.0,
            PhpValue::Str(s) => !s.is_empty() && s != "0",
            PhpValue::Array(a) => !a.is_empty(),
        }
    }
}

#[derive(Debug, Error, PartialEq)]
pub enum PhpError {
    #[error("serialised objects are not read (byte {offset})")]
    Object { offset: usize },
    #[error("nesting deeper than 32 at byte {offset}")]
    TooDeep { offset: usize },
    #[error("invalid serialised value at byte {offset}: {msg}")]
    Syntax { offset: usize, msg: &'static str },
}

/// Arrays nested deeper than this are refused.
const MAX_DEPTH: usize = 32;

/// Reads one serialised value; the whole input must be that value. Objects (`O:`, `C:`) and
/// references (`r:`, `R:`) are refused, never instantiated. String and array lengths are
/// checked against the input before anything is allocated.
pub fn unserialize(s: &str) -> Result<PhpValue, PhpError> {
    let mut p = Parser { s, pos: 0 };
    let v = p.value(0)?;
    if p.pos != s.len() {
        return Err(syntax(p.pos, "trailing data"));
    }
    Ok(v)
}

fn syntax(offset: usize, msg: &'static str) -> PhpError {
    PhpError::Syntax { offset, msg }
}

struct Parser<'a> {
    s: &'a str,
    pos: usize,
}

impl Parser<'_> {
    fn value(&mut self, depth: usize) -> Result<PhpValue, PhpError> {
        let start = self.pos;
        match self.byte()? {
            b'N' => {
                self.expect(b';')?;
                Ok(PhpValue::Null)
            }
            b'b' => {
                self.expect(b':')?;
                let v = match self.byte()? {
                    b'0' => false,
                    b'1' => true,
                    _ => return Err(syntax(self.pos - 1, "boolean is not 0 or 1")),
                };
                self.expect(b';')?;
                Ok(PhpValue::Bool(v))
            }
            b'i' => {
                self.expect(b':')?;
                let at = self.pos;
                let t = self.until(b';')?;
                t.parse()
                    .map(PhpValue::Int)
                    .map_err(|_| syntax(at, "invalid integer"))
            }
            b'd' => {
                self.expect(b':')?;
                let at = self.pos;
                let t = self.until(b';')?;
                t.parse()
                    .map(PhpValue::Float)
                    .map_err(|_| syntax(at, "invalid float"))
            }
            b's' => {
                self.expect(b':')?;
                let len = self.length(b':')?;
                self.expect(b'"')?;
                let end = self
                    .pos
                    .checked_add(len)
                    .filter(|&e| e <= self.s.len() && self.s.is_char_boundary(e))
                    .ok_or_else(|| syntax(self.pos, "string length does not match the input"))?;
                let v = self.s[self.pos..end].to_owned();
                self.pos = end;
                self.expect(b'"')?;
                self.expect(b';')?;
                Ok(PhpValue::Str(v))
            }
            b'a' => {
                if depth >= MAX_DEPTH {
                    return Err(PhpError::TooDeep { offset: start });
                }
                self.expect(b':')?;
                let n = self.length(b':')?;
                self.expect(b'{')?;
                // The shortest pair, `i:0;N;`, is 6 bytes.
                if n > (self.s.len() - self.pos) / 6 {
                    return Err(syntax(self.pos, "array count does not match the input"));
                }
                let mut pairs = Vec::with_capacity(n.min(1024));
                for _ in 0..n {
                    if !matches!(self.peek(), Some(b'i' | b's')) {
                        return Err(syntax(self.pos, "array key is not an integer or string"));
                    }
                    let k = self.value(depth + 1)?;
                    let v = self.value(depth + 1)?;
                    pairs.push((k, v));
                }
                self.expect(b'}')?;
                Ok(PhpValue::Array(pairs))
            }
            b'O' | b'C' => Err(PhpError::Object { offset: start }),
            _ => Err(syntax(start, "unknown or unsupported type")),
        }
    }

    fn peek(&self) -> Option<u8> {
        self.s.as_bytes().get(self.pos).copied()
    }

    fn byte(&mut self) -> Result<u8, PhpError> {
        let b = self
            .peek()
            .ok_or_else(|| syntax(self.pos, "unexpected end"))?;
        self.pos += 1;
        Ok(b)
    }

    fn expect(&mut self, want: u8) -> Result<(), PhpError> {
        match self.peek() {
            Some(b) if b == want => {
                self.pos += 1;
                Ok(())
            }
            _ => Err(syntax(self.pos, "unexpected character")),
        }
    }

    /// The text up to `end`, which is consumed.
    fn until(&mut self, end: u8) -> Result<&str, PhpError> {
        let rest = &self.s.as_bytes()[self.pos..];
        let n = rest
            .iter()
            .position(|&b| b == end)
            .ok_or_else(|| syntax(self.pos, "unexpected end"))?;
        let t = &self.s[self.pos..self.pos + n];
        self.pos += n + 1;
        Ok(t)
    }

    /// A decimal length up to `end`.
    fn length(&mut self, end: u8) -> Result<usize, PhpError> {
        let at = self.pos;
        let t = self.until(end)?;
        if t.is_empty() || !t.bytes().all(|b| b.is_ascii_digit()) {
            return Err(syntax(at, "invalid length"));
        }
        t.parse().map_err(|_| syntax(at, "invalid length"))
    }
}
