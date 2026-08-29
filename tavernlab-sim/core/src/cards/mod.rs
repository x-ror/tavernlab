//! The card database: one immutable table, shared by every thread.
//!
//! A card is a [`CardId`] — a 16-bit index — everywhere in the engine. Nothing
//! in the live game state holds a card's stats, name or text; it holds the
//! index and looks through it. That is what keeps a game state small enough to
//! copy, and it is why a batch run needs one copy of the corpus for the whole
//! process instead of one per worker.
//!
//! The table itself is generated (`cargo run -p xtask -- cards`) into
//! `table.rs` and compiled in, so there is no startup parse and no data file to
//! ship alongside the binary.

pub mod behaviour;
mod table;

pub use behaviour::{
    APPROXIMATE, Aura, Behaviour, Ctx, DARK_GIFTS, TargetSpec, acts_when_drawn,
    apply_game_setup, behaviour_of, controlled, gift_card, gift_keywords, gift_stats,
    awakened_by_dragon, drawn_acts_for_opponent, is_approximate, is_implemented,
    combines, is_aura, lets_attacks_ignore_taunt, pirate_damage_bonus,
    HAS_AURA, HAS_BONUS, HAS_TRIGGER, hooks,
    doubles_summons, is_leyline, rattles_from_hand_or_deck, reborn_keeps_enchantments, recombines, shatters_into,
    upgrades_while_held,
    windrunner_bit, windrunner_sisters,
};
pub use table::{BY_DBF, BY_ID, BY_NAME, CHILD_IDS, CHILD_SLICES, DEFS, INFO};

/// Index into [`DEFS`]. 16 bits is deliberate: it keeps hands, decks and
/// graveyards half the size a pointer-based design would need.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct CardId(pub u16);

impl CardId {
    /// The card's static definition.
    #[inline]
    pub fn def(self) -> &'static CardDef {
        &DEFS[self.0 as usize]
    }

    /// The card's strings. Never touched on a hot path.
    #[inline]
    pub fn info(self) -> &'static CardInfo {
        &INFO[self.0 as usize]
    }

    #[inline]
    pub fn name(self) -> &'static str {
        self.info().name
    }

    /// The cards this one creates: its tokens, and the upgraded printings of
    /// itself. Straight from the live card library, which is the only source
    /// that records the link at all.
    #[inline]
    pub fn children(self) -> &'static [CardId] {
        let (start, len) = CHILD_SLICES[self.0 as usize];
        &CHILD_IDS[start as usize..start as usize + len as usize]
    }

    /// The minions among [`children`](Self::children), which is what "summon
    /// this card's token" means for a card that has one. A card lists its own
    /// upgraded printings as children too, so the filter is not optional.
    pub fn summonable_children(self) -> impl Iterator<Item = CardId> {
        self.children()
            .iter()
            .copied()
            .filter(|c| c.def().kind() == Kind::Minion)
    }
}

/// What a card is. Discriminants are fixed by the generator's `KINDS` order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Kind {
    Minion = 0,
    Spell = 1,
    Weapon = 2,
    Location = 3,
    Hero = 4,
    HeroPower = 5,
}

/// Class, including the two pseudo-classes the corpus carries.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Class {
    Neutral = 0,
    DeathKnight = 1,
    DemonHunter = 2,
    Druid = 3,
    Hunter = 4,
    Mage = 5,
    Paladin = 6,
    Priest = 7,
    Rogue = 8,
    Shaman = 9,
    Warlock = 10,
    Warrior = 11,
    Dream = 12,
    Whizbang = 13,
}

/// The eleven playable classes, in the generator's order.
pub const PLAYABLE_CLASSES: [Class; 11] = [
    Class::DeathKnight,
    Class::DemonHunter,
    Class::Druid,
    Class::Hunter,
    Class::Mage,
    Class::Paladin,
    Class::Priest,
    Class::Rogue,
    Class::Shaman,
    Class::Warlock,
    Class::Warrior,
];

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[repr(u8)]
pub enum Rarity {
    None = 0,
    Free = 1,
    Common = 2,
    Rare = 3,
    Epic = 4,
    Legendary = 5,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum School {
    None = 0,
    Arcane = 1,
    Fel = 2,
    Fire = 3,
    Frost = 4,
    Holy = 5,
    Nature = 6,
    Shadow = 7,
}

/// Gameplay mechanics as bits. Constants are generated in `table.rs` from the
/// same list that encodes the cards, so the two cannot disagree.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Keywords(pub u64);

/// Minion tribes as bits. Constants generated alongside the table.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Races(pub u32);

/// Which formats a card is legal in, resolved at generation time so the engine
/// never compares set codes.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Formats(pub u8);

impl Formats {
    pub const STANDARD: Formats = Formats(1);
    pub const WILD: Formats = Formats(2);

    #[inline]
    pub const fn has(self, f: Formats) -> bool {
        self.0 & f.0 != 0
    }
}

macro_rules! bitset_ops {
    ($t:ty, $inner:ty) => {
        impl $t {
            /// Empty set.
            pub const NONE: $t = Self(0);

            /// True when every bit in `other` is set here.
            #[inline]
            pub const fn has(self, other: $t) -> bool {
                self.0 & other.0 == other.0
            }

            /// True when the two sets overlap at all.
            #[inline]
            pub const fn any(self, other: $t) -> bool {
                self.0 & other.0 != 0
            }

            #[inline]
            pub const fn is_empty(self) -> bool {
                self.0 == 0
            }

            #[inline]
            pub fn insert(&mut self, other: $t) {
                self.0 |= other.0;
            }

            #[inline]
            pub fn remove(&mut self, other: $t) {
                self.0 &= !other.0;
            }

            #[inline]
            pub fn set(&mut self, other: $t, on: bool) {
                if on {
                    self.insert(other)
                } else {
                    self.remove(other)
                }
            }
        }

        impl core::ops::BitOr for $t {
            type Output = $t;
            #[inline]
            fn bitor(self, rhs: $t) -> $t {
                Self(self.0 | rhs.0)
            }
        }

        impl core::ops::BitAnd for $t {
            type Output = $t;
            #[inline]
            fn bitand(self, rhs: $t) -> $t {
                Self(self.0 & rhs.0)
            }
        }

        impl core::ops::Not for $t {
            type Output = $t;
            #[inline]
            fn not(self) -> $t {
                Self(!self.0)
            }
        }

        impl core::ops::BitOrAssign for $t {
            #[inline]
            fn bitor_assign(&mut self, rhs: $t) {
                self.0 |= rhs.0;
            }
        }

        const _: () = assert!(size_of::<$t>() == size_of::<$inner>());
    };
}

bitset_ops!(Keywords, u64);
bitset_ops!(Races, u32);

/// A card's rules-relevant data. Everything the kernel reads, nothing it does
/// not — the strings live in [`CardInfo`] so this array stays dense.
#[derive(Clone, Copy, Debug)]
pub struct CardDef {
    pub kind: u8,
    pub class: u8,
    pub rarity: u8,
    pub school: u8,
    pub cost: i16,
    pub atk: i16,
    /// Health for minions and heroes; durability for locations.
    pub hp: i16,
    pub dur: i16,
    pub armor: i16,
    /// Spell Damage +N, read from the card text at generation time.
    pub spell_damage: i8,
    /// Overload amount, read from the text; the mechanic alone only says that
    /// a card overloads, not by how much.
    pub overload: i8,
    /// Turns spent dormant, read from the text.
    pub dormant: i8,
    pub races: Races,
    pub keywords: Keywords,
    pub formats: Formats,
    pub collectible: bool,
}

impl CardDef {
    #[inline]
    pub fn kind(&self) -> Kind {
        // Safety by construction: the generator writes only values in range,
        // and the match makes that explicit rather than transmuting.
        match self.kind {
            0 => Kind::Minion,
            1 => Kind::Spell,
            2 => Kind::Weapon,
            3 => Kind::Location,
            4 => Kind::Hero,
            _ => Kind::HeroPower,
        }
    }

    #[inline]
    pub fn class(&self) -> Class {
        match self.class {
            0 => Class::Neutral,
            1 => Class::DeathKnight,
            2 => Class::DemonHunter,
            3 => Class::Druid,
            4 => Class::Hunter,
            5 => Class::Mage,
            6 => Class::Paladin,
            7 => Class::Priest,
            8 => Class::Rogue,
            9 => Class::Shaman,
            10 => Class::Warlock,
            11 => Class::Warrior,
            12 => Class::Dream,
            _ => Class::Whizbang,
        }
    }

    #[inline]
    pub fn rarity(&self) -> Rarity {
        match self.rarity {
            1 => Rarity::Free,
            2 => Rarity::Common,
            3 => Rarity::Rare,
            4 => Rarity::Epic,
            5 => Rarity::Legendary,
            _ => Rarity::None,
        }
    }

    #[inline]
    pub fn school(&self) -> School {
        match self.school {
            1 => School::Arcane,
            2 => School::Fel,
            3 => School::Fire,
            4 => School::Frost,
            5 => School::Holy,
            6 => School::Nature,
            7 => School::Shadow,
            _ => School::None,
        }
    }

    /// True when this card may go in a deck of `class` — its own class or
    /// neutral.
    #[inline]
    pub fn playable_by(&self, class: Class) -> bool {
        self.class() == class || self.class() == Class::Neutral
    }

    /// How many copies a deck may hold.
    #[inline]
    pub fn copy_limit(&self) -> u8 {
        if self.rarity() == Rarity::Legendary {
            1
        } else {
            2
        }
    }

    /// Card types that can be put in a deck. Hero *cards* qualify; hero
    /// portraits share the type but are legal in no format, which the
    /// `formats` bits already exclude.
    #[inline]
    pub fn deckable(&self) -> bool {
        matches!(
            self.kind(),
            Kind::Minion | Kind::Spell | Kind::Weapon | Kind::Location | Kind::Hero
        )
    }
}

/// A card's strings. Separate from [`CardDef`] so the hot table stays dense;
/// nothing in the rules reads these.
#[derive(Clone, Copy, Debug)]
pub struct CardInfo {
    /// Blizzard's `dbfId`. Deck codes are lists of these, which makes it an
    /// import/export concern rather than a rule — so it sits here and keeps
    /// [`CardDef`] at 32 bytes.
    pub dbf: u32,
    /// Blizzard's string id, e.g. `"CS2_029"`.
    pub id: &'static str,
    pub name: &'static str,
    pub text: &'static str,
    pub set: &'static str,
}

/// The card with this `dbfId`, if the corpus has one. Deck codes carry dbf ids.
pub fn by_dbf(dbf: u32) -> Option<CardId> {
    BY_DBF
        .binary_search_by_key(&dbf, |(d, _)| *d)
        .ok()
        .map(|i| CardId(BY_DBF[i].1))
}

/// Byte equality for two strings, in a form a `const fn` can use.
const fn str_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// The card with this string id, resolved at compile time.
///
/// The point of this over [`by_id`] is the failure mode. A token id that no
/// longer resolves -- renamed upstream, or rotated out of both corpora -- stops
/// the build at the line that names it, rather than becoming a card that costs
/// mana and summons nothing. A hand-kept list of "every id the behaviour table
/// mentions", walked by a test, did the same job for exactly as long as whoever
/// added a token remembered to add it there too.
///
/// The scan is linear over a sorted array because a binary search buys nothing
/// at compile time and this reads the same as the table it searches.
pub const fn token(id: &str) -> CardId {
    let mut i = 0;
    while i < BY_ID.len() {
        if str_eq(BY_ID[i].0, id) {
            return CardId(BY_ID[i].1);
        }
        i += 1;
    }
    panic!("no card in the corpus has this id")
}

/// The card with this string id.
pub fn by_id(id: &str) -> Option<CardId> {
    BY_ID
        .binary_search_by_key(&id, |(s, _)| *s)
        .ok()
        .map(|i| CardId(BY_ID[i].1))
}

/// The card a deck list means by this name.
///
/// One printing per name, chosen at generation time: collectible beats token,
/// and a real card always beats a hero portrait that happens to share its name.
pub fn by_name(name: &str) -> Option<CardId> {
    BY_NAME
        .binary_search_by_key(&name, |(s, _)| *s)
        .ok()
        .map(|i| CardId(BY_NAME[i].1))
}

/// Every card, as ids.
pub fn all() -> impl Iterator<Item = CardId> {
    (0..DEFS.len() as u16).map(CardId)
}


/// Cards a Discover may offer: collectible, deckable, and understood by the
/// engine.
///
/// Filtered rather than cached per predicate: the corpus is 16 771 entries and
/// a Discover happens a handful of times per game, so one pass over a
/// pre-filtered base costs less than the bookkeeping a cache would need.
/// Whether this card is "from the past".
///
/// Across the Timeways prints the phrase on twelve Standard cards, and it
/// means a card from a set that has rotated out: Wild-legal and no longer
/// Standard-legal. That is a question the card data answers exactly -- every
/// `CardDef` carries the formats it is legal in -- so nothing here is a
/// judgement, and the pool moves on its own the next time the corpus is
/// regenerated after a rotation.
#[inline]
pub fn from_the_past(d: &CardDef) -> bool {
    d.formats.has(Formats::WILD) && !d.formats.has(Formats::STANDARD)
}

pub fn discover_pool(pred: impl Fn(&CardDef) -> bool) -> Vec<CardId> {
    static BASE: std::sync::OnceLock<Box<[CardId]>> = std::sync::OnceLock::new();
    let base = BASE.get_or_init(|| {
        all()
            .filter(|c| {
                let d = c.def();
                d.collectible && d.deckable() && is_implemented(*c)
            })
            .collect::<Vec<_>>()
            .into_boxed_slice()
    });
    base.iter().copied().filter(|c| pred(c.def())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_populated() {
        assert!(
            DEFS.len() > 10_000,
            "corpus looks truncated: {}",
            DEFS.len()
        );
        assert_eq!(DEFS.len(), INFO.len());
        assert_eq!(DEFS.len(), BY_DBF.len());
        assert_eq!(DEFS.len(), BY_ID.len());
    }

    #[test]
    fn hot_table_entry_stays_small() {
        // The reason strings live in a parallel array. If this grows, a batch
        // run starts missing cache on the card table.
        assert_eq!(
            size_of::<CardDef>(),
            32,
            "CardDef grew; strings and ids belong in CardInfo"
        );
    }

    #[test]
    fn lookups_agree_with_each_other() {
        for id in all() {
            let i = id.info();
            assert_eq!(by_dbf(i.dbf), Some(id), "dbf lookup for {}", i.id);
            assert_eq!(by_id(i.id), Some(id), "id lookup for {}", i.id);
        }
    }

    #[test]
    fn missing_lookups_are_none_not_panics() {
        assert_eq!(by_dbf(0xFFFF_FFFF), None);
        assert_eq!(by_id("NOT_A_CARD"), None);
        assert_eq!(by_name("Not A Card"), None);
    }

    #[test]
    fn a_known_card_reads_correctly() {
        // Fireball is the least likely card in Hearthstone to be re-costed.
        let fb = by_name("Fireball").expect("Fireball is in the corpus");
        let d = fb.def();
        assert_eq!(d.kind(), Kind::Spell);
        assert_eq!(d.class(), Class::Mage);
        assert_eq!(d.cost, 4);
        assert!(d.collectible);
        assert!(fb.info().text.contains("damage"));
    }

    #[test]
    fn keyword_bits_decode() {
        // A vanilla taunt minion: Goldshire Footman is 1/2 Taunt.
        let c = by_name("Goldshire Footman").expect("in corpus");
        assert!(c.def().keywords.has(Keywords::TAUNT));
        assert!(!c.def().keywords.has(Keywords::CHARGE));
    }

    #[test]
    fn race_bits_decode() {
        let c = by_name("Bloodfen Raptor").expect("in corpus");
        assert!(c.def().races.has(Races::BEAST));
        assert!(!c.def().races.has(Races::DRAGON));
    }

    #[test]
    fn spell_damage_is_the_cards_own_value() {
        // Taken from the corpus field, not parsed out of the card text. The
        // text also matches every card that *hands out* Spell Damage rather
        // than carrying it, which made 52 collectible minions a permanent +1:
        // Battlefield Blaster gives a spell in hand +1, Bru'kan gives it to
        // Nature spells only, and Time-Twisted Seer only while damaged.
        for (name, expected) in [
            ("Bloodmage Thalnos", 1),
            ("Battlefield Blaster", 0),
            ("Archmage Kalec", 0),
            ("Bru'kan", 0),
            ("Dalaran Aspirant", 0),
            ("Time-Twisted Seer", 0),
        ] {
            let c = by_name(name).unwrap_or_else(|| panic!("{name} is in the corpus"));
            assert_eq!(c.def().spell_damage, expected, "{name}");
        }
    }

    #[test]
    fn overload_is_the_cards_own_value() {
        for (name, expected) in [
            ("Totem Golem", 1),
            ("Feral Spirit", 1),
            ("Elemental Destruction", 2),
        ] {
            let c = by_name(name).unwrap_or_else(|| panic!("{name} is in the corpus"));
            assert_eq!(c.def().overload, expected, "{name}");
        }
    }

    #[test]
    fn keywords_survive_a_card_carddefs_has_never_seen() {
        // The live API serves a new set before the local CardDefs snapshot
        // knows it exists. Those cards carry no mechanics at all, and their
        // keywords arrive under the API's own list instead -- without that
        // fallback a plain Reborn minion enters the table as a vanilla body
        // that quietly does not come back.
        let c = by_name("Sinful Steed").expect("in corpus");
        assert!(
            c.def().keywords.has(Keywords::REBORN),
            "a card the snapshot cannot describe lost its keywords"
        );
    }

    #[test]
    fn children_are_the_tokens_a_card_creates() {
        // Straight from the live card library. Animal Companion is the clean
        // case: three minions, and nothing else.
        let ac = by_name("Animal Companion").expect("in corpus");
        let summons: Vec<&str> = ac.summonable_children().map(|c| c.name()).collect();
        assert_eq!(summons, ["Huffer", "Leokk", "Misha"]);
    }

    #[test]
    fn every_child_index_is_in_range() {
        assert_eq!(CHILD_SLICES.len(), DEFS.len());
        for id in all() {
            for child in id.children() {
                assert!(
                    (child.0 as usize) < DEFS.len(),
                    "{} points at a card outside the table",
                    id.info().id
                );
            }
        }
    }

    #[test]
    fn format_bits_are_disjoint_from_nothing_but_consistent() {
        // Standard is a subset of Wild: anything Standard-legal is Wild-legal.
        for id in all() {
            let f = id.def().formats;
            if f.has(Formats::STANDARD) {
                assert!(
                    f.has(Formats::WILD),
                    "{} is Standard but not Wild",
                    id.info().id
                );
            }
        }
    }

    #[test]
    fn name_lookup_never_resolves_to_a_hero_portrait() {
        // Three gauntlet decks in the Python engine were once built from blank
        // hero portraits that shared a name with a real card.
        // Some names belong to nothing but portraits (a hero skin and a
        // Battlegrounds skin can share one), and resolving those to a portrait
        // is correct. The property that matters is narrower: a portrait must
        // never win over a real card.
        for (name, idx) in BY_NAME.iter() {
            let id = CardId(*idx);
            if !is_portrait(id) {
                continue;
            }
            let real = all().find(|o| o.info().name == *name && !is_portrait(*o));
            assert!(
                real.is_none(),
                "{name} resolved to a portrait while {:?} is a real card",
                real.map(|r| r.info().id)
            );
        }
    }

    fn is_portrait(id: CardId) -> bool {
        id.def().kind() == Kind::Hero && id.info().set == "HERO_SKINS"
    }

    #[test]
    fn copy_limit_follows_rarity() {
        let leg = all()
            .find(|c| c.def().rarity() == Rarity::Legendary)
            .unwrap();
        let common = all().find(|c| c.def().rarity() == Rarity::Common).unwrap();
        assert_eq!(leg.def().copy_limit(), 1);
        assert_eq!(common.def().copy_limit(), 2);
    }
}
