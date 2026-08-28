//! What the log adds up to: one game, as far as it can be seen.
//!
//! Deliberately only what the log states outright. A tracker that guesses at
//! what it cannot see would hand back confident advice about a board that is
//! not there, which is the one failure this project is built to avoid — so
//! everything here is either read from a line or left unknown, and the
//! command prints the picture it built beside the advice it gives.

use super::log::{Event, ZoneMove};
use tavernlab_core::cards::{CardId, Class, by_id};

/// A minion the log has put on a board.
#[derive(Clone, Debug)]
pub struct Body {
    pub entity: u32,
    pub card: CardId,
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

    pub fn feed(&mut self, ev: Event) {
        match ev {
            Event::NewGame => *self = Tracker::new(self.me_name.clone()),
            Event::Turn(t) => {
                if t > 0 {
                    self.started = true;
                }
                self.turn = t;
            }
            Event::Result { .. } => self.over = true,
            Event::CurrentPlayer {
                player_name,
                current,
            } => {
                let Some(mine) = self.me_name.as_deref() else {
                    return;
                };
                if mine == player_name {
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
                // Only my own crystals are actionable, and telling the two
                // players apart needs the name.
                if self.me_name.as_deref() != Some(player_name.as_str()) {
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
                                b.card = card;
                            }
                        }
                    }
                    for b in self.hand.iter_mut() {
                        if b.entity == entity {
                            b.card = card;
                        }
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
                    self.hand.push(Body { entity, card: c });
                    if !self.started {
                        self.opening.push(c);
                    }
                }
            }
            "PLAY" => {
                if let Some(c) = card {
                    if c.def().kind() == tavernlab_core::cards::Kind::Minion {
                        self.board[i].push(Body { entity, card: c });
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
