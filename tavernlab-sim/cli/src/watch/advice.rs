//! What the watcher has to say, before it is put into words.
//!
//! The advice is read in two places -- the terminal command prints it, the
//! app's Live tab renders it -- and the app is bilingual. A sentence composed
//! here would be a sentence in one language arriving on a screen that may be
//! running in the other; `serve::api` already states that rule for the coach's
//! notes, and this is the same rule.
//!
//! So nothing here is prose. A [`Line`] is a key and the values that fill it
//! in, and whoever reads it writes it out from `locales/{lang}.json` in the
//! language they are running in. The two front ends cannot drift apart,
//! because there is nothing to drift: the words exist once, as data, beside
//! every other string the app shows.

use std::collections::BTreeMap;

/// A value a line is filled in with.
#[derive(Clone, Debug)]
pub enum Arg {
    /// A literal -- a number, a card name, a path. Card names are the
    /// corpus's own and are not translated, here or anywhere else in the app.
    Text(String),
    /// Another key, written out in the reader's own language. Classes come
    /// through here: `class.MAGE` is a string the app already has.
    Key(&'static str),
    /// A whole phrase of its own, with its own values. What a plan line's
    /// target is: "your Chillwind Yeti" inflects differently in the two
    /// languages, so it is written as one phrase rather than glued together
    /// from a possessive and a name.
    Nested(Box<Line>),
}

impl From<String> for Arg {
    fn from(s: String) -> Arg {
        Arg::Text(s)
    }
}

impl From<&str> for Arg {
    fn from(s: &str) -> Arg {
        Arg::Text(s.to_string())
    }
}

impl From<i64> for Arg {
    fn from(n: i64) -> Arg {
        Arg::Text(n.to_string())
    }
}

impl From<Line> for Arg {
    fn from(l: Line) -> Arg {
        Arg::Nested(Box::new(l))
    }
}

/// One line: a key and what fills it in.
#[derive(Clone, Debug)]
pub struct Line {
    pub key: &'static str,
    pub args: Vec<(&'static str, Arg)>,
}

impl Line {
    pub fn new(key: &'static str) -> Line {
        Line {
            key,
            args: Vec::new(),
        }
    }

    /// Chained so a line reads as one expression at the call site.
    pub fn with(mut self, name: &'static str, value: impl Into<Arg>) -> Line {
        self.args.push((name, value.into()));
        self
    }
}

/// One heading and the lines under it.
#[derive(Clone)]
pub struct Section {
    pub key: &'static str,
    pub lines: Vec<Line>,
}

/// Everything the tracker can currently say.
///
/// The title is a list of parts rather than one line: what is known about a
/// position varies (whose turn, whether both classes are visible, whether the
/// game is over), and a title assembled from sentence fragments is the thing
/// that does not translate. Each part is a whole phrase; the reader joins
/// them.
///
/// A heading with no lines under it is dropped rather than kept empty: an
/// empty section reads as "nothing to advise", which is a different claim
/// from "this does not apply right now".
#[derive(Clone)]
pub struct Advice {
    pub title: Vec<Line>,
    pub sections: Vec<Section>,
}

/// The app's own strings, for a reader that has to write the words itself.
///
/// The same `locales/{lang}.json` the front end fetches, so the terminal and
/// the page say the same thing rather than merely mean it.
pub struct Locale {
    strings: BTreeMap<String, String>,
}

impl Locale {
    /// Read a language, falling back to an empty table.
    ///
    /// A missing file is not an error worth stopping for: every key renders
    /// as itself, which is ugly and still says which line is which. Losing
    /// the advice entirely because a data file moved would be worse.
    pub fn load(root: &std::path::Path, lang: &str) -> Locale {
        let lang = if lang.is_empty() { "uk" } else { lang };
        let path = root.join("locales").join(format!("{lang}.json"));
        let mut strings = BTreeMap::new();
        if let Ok(src) = std::fs::read_to_string(&path)
            && let Ok(doc) = tavernlab_json::Json::parse(&src)
            && let Some(obj) = doc.as_object()
        {
            for (k, v) in obj {
                if let Some(s) = v.as_str() {
                    strings.insert(k.clone(), s.to_string());
                }
            }
        }
        Locale { strings }
    }

    /// One key, or the key itself when the table does not have it.
    pub fn get(&self, key: &str) -> String {
        self.strings
            .get(key)
            .cloned()
            .unwrap_or_else(|| key.to_string())
    }

    /// One line, written out.
    pub fn line(&self, line: &Line) -> String {
        let mut out = self.get(line.key);
        for (name, value) in &line.args {
            let text = match value {
                Arg::Text(s) => s.clone(),
                Arg::Key(k) => self.get(k),
                Arg::Nested(l) => self.line(l),
            };
            out = out.replace(&format!("{{{name}}}"), &text);
        }
        out
    }

    /// The title, as one line. `—` is punctuation both languages share, so
    /// joining the parts with it is layout rather than grammar.
    pub fn title(&self, advice: &Advice) -> String {
        advice
            .title
            .iter()
            .map(|l| self.line(l))
            .collect::<Vec<_>>()
            .join(" — ")
    }
}

/// One line as the JSON the page reads: the key, and the values under `p`.
///
/// A nested value is an object of the same shape, which is how a plan line
/// carries a target that has its own key and its own values.
pub fn write_line(o: &mut tavernlab_json::Out, line: &Line) {
    o.obj(|o| {
        o.str_field("k", line.key);
        if line.args.is_empty() {
            return;
        }
        o.field("p", |v| {
            v.obj(|o| {
                for (name, value) in &line.args {
                    match value {
                        Arg::Text(s) => o.str_field(name, s),
                        Arg::Key(k) => o.field(name, |v| v.obj(|o| o.str_field("k", k))),
                        Arg::Nested(l) => o.field(name, |v| write_line(v, l)),
                    }
                }
            })
        });
    });
}

/// `title`/`sections` as the same JSON `/api/live` sends (via [`write_line`]
/// and the section shape `serve::api::live_read` builds), packed into two
/// strings a `history::AdviceEntry` row can hold. Meant for a record kept
/// past the poll that produced it, not for the live response itself -- see
/// `history.rs`'s module docs for why this exists at all: without it, a
/// game worth discussing afterward has only its outcome, not the advice
/// that was actually shown while it was played.
pub fn to_json(advice: &Advice) -> (String, String) {
    let title = tavernlab_json::to_string(|o| {
        o.arr(|a| {
            for part in &advice.title {
                a.item(|v| write_line(v, part));
            }
        })
    });
    let sections = tavernlab_json::to_string(|o| {
        o.arr(|a| {
            for s in advice.sections.iter().filter(|s| !s.lines.is_empty()) {
                a.item(|v| {
                    v.obj(|o| {
                        o.str_field("heading", s.key);
                        o.field("lines", |v| {
                            v.arr(|a| {
                                for line in &s.lines {
                                    a.item(|v| write_line(v, line));
                                }
                            })
                        });
                    })
                });
            }
        })
    });
    (title, sections)
}
