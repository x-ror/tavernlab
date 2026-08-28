//! What the log adds up to: one game, as far as it can be seen.
//!
//! Deliberately only what the log states outright. A tracker that guesses at
//! what it cannot see would hand back confident advice about a board that is
//! not there, which is the one failure this project is built to avoid — so
//! everything here is either read from a line or left unknown, and the
//! command prints the picture it built beside the advice it gives.

use super::log::{EntityTag, Event, ZoneMove};
use tavernlab_core::cards::{CardId, Class, Keywords, by_id};

/// A minion the log has put on a board.
#[derive(Clone, Debug)]
pub struct Body {
    pub entity: u32,
    pub card: CardId,
    /// The turn counter when it entered this zone.
    ///
    /// A minion that landed this turn is still summoning sick, and one with
    /// Rush may trade but not go face. The advice is worthless without it:
    /// the first real logs had the watcher telling the player to swing with
    /// the minion they had just put down.
    pub turn: u16,
    /// Attack and maximum Health as the log last stated them, or `None` while
    /// it has said nothing and the printed numbers still stand.
    ///
    /// `TAG_CHANGE` is written when a value *changes*, so silence means the
    /// card is what the corpus says it is -- and a buffed minion says so.
    pub atk: Option<i16>,
    pub hp: Option<i16>,
    pub damage: i16,
    /// Live keywords: the printed set, then whatever the log granted or took
    /// away on top of it. Started from the card so a body the log says
    /// nothing about is still its printed self.
    pub keywords: Keywords,
    pub frozen: bool,
    pub attacks: u8,
}

impl Body {
    fn new(entity: u32, card: CardId, turn: u16) -> Body {
        Body {
            entity,
            card,
            turn,
            atk: None,
            hp: None,
            damage: 0,
            keywords: card.def().keywords,
            frozen: false,
            attacks: 0,
        }
    }

    /// The card behind a face-down entity, once the log has shown it.
    ///
    /// The printed keywords come with it: a body that was `UNKNOWN ENTITY`
    /// started from an empty card and would otherwise keep an empty keyword
    /// set for the rest of the game.
    fn reveal(&mut self, card: CardId) {
        let granted = self.keywords;
        self.card = card;
        self.keywords = card.def().keywords;
        // Anything the log granted while it was face down still holds.
        self.keywords.insert(granted);
    }

    /// Attack and health right now, falling back to the printed numbers.
    pub fn stats(&self) -> (i16, i16) {
        let d = self.card.def();
        (
            self.atk.unwrap_or(d.atk),
            self.hp.unwrap_or(d.hp) - self.damage,
        )
    }

    fn apply(&mut self, what: EntityTag) {
        match what {
            EntityTag::Atk(n) => self.atk = Some(n),
            EntityTag::Health(n) => self.hp = Some(n),
            EntityTag::Damage(n) => self.damage = n,
            EntityTag::Attacks(n) => self.attacks = n,
            EntityTag::Armor(_) => {}
            EntityTag::Keyword("FROZEN", on) => self.frozen = on,
            EntityTag::Keyword(name, on) => {
                let Some(k) = keyword_of(name) else { return };
                if on {
                    self.keywords.insert(k);
                } else {
                    self.keywords.remove(k);
                }
            }
        }
    }
}

/// The log's name for a keyword, as this engine spells it.
fn keyword_of(tag: &str) -> Option<Keywords> {
    Some(match tag {
        "TAUNT" => Keywords::TAUNT,
        "DIVINE_SHIELD" => Keywords::DIVINE_SHIELD,
        "STEALTH" => Keywords::STEALTH,
        "CHARGE" => Keywords::CHARGE,
        "RUSH" => Keywords::RUSH,
        "WINDFURY" => Keywords::WINDFURY,
        "LIFESTEAL" => Keywords::LIFESTEAL,
        "POISONOUS" => Keywords::POISONOUS,
        "REBORN" => Keywords::REBORN,
        // The game's name for Elusive.
        "CANT_BE_TARGETED_BY_SPELLS" => Keywords::ELUSIVE,
        "CANT_ATTACK" => Keywords::CANT_ATTACK,
        "IMMUNE" => Keywords::IMMUNE,
        _ => return None,
    })
}

/// A hero, as far as the log states it.
#[derive(Clone, Copy, Debug)]
pub struct Hero {
    pub entity: Option<u32>,
    /// Maximum Health. Thirty until a line says otherwise, which is what
    /// every hero starts at and what a hero card changes.
    pub max_hp: i16,
    pub damage: i16,
    pub armor: i16,
}

impl Default for Hero {
    fn default() -> Hero {
        Hero {
            entity: None,
            max_hp: 30,
            damage: 0,
            armor: 0,
        }
    }
}

impl Hero {
    pub fn health(&self) -> i16 {
        self.max_hp - self.damage
    }

    fn apply(&mut self, what: EntityTag) {
        match what {
            EntityTag::Health(n) => self.max_hp = n,
            EntityTag::Damage(n) => self.damage = n,
            EntityTag::Armor(n) => self.armor = n,
            _ => {}
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Tracker {
    /// The player number whose client wrote this log, once a FRIENDLY line
    /// has named it.
    pub me: Option<u8>,
    /// My battletag, as the `TAG_CHANGE` lines spell it. The log never says
    /// which of the two names is the one holding it, so this is supplied
    /// (`--me`, `HS_ME`) -- the same thing the project's earlier reader
    /// required, and for the same reason. Without it the mana and
    /// whose-turn-it-is lines cannot be attributed and are left unknown
    /// rather than guessed.
    pub me_name: Option<String>,
    pub classes: [Option<Class>; 2],
    /// My opening hand, in the order the log dealt it.
    pub opening: Vec<CardId>,
    /// My hand right now.
    pub hand: Vec<Body>,
    pub board: [Vec<Body>; 2],
    /// Both heroes: health, armour, and the entity the log names them by.
    pub heroes: [Hero; 2],
    /// Every player name the log has used on a line that needs one, in the
    /// order they first appeared. Printed when `me_name` matched none of
    /// them, because the fix is to pass one of these and the user cannot
    /// guess which spelling the client chose.
    pub names: Vec<String>,
    /// Cards each player has put into play or cast, oldest first. What the
    /// opponent read is built from.
    pub played: [Vec<CardId>; 2],
    pub turn: u16,
    pub my_turn: bool,
    /// Mana crystals and mana spent, for me. `None` until a `RESOURCES` line
    /// could be attributed -- see `me_name`.
    pub crystals: Option<i16>,
    pub spent: i16,
    pub over: bool,
    /// True once the mulligan is done and the game proper has started.
    pub started: bool,
}

fn card_of(card_id: &str) -> Option<CardId> {
    if card_id.is_empty() {
        return None;
    }
    by_id(card_id)
}

impl Tracker {
    pub fn new(me_name: Option<String>) -> Tracker {
        Tracker {
            me_name,
            ..Tracker::default()
        }
    }

    fn index(&self, player: u8) -> Option<usize> {
        let me = self.me?;
        Some(if player == me { 0 } else { 1 })
    }

    /// My side, then theirs — `0` is always the player holding the log.
    pub fn opponent_class(&self) -> Option<Class> {
        self.classes[1]
    }

    pub fn my_class(&self) -> Option<Class> {
        self.classes[0]
    }

    pub fn mana_left(&self) -> Option<i16> {
        Some((self.crystals? - self.spent).max(0))
    }

    /// Whether a name the log used is the one `--me` named.
    ///
    /// Exact first. The fallback compares only the part before `#`, without
    /// case: the client does not always write the numeric half, and a
    /// battletag that is right except for that is not worth losing every
    /// mana line over. Ambiguous only if both players share a base name.
    fn is_me(&self, name: &str) -> bool {
        let Some(mine) = self.me_name.as_deref() else {
            return false;
        };
        if mine == name {
            return true;
        }
        let base = |s: &str| s.split('#').next().unwrap_or(s).to_ascii_lowercase();
        !name.is_empty() && base(mine) == base(name)
    }

    fn note_name(&mut self, name: &str) {
        if !name.is_empty() && !self.names.iter().any(|n| n == name) {
            self.names.push(name.to_string());
        }
    }

    pub fn feed(&mut self, ev: Event) {
        match ev {
            Event::NewGame => *self = Tracker::new(self.me_name.clone()),
            Event::Turn(t) => {
                if t > 0 {
                    self.started = true;
                }
                self.turn = t;
            }
            Event::Result { player_name, .. } => {
                self.note_name(&player_name);
                self.over = true;
            }
            Event::CurrentPlayer {
                player_name,
                current,
            } => {
                self.note_name(&player_name);
                if self.me_name.is_none() {
                    return;
                }
                if self.is_me(&player_name) {
                    self.my_turn = current;
                } else if current {
                    self.my_turn = false;
                }
            }
            Event::Resources {
                player_name,
                total,
                used,
            } => {
                self.note_name(&player_name);
                // Only my own crystals are actionable, and telling the two
                // players apart needs the name.
                if !self.is_me(&player_name) {
                    return;
                }
                if total >= 0 {
                    self.crystals = Some(total);
                }
                if used >= 0 {
                    self.spent = used;
                }
            }
            Event::Reveal { entity, card_id } => {
                if let Some(card) = card_of(&card_id) {
                    for side in 0..2 {
                        for b in self.board[side].iter_mut() {
                            if b.entity == entity {
                                b.reveal(card);
                            }
                        }
                    }
                    for b in self.hand.iter_mut() {
                        if b.entity == entity {
                            b.reveal(card);
                        }
                    }
                }
            }
            Event::Tag { entity, what } => {
                for h in self.heroes.iter_mut() {
                    if h.entity == Some(entity) {
                        h.apply(what);
                        return;
                    }
                }
                for side in 0..2 {
                    for b in self.board[side].iter_mut() {
                        if b.entity == entity {
                            b.apply(what);
                            return;
                        }
                    }
                }
                for b in self.hand.iter_mut() {
                    if b.entity == entity {
                        b.apply(what);
                        return;
                    }
                }
            }
            Event::Zone(m) => self.zone(m),
        }
    }

    fn zone(&mut self, m: ZoneMove) {
        let ZoneMove {
            entity,
            player,
            mine,
            ..
        } = m;
        let kind = m.kind.as_deref();
        let zone = m.zone.as_str();
        if mine && self.me.is_none() {
            self.me = Some(player);
        }
        let Some(i) = self.index(player) else { return };
        let card = card_of(&m.card_id);

        // A hero landing is how each side's class is announced.
        if kind == Some("Hero") {
            if let Some(c) = card
                && c.def().class() != Class::Neutral
            {
                self.classes[i] = Some(c.def().class());
            }
            // Which entity the health and armour lines will be about. A hero
            // card replaces the starting hero, and the new entity is the one
            // that counts from here on -- but only a different entity starts
            // over, so a hero line repeated for the same hero does not throw
            // away the damage already read.
            if self.heroes[i].entity != Some(entity) {
                self.heroes[i] = Hero {
                    entity: Some(entity),
                    ..Hero::default()
                };
            }
            return;
        }
        if kind.is_some() {
            return; // Hero Power, and anything else parenthesised.
        }

        // Leaving a zone: drop it from wherever it was.
        self.board[i].retain(|b| b.entity != entity);
        if i == 0 {
            self.hand.retain(|b| b.entity != entity);
        }

        match zone {
            "HAND" if i == 0 => {
                if let Some(c) = card {
                    self.hand.push(Body::new(entity, c, self.turn));
                    if !self.started {
                        self.opening.push(c);
                    }
                }
            }
            "PLAY" => {
                if let Some(c) = card {
                    if c.def().kind() == tavernlab_core::cards::Kind::Minion {
                        self.board[i].push(Body::new(entity, c, self.turn));
                    }
                    self.played[i].push(c);
                    if c.def().class() != Class::Neutral && self.classes[i].is_none() {
                        self.classes[i] = Some(c.def().class());
                    }
                }
            }
            // A spell goes straight to the graveyard, and is still a card
            // they played: it is what the opponent read is built from.
            "GRAVEYARD" => {
                if let Some(c) = card
                    && c.def().kind() == tavernlab_core::cards::Kind::Spell
                {
                    self.played[i].push(c);
                    if c.def().class() != Class::Neutral && self.classes[i].is_none() {
                        self.classes[i] = Some(c.def().class());
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::watch_mod::log::parse;

    fn feed(t: &mut Tracker, lines: &[&str]) {
        for l in lines {
            if let Some(ev) = parse(l) {
                t.feed(ev);
            }
        }
    }

    #[test]
    fn the_first_friendly_line_decides_which_player_i_am() {
        let mut t = Tracker::new(None);
        feed(&mut t, &[
            "[Zone] [entityName=Chillwind Yeti id=42 zone=DECK zonePos=0 \
             cardId=CS2_182 player=2] zone from OPPOSING DECK -> FRIENDLY HAND",
        ]);
        assert_eq!(t.me, Some(2));
        assert_eq!(t.hand.len(), 1);
        assert_eq!(t.opening.len(), 1, "before the first turn it is the opening hand");
    }

    #[test]
    fn heroes_announce_both_classes() {
        let mut t = Tracker::new(None);
        feed(&mut t, &[
            "[Zone] [entityName=Jaina Proudmoore id=64 zone=PLAY zonePos=0 \
             cardId=HERO_08 player=1] zone from  -> FRIENDLY PLAY (Hero)",
            "[Zone] [entityName=Garrosh Hellscream id=65 zone=PLAY zonePos=0 \
             cardId=HERO_01 player=2] zone from  -> OPPOSING PLAY (Hero)",
        ]);
        assert_eq!(t.my_class(), Some(Class::Mage));
        assert_eq!(t.opponent_class(), Some(Class::Warrior));
    }

    #[test]
    fn a_minion_played_leaves_hand_and_joins_the_board() {
        let mut t = Tracker::new(None);
        feed(&mut t, &[
            "[Zone] [entityName=Chillwind Yeti id=42 zone=DECK zonePos=0 \
             cardId=CS2_182 player=1] zone from FRIENDLY DECK -> FRIENDLY HAND",
            "D [Power] TAG_CHANGE Entity=GameEntity tag=TURN value=1",
            "[Zone] [entityName=Chillwind Yeti id=42 zone=HAND zonePos=1 \
             cardId=CS2_182 player=1] zone from FRIENDLY HAND -> FRIENDLY PLAY",
        ]);
        assert!(t.hand.is_empty(), "it left hand");
        assert_eq!(t.board[0].len(), 1);
        assert_eq!(t.board[0][0].card.name(), "Chillwind Yeti");
        assert_eq!(t.played[0].len(), 1);
    }

    #[test]
    fn the_opening_hand_stops_growing_once_the_game_starts() {
        let mut t = Tracker::new(None);
        feed(&mut t, &[
            "[Zone] [entityName=Wisp id=1 zone=DECK zonePos=0 cardId=CS2_231 \
             player=1] zone from FRIENDLY DECK -> FRIENDLY HAND",
            "D [Power] TAG_CHANGE Entity=GameEntity tag=TURN value=1",
            "[Zone] [entityName=Fireball id=2 zone=DECK zonePos=0 cardId=CS2_029 \
             player=1] zone from FRIENDLY DECK -> FRIENDLY HAND",
        ]);
        assert_eq!(t.opening.len(), 1, "only what was dealt before turn one");
        assert_eq!(t.hand.len(), 2, "but the hand keeps up");
    }

    #[test]
    fn my_mana_is_read_and_the_opponents_is_not() {
        let mut t = Tracker::new(Some("Me#1".into()));
        feed(&mut t, &[
            "D [Power] TAG_CHANGE Entity=Me#1 tag=RESOURCES value=7",
            "D [Power] TAG_CHANGE Entity=Me#1 tag=RESOURCES_USED value=3",
            "D [Power] TAG_CHANGE Entity=Them#2 tag=RESOURCES value=10",
        ]);
        assert_eq!(t.crystals, Some(7));
        assert_eq!(t.mana_left(), Some(4));
    }

    #[test]
    fn a_battletag_without_its_numbers_is_still_me() {
        // The client does not always write the `#12345` half, and losing
        // every mana line over that is what a real session did.
        let mut t = Tracker::new(Some("xror#21652".into()));
        feed(&mut t, &[
            "D [Power] TAG_CHANGE Entity=xror tag=RESOURCES value=6",
            "D [Power] TAG_CHANGE Entity=xror tag=RESOURCES_USED value=2",
            "D [Power] TAG_CHANGE Entity=SomeoneElse#9 tag=RESOURCES value=10",
        ]);
        assert_eq!(t.mana_left(), Some(4));
        assert_eq!(
            t.names,
            vec!["xror".to_string(), "SomeoneElse#9".to_string()],
            "and both names are kept, to print when nothing matched"
        );
    }

    #[test]
    fn the_log_overrides_the_printed_stats() {
        let mut t = Tracker::new(None);
        feed(&mut t, &[
            "[Zone] [entityName=Chillwind Yeti id=42 zone=HAND zonePos=1 \
             cardId=CS2_182 player=1] zone from FRIENDLY HAND -> FRIENDLY PLAY",
            "D [Power] TAG_CHANGE Entity=[entityName=Chillwind Yeti id=42 \
             zone=PLAY zonePos=1 cardId=CS2_182 player=1] tag=ATK value=6",
            "D [Power] TAG_CHANGE Entity=[entityName=Chillwind Yeti id=42 \
             zone=PLAY zonePos=1 cardId=CS2_182 player=1] tag=DAMAGE value=2",
            "D [Power] TAG_CHANGE Entity=[entityName=Chillwind Yeti id=42 \
             zone=PLAY zonePos=1 cardId=CS2_182 player=1] tag=TAUNT value=1",
        ]);
        let b = &t.board[0][0];
        assert_eq!(b.stats(), (6, 3), "a 4/5 buffed to 6 attack with 2 damage on it");
        assert!(b.keywords.has(Keywords::TAUNT), "granted, not printed");
    }

    #[test]
    fn a_silenced_keyword_goes_away() {
        let mut t = Tracker::new(None);
        const PLAY: &str = "[Zone] [entityName=Goldshire Footman id=7 \
             zone=HAND zonePos=1 cardId=CS1_042 player=1] zone from \
             FRIENDLY HAND -> FRIENDLY PLAY";
        feed(&mut t, &[PLAY]);
        assert!(
            t.board[0][0].keywords.has(Keywords::TAUNT),
            "Goldshire Footman prints Taunt, so the silence below has \
             something to take away"
        );
        feed(&mut t, &[
            "D [Power] TAG_CHANGE Entity=[entityName=Goldshire Footman id=7 \
             zone=PLAY zonePos=1 cardId=CS1_042 player=1] tag=TAUNT value=0",
        ]);
        assert!(!t.board[0][0].keywords.has(Keywords::TAUNT));
    }

    #[test]
    fn the_heroes_health_and_armour_are_read() {
        let mut t = Tracker::new(None);
        feed(&mut t, &[
            "[Zone] [entityName=Jaina Proudmoore id=64 zone=PLAY zonePos=0 \
             cardId=HERO_08 player=1] zone from  -> FRIENDLY PLAY (Hero)",
            "[Zone] [entityName=Garrosh Hellscream id=65 zone=PLAY zonePos=0 \
             cardId=HERO_01 player=2] zone from  -> OPPOSING PLAY (Hero)",
            "D [Power] TAG_CHANGE Entity=64 tag=DAMAGE value=8",
            "D [Power] TAG_CHANGE Entity=65 tag=ARMOR value=5",
        ]);
        assert_eq!(t.heroes[0].health(), 22);
        assert_eq!(t.heroes[0].armor, 0);
        assert_eq!(t.heroes[1].health(), 30);
        assert_eq!(t.heroes[1].armor, 5);
    }

    #[test]
    fn a_new_game_wipes_the_last_one() {
        let mut t = Tracker::new(None);
        feed(&mut t, &[
            "[Zone] [entityName=Wisp id=1 zone=DECK zonePos=0 cardId=CS2_231 \
             player=1] zone from FRIENDLY DECK -> FRIENDLY HAND",
            "D [Power] CREATE_GAME",
        ]);
        assert!(t.hand.is_empty());
        assert!(t.me.is_none());
    }
}
