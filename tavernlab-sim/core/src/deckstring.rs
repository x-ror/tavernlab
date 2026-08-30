//! Deck codes: the format the game and every deck site exchange decks in.
//!
//! A deck code is base64 of `[0, version, format, heroes, 1x, 2x, nx,
//! (sideboard)]`, every number a varint, every card a Blizzard `dbfId`. That
//! makes it an import/export concern rather than a rule, which is why the ids
//! it speaks live in [`CardInfo`](crate::cards::CardInfo) rather than in the
//! hot table.
//!
//! [`decode`] takes what is actually on a player's clipboard rather than a bare
//! code. Every deck site hands out a commented block:
//!
//! ```text
//! ### Zee Shaman
//! # Клас: Шаман
//! # 2x (0) Учениця відьми
//! AAECAaoICsmeBsODB9C/B...
//! # Колода доступна тут: https://hsreplay.net/decks/...
//! ```
//!
//! and a player pastes the whole thing. The code is found by *parsing*
//! candidate lines rather than by looking at their shape: a block also
//! contains base64-looking words, and a length check would happily pick one
//! of them.

use crate::cards::{CardId, Class, Formats, Kind, by_dbf, is_implemented};

/// What a deck code says, before any of it is looked up in the corpus.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decoded {
    /// The version byte. Only 1 has ever existed.
    pub version: u32,
    /// Blizzard's `FormatType`: 1 Wild, 2 Standard, 3 the retired Classic,
    /// 4 Twist. Kept raw; [`format_of`] maps the two we can build a pool for.
    pub format: u32,
    pub heroes: Vec<u32>,
    /// `(dbfId, copies)`.
    pub cards: Vec<(u32, u32)>,
    /// `(dbfId, copies, owner dbfId)` — the sideboard section, which only
    /// Zilliax-style "assembled" cards and Commander Beatrix use.
    pub sideboards: Vec<(u32, u32, u32)>,
}

/// Why a paste could not be read as a deck code.
///
/// Each variant names the real problem, so a UI can repeat it to the player
/// instead of relaying a base64 complaint about Cyrillic comments.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CodeError {
    /// Nothing was pasted.
    Empty,
    /// There were only comment lines.
    OnlyComments,
    /// Something was pasted, but no line in it parses as a deck code.
    NoCode,
    /// A bare code that starts right but ends early.
    Truncated,
    /// A bare code whose header is not a deck code's.
    BadHeader,
}

impl CodeError {
    /// A stable key for the problem, for a caller that has to say this in a
    /// language of its own. The `Display` text below is the English rendering
    /// of the same thing, and is what a log or a CLI prints.
    pub fn code(self) -> &'static str {
        match self {
            CodeError::Empty => "empty",
            CodeError::OnlyComments => "only_comments",
            CodeError::NoCode => "no_code",
            CodeError::Truncated => "truncated",
            CodeError::BadHeader => "bad_header",
        }
    }
}

impl core::fmt::Display for CodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            CodeError::Empty => "nothing was pasted",
            CodeError::OnlyComments => "this is all comments — the deck code itself is missing",
            CodeError::NoCode => "no deck code anywhere in the pasted text",
            CodeError::Truncated => "the deck code ends early",
            CodeError::BadHeader => "this is not a deck code",
        })
    }
}

/// The `standard` / `wild` pool a deck code's format byte names, if it names
/// one we can build. 0 (unknown), 3 (the retired Classic format) and 4 (Twist,
/// a seasonal pool that cannot be derived from set codes) name none.
pub fn format_of(format_byte: u32) -> Option<Formats> {
    match format_byte {
        1 => Some(Formats::WILD),
        2 => Some(Formats::STANDARD),
        _ => None,
    }
}

/// The bare deck code inside whatever the user pasted.
pub fn extract(text: &str) -> Result<&str, CodeError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(CodeError::Empty);
    }
    if plausible(text).is_some() {
        return Ok(text);
    }
    let body: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();
    for line in &body {
        if plausible(line).is_some() {
            return Ok(line);
        }
    }
    if body.is_empty() {
        return Err(CodeError::OnlyComments);
    }
    Err(CodeError::NoCode)
}

/// The `### Name` title a paste carries, if it has one.
pub fn deck_name(text: &str) -> Option<&str> {
    text.lines()
        .map(str::trim)
        .filter_map(|l| l.strip_prefix("###"))
        .map(str::trim)
        .find(|n| !n.is_empty())
}

/// Decode a bare code or a pasted block around one.
pub fn decode(text: &str) -> Result<Decoded, CodeError> {
    let code = extract(text)?;
    let bytes = b64_decode(code).ok_or(CodeError::BadHeader)?;
    parse(&bytes)
}

/// A code that parses as a deck, or `None`.
///
/// A deck has a hero and cards; short random base64 can survive the varint
/// reader by accident, but it cannot survive that.
fn plausible(code: &str) -> Option<Decoded> {
    let bytes = b64_decode(code)?;
    let d = parse(&bytes).ok()?;
    (d.version == 1 && !d.heroes.is_empty() && !d.cards.is_empty()).then_some(d)
}

fn parse(data: &[u8]) -> Result<Decoded, CodeError> {
    let mut r = Reader { data, pos: 0 };
    if r.varint()? != 0 {
        return Err(CodeError::BadHeader);
    }
    let version = r.varint()?;
    let format = r.varint()?;
    let heroes = r.list()?;
    let mut cards = Vec::new();
    for dbf in r.list()? {
        cards.push((dbf, 1));
    }
    for dbf in r.list()? {
        cards.push((dbf, 2));
    }
    for _ in 0..r.varint()? {
        let dbf = r.varint()?;
        cards.push((dbf, r.varint()?));
    }
    let mut sideboards = Vec::new();
    if !r.eof() && r.varint()? == 1 {
        for _ in 0..r.varint()? {
            sideboards.push((r.varint()?, 1, r.varint()?));
        }
        for _ in 0..r.varint()? {
            sideboards.push((r.varint()?, 2, r.varint()?));
        }
        for _ in 0..r.varint()? {
            let dbf = r.varint()?;
            let n = r.varint()?;
            sideboards.push((dbf, n, r.varint()?));
        }
    }
    Ok(Decoded {
        version,
        format,
        heroes,
        cards,
        sideboards,
    })
}

/// Encode a deck back into a code, so a gauntlet deck can be taken into the
/// game. `cards` is `(dbfId, copies)`.
pub fn encode(hero_dbf: u32, cards: &[(u32, u32)], format_byte: u32) -> String {
    let mut ones: Vec<u32> = cards
        .iter()
        .filter(|(_, n)| *n == 1)
        .map(|(d, _)| *d)
        .collect();
    let mut twos: Vec<u32> = cards
        .iter()
        .filter(|(_, n)| *n == 2)
        .map(|(d, _)| *d)
        .collect();
    let mut more: Vec<(u32, u32)> = cards.iter().copied().filter(|(_, n)| *n > 2).collect();
    ones.sort_unstable();
    twos.sort_unstable();
    more.sort_unstable();

    let mut data = Vec::new();
    for n in [0, 1, format_byte, 1, hero_dbf, ones.len() as u32] {
        put_varint(&mut data, n);
    }
    for d in &ones {
        put_varint(&mut data, *d);
    }
    put_varint(&mut data, twos.len() as u32);
    for d in &twos {
        put_varint(&mut data, *d);
    }
    put_varint(&mut data, more.len() as u32);
    for (d, n) in &more {
        put_varint(&mut data, *d);
        put_varint(&mut data, *n);
    }
    put_varint(&mut data, 0); // no sideboard section
    b64_encode(&data)
}

/// Collapse a list of card instances into `(dbfId, copies)` and encode it.
///
/// The optimizer works in ids, one entry per copy; a deck code talks in
/// counts. This is the bridge so a measured swap can be pasted back into
/// the game as a list.
pub fn encode_ids(hero_dbf: u32, ids: &[CardId], format_byte: u32) -> String {
    let mut cards: Vec<(u32, u32)> = Vec::new();
    for id in ids {
        let dbf = id.info().dbf;
        match cards.iter_mut().find(|(d, _)| *d == dbf) {
            Some((_, n)) => *n += 1,
            None => cards.push((dbf, 1)),
        }
    }
    encode(hero_dbf, &cards, format_byte)
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn varint(&mut self) -> Result<u32, CodeError> {
        let mut shift = 0;
        let mut out: u64 = 0;
        loop {
            let b = *self.data.get(self.pos).ok_or(CodeError::Truncated)?;
            self.pos += 1;
            out |= ((b & 0x7F) as u64) << shift;
            if b & 0x80 == 0 {
                // A dbf id is a u32 and a count is tiny; anything wider is a
                // stream that is not a deck code, not a deck with a huge
                // number in it.
                return u32::try_from(out).map_err(|_| CodeError::BadHeader);
            }
            shift += 7;
            if shift > 35 {
                return Err(CodeError::BadHeader);
            }
        }
    }

    /// A length-prefixed run of varints.
    fn list(&mut self) -> Result<Vec<u32>, CodeError> {
        let n = self.varint()?;
        // A length is bounded by what is left to read: a corrupt code must
        // not make this reserve gigabytes before failing.
        if n as usize > self.data.len() - self.pos {
            return Err(CodeError::Truncated);
        }
        (0..n).map(|_| self.varint()).collect()
    }

    fn eof(&self) -> bool {
        self.pos >= self.data.len()
    }
}

fn put_varint(out: &mut Vec<u8>, mut n: u32) {
    loop {
        let b = (n & 0x7F) as u8;
        n >>= 7;
        out.push(if n != 0 { b | 0x80 } else { b });
        if n == 0 {
            return;
        }
    }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            B64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Strict base64: anything outside the alphabet fails rather than being
/// skipped. That strictness is what lets [`plausible`] use a successful
/// decode as evidence that a line is a deck code and not prose.
fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let bytes = s.as_bytes();
    if bytes.is_empty() || bytes.len() % 4 != 0 {
        return None;
    }
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a') as u32 + 26,
            b'0'..=b'9' => (c - b'0') as u32 + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        })
    };
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks(4) {
        let pad = chunk.iter().filter(|c| **c == b'=').count();
        if pad > 2 || (pad > 0 && !std::ptr::eq(chunk, &bytes[bytes.len() - 4..])) {
            return None; // padding anywhere but at the very end
        }
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= if c == b'=' {
                if i < 2 {
                    return None;
                }
                0
            } else {
                val(c)?
            } << (18 - 6 * i);
        }
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Some(out)
}

// ------------------------------------------------------------- resolution

/// A deck code read against the card table.
///
/// The four ways a slot can fail are kept apart on purpose, because they are
/// four different answers to "why can't you simulate my deck":
///
/// * `unimplemented` — a real card in this format that the engine does not
///   understand yet. The honest response is to name it and stop, never to
///   play the deck without it.
/// * `illegal` — a card the engine knows, in a format it is not legal in.
/// * `missing` — a dbf id that is in no corpus at all: a code from a newer
///   patch than the card table.
/// * `not_deckable` — a dbf id for something that cannot go in a deck.
#[derive(Clone, Debug)]
pub struct Resolved {
    pub class: Class,
    /// The pool the code asked for, `None` when its format byte names one we
    /// cannot build (Twist, Classic, unknown).
    pub format: Option<Formats>,
    /// `(name, copies)`, sorted by name, sideboard folded in.
    pub cards: Vec<(&'static str, u32)>,
    /// The deck as card ids, `copies` entries each. Empty unless the deck is
    /// [`playable`](Resolved::playable).
    pub ids: Vec<CardId>,
    pub unimplemented: Vec<&'static str>,
    pub illegal: Vec<&'static str>,
    pub missing: Vec<u32>,
    pub not_deckable: Vec<&'static str>,
}

impl Resolved {
    /// Whether the engine can field this list as it stands.
    pub fn playable(&self) -> bool {
        self.unimplemented.is_empty()
            && self.illegal.is_empty()
            && self.missing.is_empty()
            && self.not_deckable.is_empty()
    }

    /// Copies requested in total.
    pub fn total(&self) -> u32 {
        self.cards.iter().map(|(_, n)| n).sum()
    }
}

/// Why a code could not be resolved at all — as opposed to resolving into a
/// deck with problems, which [`Resolved`] describes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResolveError {
    Code(CodeError),
    /// Neither the hero nor any card in the list names a class.
    NoClass,
}

impl ResolveError {
    /// A stable key for the problem — see [`CodeError::code`].
    pub fn code(self) -> &'static str {
        match self {
            ResolveError::Code(e) => e.code(),
            ResolveError::NoClass => "no_class",
        }
    }
}

impl core::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ResolveError::Code(e) => write!(f, "invalid deck code: {e}"),
            ResolveError::NoClass => f.write_str("cannot tell which class this deck is"),
        }
    }
}

/// Read a pasted deck code against the card table.
///
/// Never fails because of a card: an unknown, unimplemented or illegal slot
/// is *reported*, and the caller decides. Only a code that will not decode or
/// names no class at all is an error.
pub fn resolve(text: &str) -> Result<Resolved, ResolveError> {
    let d = decode(text).map_err(ResolveError::Code)?;
    let format = format_of(d.format);

    // The class comes from the hero portrait when it has one. A neutral or
    // unknown hero falls back to a vote among the deck's own class cards,
    // which is what a Whizbang or a hand-built code needs.
    let hero_class = d
        .heroes
        .first()
        .and_then(|dbf| by_dbf(*dbf))
        .map(|c| c.def().class())
        .filter(|c| *c != Class::Neutral);

    // Beatrix runs her sideboard as ten copies of one card. The count in the
    // code is 1, and the deck the simulator has to field is the other one.
    let beatrix = d.cards.iter().any(|(dbf, _)| {
        by_dbf(*dbf)
            .map(|c| c.name() == "Commander Beatrix")
            .unwrap_or(false)
    });

    let slots = d.cards.iter().map(|(dbf, n)| (*dbf, *n)).chain(
        d.sideboards
            .iter()
            .map(|(dbf, n, _)| (*dbf, if beatrix { 10 } else { *n })),
    );

    let mut r = Resolved {
        class: Class::Neutral,
        format,
        cards: Vec::new(),
        ids: Vec::new(),
        unimplemented: Vec::new(),
        illegal: Vec::new(),
        missing: Vec::new(),
        not_deckable: Vec::new(),
    };
    let mut votes: [(Class, u32); 12] = [
        (Class::DeathKnight, 0),
        (Class::DemonHunter, 0),
        (Class::Druid, 0),
        (Class::Hunter, 0),
        (Class::Mage, 0),
        (Class::Paladin, 0),
        (Class::Priest, 0),
        (Class::Rogue, 0),
        (Class::Shaman, 0),
        (Class::Warlock, 0),
        (Class::Warrior, 0),
        (Class::Neutral, 0),
    ];

    for (dbf, n) in slots {
        let Some(id) = by_dbf(dbf) else {
            r.missing.push(dbf);
            continue;
        };
        let def = id.def();
        if !def.deckable() || def.kind() == Kind::HeroPower {
            r.not_deckable.push(id.name());
            continue;
        }
        if !is_implemented(id) {
            r.unimplemented.push(id.name());
        }
        if let Some(f) = format
            && !def.formats.has(f)
        {
            r.illegal.push(id.name());
        }
        if let Some(v) = votes.iter_mut().find(|(c, _)| *c == def.class()) {
            v.1 += n;
        }
        r.cards.push((id.name(), n));
    }

    r.class = match hero_class {
        Some(c) => c,
        None => {
            votes[..11]
                .iter()
                .copied()
                .filter(|(_, n)| *n > 0)
                .max_by_key(|(_, n)| *n)
                .ok_or(ResolveError::NoClass)?
                .0
        }
    };

    r.cards.sort_unstable();
    dedup_sorted(&mut r.unimplemented);
    dedup_sorted(&mut r.illegal);
    dedup_sorted(&mut r.not_deckable);
    if r.playable() {
        for (name, n) in &r.cards {
            if let Some(id) = crate::cards::by_name(name) {
                for _ in 0..*n {
                    r.ids.push(id);
                }
            }
        }
    }
    Ok(r)
}

fn dedup_sorted(v: &mut Vec<&'static str>) {
    v.sort_unstable();
    v.dedup();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cards::by_name;

    /// A real Standard code: "Zee Shaman" from the 2026 gauntlet's own era,
    /// round-tripped through the encoder so the test does not depend on a
    /// site's export staying online.
    fn a_real_deck() -> (u32, Vec<(u32, u32)>) {
        let hero = by_name("Thrall").map(|c| c.info().dbf).unwrap_or(1066);
        let cards = ["Fireball", "Frostbolt", "Arcane Intellect"]
            .iter()
            .map(|n| (by_name(n).unwrap().info().dbf, 2))
            .collect();
        (hero, cards)
    }

    /// A code as a deck site actually hands it out. Every other case here
    /// goes through this module's own encoder, which cannot catch an encoding
    /// the format uses and this decoder does not.
    #[test]
    fn a_real_site_code_decodes_the_way_the_reference_did() {
        let code = "AAECAaoICsmeBsODB9C/B/nDB4LUB5vUB8/bB9DbB4jdB9/lBwqe1ATt5gbgnQexsAePvge1wAfJwAfJ2wfI5Qfm/QcAAA==";
        let d = decode(code).expect("a real deck code");
        assert_eq!((d.version, d.format), (1, 2));
        assert_eq!(d.heroes, vec![1066]);
        assert!(d.sideboards.is_empty());
        let mut got = d.cards.clone();
        got.sort_unstable();
        let mut want: Vec<(u32, u32)> = vec![
            (102217, 1),
            (115139, 1),
            (122832, 1),
            (123385, 1),
            (125442, 1),
            (125467, 1),
            (126415, 1),
            (126416, 1),
            (126600, 1),
            (127711, 1),
            (76318, 2),
            (111469, 2),
            (118496, 2),
            (120881, 2),
            (122639, 2),
            (122933, 2),
            (122953, 2),
            (126409, 2),
            (127688, 2),
            (130790, 2),
        ];
        want.sort_unstable();
        assert_eq!(got, want);

        let r = resolve(code).expect("it resolves");
        assert_eq!(r.class, Class::Shaman);
        assert_eq!(r.total(), 30);
    }

    #[test]
    fn encode_ids_collapses_copies() {
        let (hero, _) = a_real_deck();
        let fireball = by_name("Fireball").unwrap();
        let frostbolt = by_name("Frostbolt").unwrap();
        let code = encode_ids(hero, &[fireball, fireball, frostbolt], 2);
        let d = decode(&code).expect("the collapsed list encodes");
        let mut got = d.cards;
        got.sort_unstable();
        let mut want = vec![(fireball.info().dbf, 2), (frostbolt.info().dbf, 1)];
        want.sort_unstable();
        assert_eq!(got, want);
        assert_eq!(d.heroes, vec![hero]);
        assert_eq!(d.format, 2);
    }

    #[test]
    fn a_code_round_trips() {
        let (hero, cards) = a_real_deck();
        let code = encode(hero, &cards, 2);
        let d = decode(&code).expect("our own code decodes");
        assert_eq!(d.version, 1);
        assert_eq!(d.format, 2);
        assert_eq!(d.heroes, vec![hero]);
        let mut got = d.cards.clone();
        got.sort_unstable();
        let mut want = cards.clone();
        want.sort_unstable();
        assert_eq!(got, want);
    }

    #[test]
    fn counts_above_two_survive_the_round_trip() {
        // Beatrix's ten copies go through the `nx` section, which is the one
        // an encoder is most likely to get wrong because normal decks never
        // use it.
        let (hero, _) = a_real_deck();
        let fireball = by_name("Fireball").unwrap().info().dbf;
        let code = encode(hero, &[(fireball, 10)], 2);
        assert_eq!(decode(&code).unwrap().cards, vec![(fireball, 10)]);
    }

    #[test]
    fn a_pasted_block_yields_the_code_inside_it() {
        let (hero, cards) = a_real_deck();
        let code = encode(hero, &cards, 2);
        let block = format!(
            "### Zee Shaman\n# Клас: Шаман\n# 2x (0) Учениця відьми\n{code}\n\
             # Колода доступна тут: https://hsreplay.net/decks/abc/\n"
        );
        assert_eq!(extract(&block), Ok(code.as_str()));
        assert_eq!(deck_name(&block), Some("Zee Shaman"));
        // The URL line is base64-looking prose and must not win.
        assert_eq!(decode(&block).unwrap().heroes, vec![hero]);
    }

    #[test]
    fn every_failure_has_a_stable_key_for_a_translator() {
        // The UI is bilingual and composes its own sentence; the key is what
        // it keys on, so it must exist for every variant and stay distinct.
        let keys: Vec<&str> = [
            CodeError::Empty,
            CodeError::OnlyComments,
            CodeError::NoCode,
            CodeError::Truncated,
            CodeError::BadHeader,
        ]
        .iter()
        .map(|e| e.code())
        .collect();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), keys.len(), "two failures share a key");
        assert_eq!(ResolveError::Code(CodeError::NoCode).code(), "no_code");
        assert_eq!(ResolveError::NoClass.code(), "no_class");
    }

    #[test]
    fn a_paste_without_a_code_names_the_real_problem() {
        assert_eq!(extract(""), Err(CodeError::Empty));
        assert_eq!(extract("   \n  "), Err(CodeError::Empty));
        assert_eq!(
            extract("### Just a title\n# and a comment"),
            Err(CodeError::OnlyComments)
        );
        assert_eq!(extract("this is not a deck code"), Err(CodeError::NoCode));
    }

    #[test]
    fn a_truncated_code_does_not_panic_or_allocate_wildly() {
        // The length prefix of a corrupt code is attacker-shaped input even
        // on loopback: it says "read four billion varints".
        let bad = b64_encode(&[0x00, 0x01, 0x02, 0xFF, 0xFF, 0xFF, 0xFF, 0x0F]);
        assert!(parse(&b64_decode(&bad).unwrap()).is_err());
    }

    #[test]
    fn base64_is_strict() {
        assert!(b64_decode("not base64!").is_none());
        assert!(b64_decode("AAA").is_none()); // unpadded
        assert!(b64_decode("A=AA").is_none()); // padding in the middle
        assert_eq!(b64_decode("AAAA"), Some(vec![0, 0, 0]));
        assert_eq!(b64_decode("AAA="), Some(vec![0, 0]));
        assert_eq!(b64_decode("AA=="), Some(vec![0]));
    }

    #[test]
    fn resolution_names_what_it_cannot_play() {
        let hero = by_name("Jaina Proudmoore")
            .map(|c| c.info().dbf)
            .unwrap_or_else(|| by_name("Fireball").unwrap().info().dbf);
        let fireball = by_name("Fireball").unwrap().info().dbf;
        // A real card the engine does not implement, taken from the table so
        // the test cannot rot when it is implemented.
        let unimpl = crate::cards::all()
            .find(|c| {
                let d = c.def();
                d.collectible
                    && d.deckable()
                    && d.formats.has(Formats::STANDARD)
                    && !is_implemented(*c)
            })
            .expect("the table has unimplemented Standard cards");
        let code = encode(
            hero,
            &[(fireball, 2), (unimpl.info().dbf, 1), (999_999, 1)],
            2,
        );
        let r = resolve(&code).expect("the code itself is valid");
        assert_eq!(r.format, Some(Formats::STANDARD));
        assert!(!r.playable());
        assert_eq!(r.unimplemented, vec![unimpl.name()]);
        assert_eq!(r.missing, vec![999_999]);
        assert!(
            r.ids.is_empty(),
            "an unplayable list must not be handed to the engine"
        );
    }

    #[test]
    fn a_playable_list_comes_back_as_card_ids() {
        let hero = by_name("Jaina Proudmoore").unwrap().info().dbf;
        let fireball = by_name("Fireball").unwrap();
        let code = encode(hero, &[(fireball.info().dbf, 2)], 2);
        let r = resolve(&code).unwrap();
        assert!(r.playable());
        assert_eq!(r.class, Class::Mage);
        assert_eq!(r.ids, vec![fireball, fireball]);
        assert_eq!(r.total(), 2);
    }

    #[test]
    fn a_wild_only_card_in_a_standard_code_is_illegal_not_missing() {
        let wild_only = crate::cards::all()
            .find(|c| {
                let d = c.def();
                d.collectible
                    && d.deckable()
                    && d.formats.has(Formats::WILD)
                    && !d.formats.has(Formats::STANDARD)
                    && is_implemented(*c)
            })
            .expect("the table has implemented Wild-only cards");
        let hero = by_name("Jaina Proudmoore").unwrap().info().dbf;
        let code = encode(hero, &[(wild_only.info().dbf, 1)], 2);
        let r = resolve(&code).unwrap();
        assert_eq!(r.illegal, vec![wild_only.name()]);
        assert!(r.missing.is_empty());
        // The same list is fine in Wild.
        let wild = resolve(&encode(hero, &[(wild_only.info().dbf, 1)], 1)).unwrap();
        assert!(wild.illegal.is_empty());
    }

    #[test]
    fn the_class_falls_back_to_a_vote_when_the_hero_is_not_one() {
        let fireball = by_name("Fireball").unwrap();
        let wisp = by_name("Wisp").expect("Wisp is in the corpus");
        let code = encode(
            wisp.info().dbf, // a neutral minion where a hero portrait belongs
            &[(fireball.info().dbf, 2), (wisp.info().dbf, 2)],
            2,
        );
        assert_eq!(resolve(&code).unwrap().class, Class::Mage);
    }

    #[test]
    fn a_format_byte_we_cannot_build_a_pool_for_is_none_not_standard() {
        // Twist is a seasonal pool that cannot be derived from set codes;
        // reading it as Standard would silently score against the wrong one.
        assert_eq!(format_of(4), None);
        assert_eq!(format_of(3), None);
        assert_eq!(format_of(0), None);
        assert_eq!(format_of(1), Some(Formats::WILD));
        assert_eq!(format_of(2), Some(Formats::STANDARD));
    }
}
