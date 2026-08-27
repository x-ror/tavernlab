//! Writing JSON.
//!
//! The reader half of this crate exists for build-time tooling; this half
//! exists because the HTTP server answers in JSON and there is still no
//! serialisation crate available (see the crate docs). It is a *writer*, not a
//! serialiser: there are no derives and nothing reflects over a struct.
//!
//! Nesting is expressed with closures rather than paired `begin`/`end` calls:
//!
//! ```
//! # use tavernlab_json::Out;
//! let mut out = Out::new();
//! out.obj(|o| {
//!     o.field("name", |v| v.str("Fireball"));
//!     o.field("cost", |v| v.num(4.0));
//!     o.field("tags", |v| v.arr(|a| a.item(|v| v.str("spell"))));
//! });
//! assert_eq!(out.finish(), r#"{"name":"Fireball","cost":4,"tags":["spell"]}"#);
//! ```
//!
//! That shape is the whole reason to have a type here at all: a builder that
//! can emit `{"a":1,}` or forget a brace is a builder that ships a response no
//! browser will parse, and the bug surfaces in the front end rather than here.

use std::fmt::Write as _;

/// A JSON document under construction.
#[derive(Default)]
pub struct Out {
    buf: String,
}

impl Out {
    pub fn new() -> Out {
        Out { buf: String::new() }
    }

    /// The finished document.
    pub fn finish(self) -> String {
        self.buf
    }

    /// Write one object. The closure adds its fields.
    pub fn obj(&mut self, f: impl FnOnce(&mut ObjOut<'_>)) {
        self.buf.push('{');
        let mut o = ObjOut {
            out: self,
            first: true,
        };
        f(&mut o);
        self.buf.push('}');
    }

    /// Write one array. The closure adds its items.
    pub fn arr(&mut self, f: impl FnOnce(&mut ArrOut<'_>)) {
        self.buf.push('[');
        let mut a = ArrOut {
            out: self,
            first: true,
        };
        f(&mut a);
        self.buf.push(']');
    }

    pub fn str(&mut self, s: &str) {
        escape_into(s, &mut self.buf);
    }

    /// A number. Non-finite values are written as `null`: JSON has no way to
    /// say NaN, and a rate over zero games is genuinely "no answer" rather
    /// than zero.
    pub fn num(&mut self, n: f64) {
        if !n.is_finite() {
            self.buf.push_str("null");
        } else if n.fract() == 0.0 && n.abs() < 9e15 {
            let _ = write!(self.buf, "{}", n as i64);
        } else {
            let _ = write!(self.buf, "{n}");
        }
    }

    /// A number rounded to `places` decimals, which is how every rate and
    /// delta in the API is published — a win rate printed to seventeen
    /// digits claims a precision no sample size here supports.
    pub fn round(&mut self, n: f64, places: u32) {
        let f = 10f64.powi(places as i32);
        self.num((n * f).round() / f);
    }

    pub fn int(&mut self, n: i64) {
        let _ = write!(self.buf, "{n}");
    }

    pub fn bool(&mut self, b: bool) {
        self.buf.push_str(if b { "true" } else { "false" });
    }

    pub fn null(&mut self) {
        self.buf.push_str("null");
    }

    /// `Some(v)` written by `f`, `None` written as `null`.
    pub fn opt<T>(&mut self, v: Option<T>, f: impl FnOnce(&mut Out, T)) {
        match v {
            Some(v) => f(self, v),
            None => self.null(),
        }
    }

    /// Splice in a document produced elsewhere — a cached response body, say.
    /// The caller owns its validity; nothing here checks it.
    pub fn raw(&mut self, json: &str) {
        self.buf.push_str(json);
    }
}

/// The inside of an object.
pub struct ObjOut<'a> {
    out: &'a mut Out,
    first: bool,
}

impl ObjOut<'_> {
    /// One `"key": value` pair; the closure writes the value.
    pub fn field(&mut self, key: &str, f: impl FnOnce(&mut Out)) {
        if !self.first {
            self.out.buf.push(',');
        }
        self.first = false;
        escape_into(key, &mut self.out.buf);
        self.out.buf.push(':');
        f(self.out);
    }

    pub fn str_field(&mut self, key: &str, value: &str) {
        self.field(key, |v| v.str(value));
    }

    pub fn int_field(&mut self, key: &str, value: i64) {
        self.field(key, |v| v.int(value));
    }

    pub fn bool_field(&mut self, key: &str, value: bool) {
        self.field(key, |v| v.bool(value));
    }
}

/// The inside of an array.
pub struct ArrOut<'a> {
    out: &'a mut Out,
    first: bool,
}

impl ArrOut<'_> {
    /// One element; the closure writes it.
    pub fn item(&mut self, f: impl FnOnce(&mut Out)) {
        if !self.first {
            self.out.buf.push(',');
        }
        self.first = false;
        f(self.out);
    }

    pub fn str_item(&mut self, value: &str) {
        self.item(|v| v.str(value));
    }
}

/// Build a document in one expression.
pub fn to_string(f: impl FnOnce(&mut Out)) -> String {
    let mut out = Out::new();
    f(&mut out);
    out.finish()
}

/// A JSON string literal, quotes included.
pub fn escape(s: &str) -> String {
    let mut buf = String::with_capacity(s.len() + 2);
    escape_into(s, &mut buf);
    buf
}

fn escape_into(s: &str, buf: &mut String) {
    buf.push('"');
    for c in s.chars() {
        match c {
            '"' => buf.push_str("\\\""),
            '\\' => buf.push_str("\\\\"),
            '\n' => buf.push_str("\\n"),
            '\r' => buf.push_str("\\r"),
            '\t' => buf.push_str("\\t"),
            // Control characters have no literal form. U+2028 and U+2029 are
            // legal in JSON and illegal in a JavaScript string literal, and
            // this document is read by a browser.
            c if (c as u32) < 0x20 || c == '\u{2028}' || c == '\u{2029}' => {
                let _ = write!(buf, "\\u{:04x}", c as u32);
            }
            c => buf.push(c),
        }
    }
    buf.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Json;

    #[test]
    fn writes_a_nested_document() {
        let s = to_string(|o| {
            o.obj(|o| {
                o.str_field("name", "Zee Shaman");
                o.field("rates", |v| {
                    v.obj(|o| o.field("Burn Mage", |v| v.round(0.512_345, 4)))
                });
                o.field("cards", |v| {
                    v.arr(|a| {
                        a.item(|v| {
                            v.arr(|a| {
                                a.str_item("Fireball");
                                a.item(|v| v.int(2));
                            })
                        })
                    })
                });
            })
        });
        assert_eq!(
            s,
            r#"{"name":"Zee Shaman","rates":{"Burn Mage":0.5123},"cards":[["Fireball",2]]}"#
        );
    }

    #[test]
    fn everything_written_parses_back() {
        // The one property that matters: whatever this emits, the reader —
        // and therefore a browser — accepts.
        let s = to_string(|o| {
            o.obj(|o| {
                o.str_field("quote", "he said \"no\"\n\ttabbed\\");
                o.str_field("cyrillic", "Колода");
                o.str_field("control", "\u{1}\u{2028}");
                o.field("empty_obj", |v| v.obj(|_| {}));
                o.field("empty_arr", |v| v.arr(|_| {}));
                o.field("nan", |v| v.num(f64::NAN));
                o.field("missing", |v| v.opt(None::<f64>, |o, n| o.num(n)));
                o.field("present", |v| v.opt(Some(0.5), |o, n| o.num(n)));
                o.bool_field("ok", true);
                o.int_field("n", -7);
            })
        });
        let doc = Json::parse(&s).expect("what the writer emits must parse");
        assert_eq!(doc.str_or_empty("quote"), "he said \"no\"\n\ttabbed\\");
        assert_eq!(doc.str_or_empty("cyrillic"), "Колода");
        assert_eq!(doc.str_or_empty("control"), "\u{1}\u{2028}");
        assert_eq!(doc.get("nan"), Some(&Json::Null));
        assert_eq!(doc.get("missing"), Some(&Json::Null));
        assert_eq!(doc.get("present").and_then(Json::as_f64), Some(0.5));
        assert_eq!(doc.i64_or_zero("n"), -7);
    }

    #[test]
    fn whole_numbers_are_written_as_integers() {
        // `1.0` in a count field reads as a rounding accident to whoever is
        // looking at the response.
        assert_eq!(to_string(|o| o.num(3.0)), "3");
        assert_eq!(to_string(|o| o.round(0.5, 4)), "0.5");
        assert_eq!(to_string(|o| o.round(1.0 / 3.0, 3)), "0.333");
    }
}
