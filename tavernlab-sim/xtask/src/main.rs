//! Build-time tooling.
//!
//! `cargo run -p xtask -- cards` reads the card corpora in `data/` and writes
//! `core/src/cards/table.rs`.
//!
//! `cargo run -p xtask -- wild-gauntlet` rebuilds `data/gauntlet_wild.json`
//! from the cards the engine implements in Wild — see [`wild`].
//!
//! The card database is baked into the binary as Rust source rather than parsed
//! at startup. Three reasons: a batch run starting thousands of games should not
//! re-parse 5 MB of JSON; the table ends up in read-only memory shared by every
//! thread, rather than per worker; and it removes the last excuse for a
//! serialisation dependency, which Smart App Control would block anyway.
//!
//! The generator also emits the [`Keywords`] and [`Races`] bit constants it used
//! to encode the table. Hand-writing those in the engine and the encoding here
//! would be two lists that must agree forever; generating both from one list
//! means they cannot drift.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use tavernlab_json::Json;

mod backfill;
mod sets;
mod wild;

/// Mechanics that carry gameplay meaning, in bit order.
///
/// Bit position is stable: appending is safe, reordering silently rewrites
/// every card in the table.
const KEYWORDS: &[&str] = &[
    // Evergreen keywords the kernel checks directly.
    "TAUNT",
    "DIVINE_SHIELD",
    "CHARGE",
    "RUSH",
    "WINDFURY",
    "STEALTH",
    "LIFESTEAL",
    "POISONOUS",
    "ELUSIVE",
    "REBORN",
    "CANT_ATTACK",
    "IMMUNE",
    "CANT_BE_DESTROYED",
    "CANT_BE_SILENCED",
    "CANT_BE_FATIGUED",
    "FREEZE",
    "SILENCE",
    "SPELLPOWER",
    "OVERLOAD",
    "AURA",
    // Play-time hooks.
    "BATTLECRY",
    "DEATHRATTLE",
    "COMBO",
    "CHOOSE_ONE",
    "DISCOVER",
    "SECRET",
    "QUEST",
    "SIDE_QUEST",
    "START_OF_GAME_KEYWORD",
    "INSPIRE",
    "SPELLBURST",
    "FRENZY",
    "ENRAGED",
    "OVERKILL",
    "HONORABLE_KILL",
    "OUTCAST",
    "ECHO",
    "TWINSPELL",
    "TRADEABLE",
    "CORRUPT",
    "INFUSE",
    "MAGNETIC",
    "DREDGE",
    "FORGE",
    "EXCAVATE",
    "MANATHIRST",
    "QUICKDRAW",
    "OVERHEAL",
    "TITAN",
    "COLOSSAL",
    "STARSHIP",
    "STARSHIP_PIECE",
    "MINIATURIZE",
    "GIGANTIFY",
    "JADE_GOLEM",
    "ADJACENT_BUFF",
    "FORGETFUL",
    "AFFECTED_BY_SPELL_POWER",
    "ImmuneToSpellpower",
    "HEROPOWER_DAMAGE",
    "FINALE",
    // Rewind is never in `mech` -- the corpus carries it only in `kw`, which
    // also names keywords a card merely *references*, so it is read off the
    // text below instead. It is listed here for its bit and its constant.
    "REWIND",
];

/// Bits the engine reads that are not generated from [`KEYWORDS`].
///
/// They sit at the top of the word so that appending a keyword stays safe, and
/// they are named here because three places have to agree on the numbers: the
/// encoder below, the constants it emits, and the budget check.
const PREPARE_BIT: u32 = 62;
const TEXT_UNDERSTOOD_BIT: u32 = 63;

/// Mechanics the corpus spells differently from the engine.
///
/// `GameTag` defines each of these pairs as two names for one enum member, so
/// a dump emits whichever spelling the library calls canonical and the engine
/// has always known the other. Renaming the [`KEYWORDS`] entries would be the
/// obvious fix and the wrong one: bit position there is load-bearing, and the
/// constant names are what `behaviour.rs` reads. Translating on the way in
/// leaves both sides stable and puts the rename on record.
const MECHANIC_ALIASES: &[(&str, &str)] = &[
    ("MODULAR", "MAGNETIC"),
    ("START_OF_GAME", "START_OF_GAME_KEYWORD"),
    ("SIDEQUEST", "SIDE_QUEST"),
];

/// Whether this card's text prints Rewind as a leading keyword.
///
/// Keywords are printed first and capitalised, and the rules sentence after
/// them starts capitalised too -- "Rewind Battlecry: ...", "Rewind Deal 4
/// damage...", and Mister Clocksworth's "Rewind, Rewind, Rewind Battlecry:".
/// Used as an ordinary verb it is followed by a lowercase word instead:
/// "Rewind the card's effect", "Rewind to the start of your last turn". That
/// is the whole distinction, and it separates the seventeen cards that carry
/// the keyword from the two that only name it.
fn leads_with_rewind(text: &str) -> bool {
    let Some(rest) = text.strip_prefix("Rewind") else {
        return false;
    };
    match rest.chars().next() {
        // "Rewind, Rewind, Rewind ..."
        Some(',') => true,
        Some(' ') => rest[1..].chars().next().is_some_and(char::is_uppercase),
        _ => false,
    }
}

/// The bit [`KEYWORDS`] gave Rewind.
fn rewind_bit() -> u32 {
    KEYWORDS
        .iter()
        .position(|k| *k == "REWIND")
        .expect("KEYWORDS lists REWIND") as u32
}

/// The corpus's name for a mechanic, as the engine spells it.
fn alias(name: &str) -> &str {
    match MECHANIC_ALIASES.iter().find(|(from, _)| *from == name) {
        Some((_, to)) => to,
        None => name,
    }
}

/// Mechanics deliberately dropped, so the omission is a decision on record
/// rather than an oversight. These drive client presentation, deck-builder
/// filters or the tutorial AI, and none of them changes a rule.
const IGNORED_MECHANICS: &[&str] = &[
    "TRIGGER_VISUAL",
    "UNTOUCHABLE",
    "AI_MUST_PLAY",
    "DUNGEON_PASSIVE_BUFF",
    "EVIL_GLOW",
    "InvisibleDeathrattle",
    "APPEAR_FUNCTIONALLY_DEAD",
    "PUZZLE",
    "IGNORE_HIDE_STATS_FOR_BIG_CARD",
    "COLLECTIONMANAGER_FILTER_MANA_EVEN",
    "COLLECTIONMANAGER_FILTER_MANA_ODD",
    "MULTIPLY_BUFF_VALUE",
    "RECEIVES_DOUBLE_SPELLDAMAGE_BONUS",
    "FINISH_ATTACK_SPELL_ON_DAMAGE",
    "END_OF_TURN_TRIGGER",
    "DEATH_KNIGHT",
    "SPARE_PART",
    "GEARS",
    "JADE_LOTUS",
    "GRIMY_GOONS",
    "KABAL",
];

/// Minion tribes, in bit order. The first thirteen are the tribes cards
/// actually reference; the rest are flavour with no rules attached, kept only
/// so the encoding is lossless.
const RACES: &[&str] = &[
    "BEAST",
    "UNDEAD",
    "ELEMENTAL",
    "MECHANICAL",
    "DEMON",
    "DRAGON",
    "MURLOC",
    "PIRATE",
    "DRAENEI",
    "NAGA",
    "TOTEM",
    "QUILBOAR",
    "ALL",
    "ORC",
    "LOCK",
    "TROLL",
    "BLOODELF",
    "GNOME",
    "HUMAN",
    "NIGHTELF",
    "DWARF",
    "TAUREN",
];

const KINDS: &[&str] = &[
    "MINION",
    "SPELL",
    "WEAPON",
    "LOCATION",
    "HERO",
    "HERO_POWER",
];

const CLASSES: &[&str] = &[
    "NEUTRAL",
    "DEATHKNIGHT",
    "DEMONHUNTER",
    "DRUID",
    "HUNTER",
    "MAGE",
    "PALADIN",
    "PRIEST",
    "ROGUE",
    "SHAMAN",
    "WARLOCK",
    "WARRIOR",
    "DREAM",
    "WHIZBANG",
];

const RARITIES: &[&str] = &["NONE", "FREE", "COMMON", "RARE", "EPIC", "LEGENDARY"];

const SCHOOLS: &[&str] = &[
    "NONE", "ARCANE", "FEL", "FIRE", "FROST", "HOLY", "NATURE", "SHADOW",
];

fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("cards") => match generate_cards() {
            Ok(msg) => println!("{msg}"),
            Err(e) => {
                eprintln!("xtask cards: {e}");
                std::process::exit(1);
            }
        },
        Some("wild-gauntlet") => match wild::generate(&repo_root()) {
            Ok(msg) => println!("{msg}"),
            Err(e) => {
                eprintln!("xtask wild-gauntlet: {e}");
                std::process::exit(1);
            }
        },
        Some("backfill") => {
            let Some(dump) = args.next() else {
                eprintln!("usage: cargo run -p xtask -- backfill <dump.json>");
                eprintln!("see the module docs in xtask/src/backfill.rs for the fetch");
                std::process::exit(2);
            };
            match backfill::run(&repo_root(), Path::new(&dump)) {
                Ok(msg) => println!("{msg}"),
                Err(e) => {
                    eprintln!("xtask backfill: {e}");
                    std::process::exit(1);
                }
            }
        }
        other => {
            eprintln!("usage: cargo run -p xtask -- [cards|wild-gauntlet|backfill <dump>]");
            if let Some(o) = other {
                eprintln!("unknown command: {o}");
            }
            std::process::exit(2);
        }
    }
}

/// Repository root, found by walking up from this crate.
fn repo_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // tavernlab-sim
    p.pop(); // repo
    p
}

struct Card {
    id: String,
    dbf: u32,
    name: String,
    text: String,
    kind: usize,
    class: usize,
    rarity: usize,
    school: usize,
    set: String,
    cost: i16,
    atk: i16,
    hp: i16,
    dur: i16,
    armor: i16,
    races: u32,
    keywords: u64,
    spell_damage: i8,
    overload: i8,
    dormant: i8,
    collectible: bool,
    formats: u8,
    /// dbf ids of the tokens this card creates, resolved to table indices in
    /// [`render`] once every card has one.
    children: Vec<u32>,
}

fn generate_cards() -> Result<String, String> {
    let root = repo_root();
    let std_path = root.join("data/standard_cards.json");
    let wild_path = root.join("data/wild_cards.json");

    // Wild is a delta on top of Standard, the way the corpus builder wrote
    // them. Building one table for both formats and tagging each card
    // with the formats it is legal in means a format switch is a bit test
    // rather than a second corpus and a second set of workers.
    let mut raw: BTreeMap<String, Json> = BTreeMap::new();
    load_into(&std_path, &mut raw)?;
    if wild_path.exists() {
        load_into(&wild_path, &mut raw)?;
    }

    // Whether the corpus carries the numeric fields at all. One built before
    // the corpus builder learned to write them has none, and the card text is
    // then the only source. One built after it writes them only where they are
    // non-zero -- so an absent field means zero, and falling back to the text
    // there would reinstate what the field exists to avoid: the text parse
    // also matches the cards that *hand out* Spell Damage rather than carry
    // it.
    let has_sd = raw.values().any(|e| e.get("sd").is_some());
    let has_ovl = raw.values().any(|e| e.get("ovl").is_some());

    let mut unknown_mech: BTreeMap<String, usize> = BTreeMap::new();
    let mut unknown_race: BTreeMap<String, usize> = BTreeMap::new();
    let mut cards: Vec<Card> = Vec::with_capacity(raw.len());

    for (id, e) in &raw {
        let set = e.str_or_empty("set").to_string();
        // The retired Classic format duplicates the whole EXPERT1 pool under a
        // second set code and is legal nowhere.
        if set == "VANILLA" {
            continue;
        }
        let text = e.str_or_empty("text").to_string();
        // HearthstoneJSON stores weapon durability in `health`, not
        // `durability`: every one of the 232 collectible weapons has
        // `durability: 0`. Reading `dur` directly gives a weapon that breaks
        // on its first swing, which is what the engine did until this was
        // measured. Normalised here so `dur` is the single trustworthy field.
        let kind = e.str_or_empty("type");
        let mut dur = e.i64_or_zero("dur") as i16;
        let hp = e.i64_or_zero("hp") as i16;
        if dur == 0 && matches!(kind, "WEAPON" | "LOCATION") {
            dur = hp;
        }
        // A card the live API served but the local CardDefs snapshot has
        // never heard of carries no `mech` at all: its keywords arrive under
        // `kw`, the API's own list. That list also names keywords a card only
        // references, so it is a fallback and never a second opinion -- read
        // only where CardDefs contributed nothing. Without it a new set's
        // plain Taunt minions enter the table as vanilla bodies, which is
        // exactly the state 29 collectible Standard minions are in today.
        let nodefs = e.bool_or_false("nodefs");
        let mut keywords = 0u64;
        for m in e.arr_or_empty(if nodefs { "kw" } else { "mech" }) {
            let Some(m) = m.as_str() else { continue };
            let m = alias(m);
            // Prepare is normally printed as a bare keyword with no mechanic
            // tag of its own, and is read off the text below. The live API's
            // keyword list does name it, so that is honoured where it appears
            // -- into the reserved bit, not a generated one.
            if m == "PREPARE" {
                keywords |= 1 << PREPARE_BIT;
                continue;
            }
            match KEYWORDS.iter().position(|k| *k == m) {
                Some(bit) => keywords |= 1 << bit,
                None => {
                    if !IGNORED_MECHANICS.contains(&m) {
                        *unknown_mech.entry(m.to_string()).or_default() += 1;
                    }
                }
            }
        }
        let mut races = 0u32;
        for r in e.arr_or_empty("races") {
            let Some(r) = r.as_str() else { continue };
            match RACES.iter().position(|x| *x == r) {
                Some(bit) => races |= 1 << bit,
                None => *unknown_race.entry(r.to_string()).or_default() += 1,
            }
        }

        let mut formats = 0u8;
        if sets::STANDARD.contains(&set.as_str()) {
            formats |= 1;
        }
        if sets::WILD.contains(&set.as_str()) {
            formats |= 2;
        }

        cards.push(Card {
            id: id.clone(),
            dbf: e.i64_or_zero("dbf") as u32,
            name: e.str_or_empty("name").to_string(),
            kind: index_of(KINDS, e.str_or_empty("type"), "type")?,
            class: index_of(CLASSES, e.str_or_empty("cls"), "cls")?,
            rarity: index_or_none(RARITIES, e.str_or_empty("rarity")),
            school: index_or_none(SCHOOLS, e.str_or_empty("school")),
            cost: e.i64_or_zero("cost") as i16,
            atk: e.i64_or_zero("atk") as i16,
            hp,
            dur,
            armor: e.i64_or_zero("armor") as i16,
            races,
            spell_damage: if has_sd {
                e.i64_or_zero("sd") as i8
            } else {
                parse_after(&text, "Spell Damage +").unwrap_or(0)
            },
            overload: if has_ovl {
                e.i64_or_zero("ovl") as i8
            } else {
                // Without the field the mechanic says only *that* a card
                // overloads, so the amount has to come from the text.
                parse_between(&text, "Overload:", "(", ")").unwrap_or(
                    if keywords & bit_of(KEYWORDS, "OVERLOAD") != 0 {
                        1
                    } else {
                        0
                    },
                )
            },
            dormant: parse_dormant(&text),
            keywords: keywords
                // "the text is nothing but keywords" is a claim about the
                // keywords a card is known to have. For one CardDefs cannot
                // describe, the bits came from a list that also names what
                // the card merely references, so the claim is unbacked and
                // the card counts as unimplemented instead.
                | if !nodefs && text_is_only_keywords(&text) {
                    1u64 << TEXT_UNDERSTOOD_BIT
                } else {
                    0
                }
                // Prepare is printed as a bare keyword with no mechanic tag,
                // so it is recognised from the text here rather than at
                // runtime.
                | if text.starts_with("Prepare") {
                    1u64 << PREPARE_BIT
                } else {
                    0
                }
                // Rewind, the same way and for a stronger reason: it is in no
                // card's `mech` at all, and the `kw` list that does name it
                // cannot tell a card that *has* Rewind from one that only
                // talks about it -- Time Machine ("Get a random Rewind card")
                // and Morchie ("Your Rewinds keep BOTH outcomes") are both in
                // it. The printed text can: a card with the keyword leads
                // with it. See `leads_with_rewind`.
                | if leads_with_rewind(&text) {
                    1u64 << rewind_bit()
                } else {
                    0
                },
            set,
            text,
            collectible: e.bool_or_false("coll"),
            formats,
            children: e
                .arr_or_empty("child")
                .iter()
                .filter_map(|v| v.as_i64())
                .map(|v| v as u32)
                .collect(),
        });
    }

    // A stable order the generated ids can be trusted across regenerations:
    // dbf is assigned by Blizzard and never reused.
    cards.sort_by_key(|c| c.dbf);

    if !unknown_mech.is_empty() {
        return Err(format!(
            "unclassified mechanics {unknown_mech:?} — add them to KEYWORDS \
             (if they change a rule) or IGNORED_MECHANICS (if they do not)"
        ));
    }
    if !unknown_race.is_empty() {
        return Err(format!(
            "unclassified races {unknown_race:?} — append them to RACES"
        ));
    }
    // Bits 62 and 63 are spoken for (PREPARE and TEXT_UNDERSTOOD), so the
    // generated list may not grow into them. Appending past this point would
    // silently overwrite one of the two.
    if KEYWORDS.len() as u32 > PREPARE_BIT {
        return Err(format!(
            "{} keyword bits would overwrite PREPARE ({PREPARE_BIT}) or TEXT_UNDERSTOOD ({TEXT_UNDERSTOOD_BIT})",
            KEYWORDS.len()
        ));
    }
    if cards.len() > u16::MAX as usize {
        return Err(format!(
            "{} cards exceeds the u16 CardId space",
            cards.len()
        ));
    }

    let (out, dropped_children) = render(&cards);
    let dest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/src/cards/table.rs");
    std::fs::write(&dest, &out).map_err(|e| format!("writing {}: {e}", dest.display()))?;

    let standard = cards.iter().filter(|c| c.formats & 1 != 0).count();
    let wild = cards.iter().filter(|c| c.formats & 2 != 0).count();
    let nodefs = raw.values().filter(|e| e.bool_or_false("nodefs")).count();
    let mut note = String::new();
    if nodefs > 0 {
        let _ = write!(
            note,
            "\n  {nodefs} card(s) are newer than the CardDefs snapshot: keywords \
             read from the API list, text not trusted"
        );
    }
    if dropped_children > 0 {
        let _ = write!(
            note,
            "\n  {dropped_children} child reference(s) point outside the corpus \
             (enchantments, and cards no format keeps) and were dropped"
        );
    }
    Ok(format!(
        "wrote {} ({} cards, {} Standard-legal, {} Wild-legal, {} KB of source){note}",
        dest.display(),
        cards.len(),
        standard,
        wild,
        out.len() / 1024
    ))
}

fn load_into(path: &Path, out: &mut BTreeMap<String, Json>) -> Result<(), String> {
    let src =
        std::fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let v = Json::parse(&src).map_err(|e| format!("{}: {e}", path.display()))?;
    let obj = v
        .as_object()
        .ok_or_else(|| format!("{}: top level is not an object", path.display()))?;
    for (k, v) in obj {
        out.insert(k.clone(), v.clone());
    }
    Ok(())
}

fn index_of(table: &[&str], value: &str, what: &str) -> Result<usize, String> {
    table
        .iter()
        .position(|x| *x == value)
        .ok_or_else(|| format!("unknown {what}: {value:?}"))
}

/// Index in `table`, where an empty or absent value means slot 0 ("NONE").
fn index_or_none(table: &[&str], value: &str) -> usize {
    if value.is_empty() {
        return 0;
    }
    table.iter().position(|x| *x == value).unwrap_or(0)
}

fn bit_of(table: &[&str], name: &str) -> u64 {
    match table.iter().position(|x| *x == name) {
        Some(b) => 1 << b,
        None => 0,
    }
}

/// Phrases that are exactly a keyword the kernel already implements.
///
/// A minion printed with nothing but these needs no code: the keyword bits and
/// the stats already say everything. Longest first, so "Mega-Windfury" is not
/// half-eaten by "Windfury".
const KEYWORD_PHRASES: &[&str] = &[
    "Can't be targeted by spells or Hero Powers",
    "Can't be targeted by Spells or Hero Powers",
    "Can't be targeted by spells",
    "Can't be Silenced",
    "Can't be destroyed",
    "Mega-Windfury",
    "Divine Shield",
    "Can't Attack",
    "Can't attack",
    "Windfury",
    "Lifesteal",
    "Poisonous",
    "Venomous",
    "Stealth",
    "Elusive",
    "Reborn",
    "Charge",
    "Immune",
    "Taunt",
    "Rush",
];

/// Whether a card's whole text is keywords the kernel already models.
///
/// Deliberately conservative: anything left over after removing known phrases,
/// the numeric keyword forms, and punctuation means the card has real rules
/// text and is not implemented until someone writes it.
fn text_is_only_keywords(text: &str) -> bool {
    let mut s = text.to_string();

    // Numeric keyword forms, whose value is already parsed into the table.
    for (anchor, close) in [
        ("Spell Damage +", ""),
        ("Overload: (", ")"),
        ("Overload (", ")"),
        // The dormancy itself is in the table and the kernel counts it down,
        // so a body that is dormant and otherwise vanilla needs no code.
        ("Dormant for ", "turns."),
        ("Dormant for ", "turn."),
    ] {
        while let Some(at) = s.find(anchor) {
            let rest = &s[at + anchor.len()..];
            let take = if close.is_empty() {
                rest.chars().take_while(char::is_ascii_digit).count()
            } else {
                match rest.find(close) {
                    Some(end) => end + close.len(),
                    None => break,
                }
            };
            s.replace_range(at..at + anchor.len() + take, " ");
        }
    }

    // Prepare on its own says nothing beyond the keyword.
    if let Some(rest) = s.strip_prefix("Prepare") {
        s = rest.to_string();
    }

    for phrase in KEYWORD_PHRASES {
        s = s.replace(phrase, " ");
        s = s.replace(&phrase.to_lowercase(), " ");
    }

    // What survives must be punctuation and whitespace only.
    s.chars().all(|c| {
        c.is_whitespace() || matches!(c, '.' | ',' | ';' | ':' | '<' | '>' | '/' | 'b' | 'i')
    }) && !s
        .chars()
        .any(|c| c.is_alphanumeric() && !matches!(c, 'b' | 'i'))
}

/// Turns a card spends dormant when it enters play.
///
/// Read from the text because nothing records it as a tag. The catch is that
/// the same phrase is how a card says it puts *something else* to sleep --
/// "make it go Dormant for 1 turn" -- and taking that at face value started
/// the card's own body dormant, on 13 collectible cards including Warden
/// Maiev and Maiev Shadowsong. A card that is itself dormant prints the
/// phrase as a clause of its own; a card that inflicts it puts a verb first.
fn parse_dormant(text: &str) -> i8 {
    const GRANTED: &[&str] = &["go ", "goes ", "is ", "'s ", "’s "];
    let Some(at) = text.find("Dormant for ") else {
        return 0;
    };
    let before = text[..at].to_lowercase();
    if GRANTED.iter().any(|g| before.ends_with(g)) {
        return 0;
    }
    parse_after(text, "Dormant for ").unwrap_or(0)
}

/// The integer immediately after `prefix`, e.g. `"Spell Damage +2"` -> 2.
fn parse_after(text: &str, prefix: &str) -> Option<i8> {
    let rest = text.split_once(prefix)?.1;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// The integer between `open` and `close`, searched after `anchor`.
/// `"Overload: (2)"` -> 2, tolerating the space the corpus is inconsistent about.
fn parse_between(text: &str, anchor: &str, open: &str, close: &str) -> Option<i8> {
    let rest = text.split_once(anchor)?.1;
    let rest = rest.split_once(open)?.1;
    let inner = rest.split_once(close)?.0;
    inner.trim().parse().ok()
}

fn render(cards: &[Card]) -> (String, usize) {
    let n = cards.len();
    let mut s = String::with_capacity(n * 220);

    s.push_str(
        "// @generated by `cargo run -p xtask -- cards`. Do not edit by hand.\n\
         //\n\
         // Regenerate after refreshing the HearthstoneJSON corpora. The bit\n\
         // constants below are emitted alongside the table that uses them, so the\n\
         // encoding and the engine's view of it cannot drift apart.\n\n\
         use super::{CardDef, CardId, CardInfo, Formats, Keywords, Races};\n\n",
    );

    // ---- bit constants ------------------------------------------------
    s.push_str("impl Keywords {\n");
    for (i, k) in KEYWORDS.iter().enumerate() {
        let _ = writeln!(
            s,
            "    pub const {}: Keywords = Keywords(1 << {i});",
            const_name(k)
        );
    }
    let _ = writeln!(
        s,
        "\n    /// Set when the card's whole rules text is keywords already\n\
         \x20   /// modelled above, so no per-card code is needed to play it.\n\
         \x20   /// Computed at generation time; the top bit is reserved for it.\n\
         \x20   pub const TEXT_UNDERSTOOD: Keywords = Keywords(1 << {TEXT_UNDERSTOOD_BIT});"
    );
    let _ = writeln!(
        s,
        "\n    /// Prepare: bank your remaining mana into a discount on this card."
    );
    let _ = writeln!(
        s,
        "    /// Printed as a bare keyword with no mechanic tag of its own, so it"
    );
    let _ = writeln!(s, "    /// is recognised from the card text at generation time.");
    let _ = writeln!(
        s,
        "    pub const PREPARE: Keywords = Keywords(1 << {PREPARE_BIT});"
    );
    let _ = writeln!(s, "\n    /// Number of distinct mechanic bits in use.");
    let _ = writeln!(s, "    pub const COUNT: u32 = {};", KEYWORDS.len());
    s.push_str("}\n\n");

    s.push_str("impl Races {\n");
    for (i, r) in RACES.iter().enumerate() {
        let _ = writeln!(
            s,
            "    pub const {}: Races = Races(1 << {i});",
            const_name(r)
        );
    }
    let _ = writeln!(s, "\n    /// Number of distinct tribe bits in use.");
    let _ = writeln!(s, "    pub const COUNT: u32 = {};", RACES.len());
    s.push_str("}\n\n");

    // ---- the tables ---------------------------------------------------
    let _ = writeln!(
        s,
        "/// Every card, ordered by Blizzard's `dbfId` so a `CardId` is stable\n\
         /// across regenerations.\n\
         pub static DEFS: [CardDef; {n}] = ["
    );
    for c in cards {
        let _ = writeln!(
            s,
            "    CardDef {{ kind: {}, class: {}, rarity: {}, school: {}, \
             cost: {}, atk: {}, hp: {}, dur: {}, armor: {}, spell_damage: {}, overload: {}, \
             dormant: {}, races: Races({}), keywords: Keywords({}), formats: Formats({}), \
             collectible: {} }},",
            c.kind,
            c.class,
            c.rarity,
            c.school,
            c.cost,
            c.atk,
            c.hp,
            c.dur,
            c.armor,
            c.spell_damage,
            c.overload,
            c.dormant,
            c.races,
            c.keywords,
            c.formats,
            c.collectible
        );
    }
    s.push_str("];\n\n");

    let _ = writeln!(
        s,
        "/// Strings, split out of [`DEFS`] so the hot table stays small enough\n\
         /// to stay in cache during a batch run. Same index space.\n\
         pub static INFO: [CardInfo; {n}] = ["
    );
    for c in cards {
        let _ = writeln!(
            s,
            "    CardInfo {{ dbf: {}, id: {}, name: {}, text: {}, set: {} }},",
            c.dbf,
            quote(&c.id),
            quote(&c.name),
            quote(&c.text),
            quote(&c.set)
        );
    }
    s.push_str("];\n\n");

    // ---- lookup indexes -----------------------------------------------
    // Sorted arrays plus binary search: no hashing, no allocation, and the
    // whole thing lives in read-only memory.
    let mut by_dbf: Vec<(u32, usize)> = cards.iter().enumerate().map(|(i, c)| (c.dbf, i)).collect();
    by_dbf.sort_unstable();
    let _ = writeln!(
        s,
        "/// `(dbf, index)` sorted by dbf, for binary search. Deck codes carry\n\
         /// dbf ids, so this is the import path.\n\
         pub static BY_DBF: [(u32, u16); {n}] = ["
    );
    for (dbf, i) in &by_dbf {
        let _ = writeln!(s, "    ({dbf}, {i}),");
    }
    s.push_str("];\n\n");

    let mut by_id: Vec<(&str, usize)> = cards
        .iter()
        .enumerate()
        .map(|(i, c)| (c.id.as_str(), i))
        .collect();
    by_id.sort_unstable();
    let _ = writeln!(
        s,
        "/// `(card id, index)` sorted by id, for binary search.\n\
         pub static BY_ID: [(&str, u16); {n}] = ["
    );
    for (id, i) in &by_id {
        let _ = writeln!(s, "    ({}, {i}),", quote(id));
    }
    s.push_str("];\n\n");

    // Name lookup resolves to one printing. Collectible wins over tokens, and
    // a hero portrait never wins over a real card sharing its name: a deck
    // list naming a card means the card, not the skin.
    let mut best_by_name: BTreeMap<&str, (i32, usize)> = BTreeMap::new();
    for (i, c) in cards.iter().enumerate() {
        // Non-portrait dominates collectibility: a hero skin is never what a
        // deck list means, even when the skin is collectible and the real card
        // sharing its name is a non-collectible token.
        let portrait = c.kind == 4 && c.set == "HERO_SKINS";
        let rank = (!portrait as i32) * 4 + (c.collectible as i32) * 2 + (c.set == "CORE") as i32;
        let e = best_by_name.entry(c.name.as_str()).or_insert((-1, i));
        if rank > e.0 {
            *e = (rank, i);
        }
    }
    let _ = writeln!(
        s,
        "/// `(name, index)` sorted by name, for binary search. One entry per\n\
         /// distinct name: the collectible, non-portrait printing wins.\n\
         pub static BY_NAME: [(&str, u16); {}] = [",
        best_by_name.len()
    );
    for (name, (_, i)) in &best_by_name {
        let _ = writeln!(s, "    ({}, {i}),", quote(name));
    }
    s.push_str("];\n\n");

    // ---- the tokens each card creates ---------------------------------
    // `childIds` from the live card library is the game's own answer to what
    // a card puts into play or into hand, and the only source for that link.
    // Flattened into one array plus a `(start, len)` per card: the relation
    // is then two statics in read-only memory and a lookup is one index,
    // which is how the rest of the table is addressed.
    let index_of_dbf: BTreeMap<u32, u16> = cards
        .iter()
        .enumerate()
        .map(|(i, c)| (c.dbf, i as u16))
        .collect();
    let mut flat: Vec<u16> = Vec::new();
    let mut slices: Vec<(u32, u16)> = Vec::with_capacity(n);
    let mut dropped = 0usize;
    for c in cards {
        let start = flat.len() as u32;
        for dbf in &c.children {
            match index_of_dbf.get(dbf) {
                Some(i) => flat.push(*i),
                // A child the corpus does not carry: an enchantment, or a
                // card from a set no format keeps. Nothing could summon it,
                // so the reference is dropped rather than left dangling.
                None => dropped += 1,
            }
        }
        slices.push((start, (flat.len() as u32 - start) as u16));
    }

    let _ = writeln!(
        s,
        "/// `(start, len)` into [`CHILD_IDS`] for each card, in the same index\n\
         /// space as [`DEFS`]. Dense rather than sparse, so a lookup is one\n\
         /// index rather than a search.\n\
         pub static CHILD_SLICES: [(u32, u16); {n}] = ["
    );
    for (start, len) in &slices {
        let _ = writeln!(s, "    ({start}, {len}),");
    }
    s.push_str("];\n\n");

    let _ = writeln!(
        s,
        "/// Every card a card creates, sliced by [`CHILD_SLICES`]. A card\n\
         /// lists the upgraded printings of itself here as well as the tokens\n\
         /// it summons, so callers filter by kind.\n\
         pub static CHILD_IDS: [CardId; {}] = [",
        flat.len()
    );
    for i in &flat {
        let _ = writeln!(s, "    CardId({i}),");
    }
    s.push_str("];\n");

    (s, dropped)
}

/// A Rust identifier for a mechanic name that may be mixed case.
fn const_name(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 4);
    let mut prev_lower = false;
    for c in raw.chars() {
        if c.is_ascii_uppercase() && prev_lower {
            out.push('_');
        }
        prev_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        out.push(c.to_ascii_uppercase());
    }
    out
}

/// A Rust string literal. Non-ASCII passes through — generated source is UTF-8.
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{{{:x}}}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
