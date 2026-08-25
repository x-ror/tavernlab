//! A small JSON reader for build-time tooling.
//!
//! This exists because Smart App Control is enforced on the build machine and
//! blocks the compiled output of any crate's `build.rs`, which rules out serde
//! and every other derive-based option. It reads the HearthstoneJSON corpus so
//! [`xtask`] can bake it into Rust source; nothing in the engine's hot path ever
//! touches it.
//!
//! Scope is deliberately narrow: parse, look up, convert. No serialisation, no
//! derives, no borrowing games. Objects keep insertion order in a flat vector
//! because the documents involved have well under a hundred keys per object and
//! a linear scan beats a hash map at that size.

use std::fmt;

/// A parsed JSON value.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    /// All JSON numbers are doubles; [`Json::as_i64`] recovers integers.
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    /// Key/value pairs in document order.
    Obj(Vec<(String, Json)>),
}

/// Where parsing stopped and why.
#[derive(Clone, Debug, PartialEq)]
pub struct Error {
    pub offset: usize,
    pub msg: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JSON error at byte {}: {}", self.offset, self.msg)
    }
}

impl std::error::Error for Error {}

impl Json {
    /// Parse a whole document, rejecting trailing content.
    pub fn parse(src: &str) -> Result<Json, Error> {
        let mut p = Parser {
            b: src.as_bytes(),
            i: 0,
        };
        p.ws();
        let v = p.value()?;
        p.ws();
        if p.i != p.b.len() {
            return Err(p.err("trailing content after the top-level value"));
        }
        Ok(v)
    }

    /// The value for `key`, if this is an object that has one.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(kvs) => kvs.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }

    /// The number as an integer, when it has no fractional part.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Json::Num(n) if n.fract() == 0.0 && n.is_finite() => Some(*n as i64),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&[(String, Json)]> {
        match self {
            Json::Obj(o) => Some(o),
            _ => None,
        }
    }

    /// `get(key).as_str()`, defaulting to `""` — the corpus uses both a missing
    /// key and an explicit null for "no value", and callers never care which.
    pub fn str_or_empty(&self, key: &str) -> &str {
        self.get(key).and_then(Json::as_str).unwrap_or("")
    }

    /// `get(key)` as an integer, defaulting to 0 for missing, null or absent.
    pub fn i64_or_zero(&self, key: &str) -> i64 {
        self.get(key).and_then(Json::as_i64).unwrap_or(0)
    }

    /// `get(key)` as a bool, defaulting to false.
    pub fn bool_or_false(&self, key: &str) -> bool {
        self.get(key).and_then(Json::as_bool).unwrap_or(false)
    }

    /// The elements of an array-valued key, or an empty slice.
    pub fn arr_or_empty(&self, key: &str) -> &[Json] {
        self.get(key).and_then(Json::as_array).unwrap_or(&[])
    }
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Parser<'a> {
    fn err(&self, msg: &str) -> Error {
        Error {
            offset: self.i,
            msg: msg.to_string(),
        }
    }

    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn eat(&mut self, c: u8) -> Result<(), Error> {
        if self.peek() == Some(c) {
            self.i += 1;
            Ok(())
        } else {
            Err(self.err(&format!("expected {:?}", c as char)))
        }
    }

    fn lit(&mut self, word: &str, v: Json) -> Result<Json, Error> {
        if self.b[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            Ok(v)
        } else {
            Err(self.err(&format!("expected {word}")))
        }
    }

    fn value(&mut self) -> Result<Json, Error> {
        match self
            .peek()
            .ok_or_else(|| self.err("unexpected end of input"))?
        {
            b'{' => self.object(),
            b'[' => self.array(),
            b'"' => Ok(Json::Str(self.string()?)),
            b't' => self.lit("true", Json::Bool(true)),
            b'f' => self.lit("false", Json::Bool(false)),
            b'n' => self.lit("null", Json::Null),
            b'-' | b'0'..=b'9' => self.number(),
            c => Err(self.err(&format!("unexpected character {:?}", c as char))),
        }
    }

    fn object(&mut self) -> Result<Json, Error> {
        self.eat(b'{')?;
        let mut out = Vec::new();
        self.ws();
        if self.peek() == Some(b'}') {
            self.i += 1;
            return Ok(Json::Obj(out));
        }
        loop {
            self.ws();
            let k = self.string()?;
            self.ws();
            self.eat(b':')?;
            self.ws();
            out.push((k, self.value()?));
            self.ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b'}') => {
                    self.i += 1;
                    return Ok(Json::Obj(out));
                }
                _ => return Err(self.err("expected ',' or '}' in object")),
            }
        }
    }

    fn array(&mut self) -> Result<Json, Error> {
        self.eat(b'[')?;
        let mut out = Vec::new();
        self.ws();
        if self.peek() == Some(b']') {
            self.i += 1;
            return Ok(Json::Arr(out));
        }
        loop {
            self.ws();
            out.push(self.value()?);
            self.ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b']') => {
                    self.i += 1;
                    return Ok(Json::Arr(out));
                }
                _ => return Err(self.err("expected ',' or ']' in array")),
            }
        }
    }

    fn string(&mut self) -> Result<String, Error> {
        self.eat(b'"')?;
        let start = self.i;
        // Fast path: no escapes means the bytes can be taken verbatim, which is
        // most of a 1.5 MB corpus.
        while let Some(c) = self.peek() {
            match c {
                b'"' => {
                    let s = std::str::from_utf8(&self.b[start..self.i])
                        .map_err(|_| self.err("invalid UTF-8 in string"))?
                        .to_string();
                    self.i += 1;
                    return Ok(s);
                }
                b'\\' => break,
                _ => self.i += 1,
            }
        }
        // Slow path: rewind and rebuild with escapes resolved.
        self.i = start;
        let mut out = String::new();
        loop {
            let c = self.peek().ok_or_else(|| self.err("unterminated string"))?;
            match c {
                b'"' => {
                    self.i += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.i += 1;
                    let e = self.peek().ok_or_else(|| self.err("unterminated escape"))?;
                    self.i += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => out.push(self.unicode_escape()?),
                        _ => return Err(self.err("unknown escape")),
                    }
                }
                _ => {
                    // Copy one whole UTF-8 sequence, not one byte, so multi-byte
                    // characters survive.
                    let len = utf8_len(c);
                    let end = (self.i + len).min(self.b.len());
                    let s = std::str::from_utf8(&self.b[self.i..end])
                        .map_err(|_| self.err("invalid UTF-8 in string"))?;
                    out.push_str(s);
                    self.i = end;
                }
            }
        }
    }

    /// A `\uXXXX` escape, joining a surrogate pair when one follows.
    fn unicode_escape(&mut self) -> Result<char, Error> {
        let hi = self.hex4()?;
        if (0xD800..0xDC00).contains(&hi) {
            if self.peek() == Some(b'\\') && self.b.get(self.i + 1) == Some(&b'u') {
                self.i += 2;
                let lo = self.hex4()?;
                if (0xDC00..0xE000).contains(&lo) {
                    let c = 0x1_0000 + ((hi - 0xD800) << 10) + (lo - 0xDC00);
                    return char::from_u32(c).ok_or_else(|| self.err("bad surrogate pair"));
                }
                return Err(self.err("high surrogate not followed by a low one"));
            }
            return Err(self.err("lone high surrogate"));
        }
        char::from_u32(hi).ok_or_else(|| self.err("escape is not a character"))
    }

    fn hex4(&mut self) -> Result<u32, Error> {
        if self.i + 4 > self.b.len() {
            return Err(self.err("truncated \\u escape"));
        }
        let s = std::str::from_utf8(&self.b[self.i..self.i + 4])
            .map_err(|_| self.err("invalid \\u escape"))?;
        let v = u32::from_str_radix(s, 16).map_err(|_| self.err("invalid \\u escape"))?;
        self.i += 4;
        Ok(v)
    }

    fn number(&mut self) -> Result<Json, Error> {
        let start = self.i;
        if self.peek() == Some(b'-') {
            self.i += 1;
        }
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.i += 1;
        }
        if self.peek() == Some(b'.') {
            self.i += 1;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.i += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.i += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.i += 1;
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.i += 1;
            }
        }
        let s = std::str::from_utf8(&self.b[start..self.i]).map_err(|_| self.err("bad number"))?;
        s.parse::<f64>()
            .map(Json::Num)
            .map_err(|_| self.err("bad number"))
    }
}

/// Length in bytes of the UTF-8 sequence starting with `first`.
fn utf8_len(first: u8) -> usize {
    match first {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalars() {
        assert_eq!(Json::parse("null").unwrap(), Json::Null);
        assert_eq!(Json::parse("true").unwrap(), Json::Bool(true));
        assert_eq!(Json::parse("false").unwrap(), Json::Bool(false));
        assert_eq!(Json::parse("0").unwrap(), Json::Num(0.0));
        assert_eq!(Json::parse("-12").unwrap().as_i64(), Some(-12));
        assert_eq!(Json::parse("1.5e3").unwrap().as_f64(), Some(1500.0));
    }

    #[test]
    fn integers_and_fractions_are_distinguished() {
        // Card costs must never silently truncate.
        assert_eq!(Json::parse("3").unwrap().as_i64(), Some(3));
        assert_eq!(Json::parse("3.5").unwrap().as_i64(), None);
    }

    #[test]
    fn strings_with_and_without_escapes() {
        assert_eq!(Json::parse(r#""plain""#).unwrap().as_str(), Some("plain"));
        assert_eq!(
            Json::parse(r#""a\nb\t\"c\"\\d\/e""#).unwrap().as_str(),
            Some("a\nb\t\"c\"\\d/e")
        );
    }

    #[test]
    fn non_ascii_survives_both_paths() {
        // The corpus carries card names with accents and typographic quotes;
        // the fast path copies bytes and the slow path copies characters, so
        // both need checking.
        assert_eq!(
            Json::parse("\"Sindragosa — Frostmourne\"")
                .unwrap()
                .as_str(),
            Some("Sindragosa — Frostmourne")
        );
        assert_eq!(
            Json::parse(r#""Al\u0027Akir — ✓""#).unwrap().as_str(),
            Some("Al'Akir — ✓")
        );
    }

    #[test]
    fn surrogate_pairs_join() {
        assert_eq!(
            Json::parse(r#""\ud83d\ude00""#).unwrap().as_str(),
            Some("😀")
        );
    }

    #[test]
    fn lone_surrogate_is_an_error_not_a_replacement_char() {
        assert!(Json::parse(r#""\ud83d""#).is_err());
    }

    #[test]
    fn nested_structures() {
        let v = Json::parse(r#"{"a":[1,2,{"b":null}],"c":{"d":true}}"#).unwrap();
        assert_eq!(v.get("a").unwrap().as_array().unwrap().len(), 3);
        assert_eq!(v.get("c").unwrap().get("d").unwrap().as_bool(), Some(true));
        assert!(v.get("missing").is_none());
    }

    #[test]
    fn empty_containers() {
        assert_eq!(Json::parse("{}").unwrap().as_object().unwrap().len(), 0);
        assert_eq!(Json::parse("[]").unwrap().as_array().unwrap().len(), 0);
        assert_eq!(
            Json::parse(r#"{"a":[],"b":{}}"#)
                .unwrap()
                .arr_or_empty("a")
                .len(),
            0
        );
    }

    #[test]
    fn whitespace_everywhere() {
        let v = Json::parse(" {\n \"a\" : [ 1 , 2 ] \t} \n").unwrap();
        assert_eq!(v.arr_or_empty("a").len(), 2);
    }

    #[test]
    fn accessors_default_rather_than_panic() {
        let v = Json::parse(r#"{"s":"x","n":4,"b":true,"nul":null}"#).unwrap();
        assert_eq!(v.str_or_empty("s"), "x");
        assert_eq!(v.str_or_empty("nul"), "");
        assert_eq!(v.str_or_empty("absent"), "");
        assert_eq!(v.i64_or_zero("n"), 4);
        assert_eq!(v.i64_or_zero("nul"), 0);
        assert!(v.bool_or_false("b"));
        assert!(!v.bool_or_false("absent"));
    }

    #[test]
    fn malformed_input_reports_an_offset() {
        for bad in [
            "",
            "{",
            "[1,",
            "{\"a\"}",
            "{\"a\":}",
            "tru",
            "\"unterminated",
            "[1] extra",
            "{\"a\":1,}",
            "01x",
        ] {
            let e = Json::parse(bad);
            assert!(e.is_err(), "{bad:?} should not parse");
        }
    }

    #[test]
    fn duplicate_keys_keep_the_first() {
        // Order-preserving storage plus a linear find; the corpus has no
        // duplicates, but the behaviour should be defined rather than accidental.
        let v = Json::parse(r#"{"a":1,"a":2}"#).unwrap();
        assert_eq!(v.i64_or_zero("a"), 1);
    }
}
