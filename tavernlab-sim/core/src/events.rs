//! Triggered effects.
//!
//! A trigger is a card that reacts to something happening elsewhere: at the end
//! of your turn, whenever this takes damage, after you cast a spell. They are
//! the largest single category of card text after plain battlecries, so the
//! dispatch has to be cheap and, more importantly, has to be *one* mechanism —
//! a second ad-hoc hook for "end of turn" would drift out of step with the
//! first one within a dozen cards.
//!
//! # Why there is no listener registry
//!
//! The obvious design is a list of subscriptions maintained as minions enter
//! and leave play. The Python engine did that and paid for it twice: the list
//! had to survive cloning, which is what forced its handler-id indirection, and
//! it could go stale.
//!
//! Here [`Game::fire`] simply walks both boards — at most fourteen minions —
//! and asks each card whether it cares. That is fourteen array lookups and
//! fourteen predictable branches, against a game that costs roughly eighteen
//! microseconds; a subscription list would be bookkeeping to avoid work that
//! does not measurably exist. Nothing to keep in sync, nothing to clone.

use crate::cards::{CardId, behaviour_of};
use crate::inline::Inline;
use crate::state::{Game, MAX_BOARD, MAX_SECRETS, Side, Target};

/// Something that happened, which a card may react to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Event {
    /// The start of `side`'s turn, after mana and the draw.
    TurnStart {
        side: Side,
    },
    /// The end of `side`'s turn, before frozen characters thaw.
    TurnEnd {
        side: Side,
    },
    /// A minion entered play, however it got there. `slot` is where it
    /// landed, so a reactor can tell its own arrival from anyone else's —
    /// Knife Juggler does not throw a knife for itself.
    MinionSummoned {
        side: Side,
        card: CardId,
        slot: u8,
    },
    /// A card was played from hand — any kind.
    CardPlayed {
        side: Side,
        card: CardId,
    },
    /// A spell is about to resolve. The only event a secret may cancel:
    /// Counterspell sets [`Game::countered`] here and the cast is abandoned.
    SpellCasting {
        side: Side,
        card: CardId,
    },
    /// A spell finished resolving.
    SpellCast {
        side: Side,
        card: CardId,
    },
    /// The exchange is over and both sides have taken their damage. This is
    /// what "after your hero attacks" listens to.
    AfterAttack {
        attacker: Target,
        defender: Target,
        /// Whether `defender` died in this exchange, captured before the
        /// death sweep removes it -- by the time this event fires the board
        /// no longer holds the body, so this is the only way a reactor can
        /// tell "after your hero attacks and kills a minion" from a miss.
        defender_died: bool,
    },
    /// An attack has been declared but no damage has landed yet. Secrets that
    /// react to being attacked get their chance here, and may remove the
    /// attacker or the defender before the exchange happens.
    AttackDeclared {
        attacker: Target,
        defender: Target,
    },
    /// A minion left play by dying.
    MinionDied {
        side: Side,
        card: CardId,
    },
    /// Damage actually landed. A popped Divine Shield does not count.
    Damaged {
        target: Target,
        amount: i16,
    },
    /// Healing actually restored something.
    Healed {
        target: Target,
        amount: i16,
    },
    CardDrawn {
        side: Side,
    },
    HeroPowerUsed {
        side: Side,
    },
    /// A Discover finished: one card was taken and the rest were let go.
    ///
    /// `others` carries the options that were *not* taken, padded with
    /// `CardId(0)`, because The Origin Stone plays them and they exist
    /// nowhere else -- a Discover throws them away the instant it picks.
    Discovered {
        side: Side,
        others: [CardId; 2],
    },
}

impl Event {
    /// The player this event belongs to, for "your"/"enemy" wording.
    pub fn actor(self) -> Option<Side> {
        match self {
            Event::TurnStart { side }
            | Event::TurnEnd { side }
            | Event::MinionSummoned { side, .. }
            | Event::CardPlayed { side, .. }
            | Event::Discovered { side, .. }
            | Event::SpellCasting { side, .. }
            | Event::SpellCast { side, .. }
            | Event::MinionDied { side, .. }
            | Event::CardDrawn { side }
            | Event::HeroPowerUsed { side } => Some(side),
            // An attack belongs to whoever controls the attacker.
            Event::AttackDeclared { attacker, .. } | Event::AfterAttack { attacker, .. } => {
                Some(match attacker {
                    Target::Hero(s) | Target::Minion(s, _) => s,
                })
            }
            Event::Damaged { .. } | Event::Healed { .. } => None,
        }
    }
}

/// What a triggered effect is told.
#[derive(Clone, Copy, Debug)]
pub struct TriggerCtx {
    /// Controller of the reacting permanent.
    pub side: Side,
    /// Its board slot at the moment the trigger fires.
    pub slot: u8,
    pub event: Event,
}

impl TriggerCtx {
    /// The reacting permanent itself, as a target.
    #[inline]
    pub fn me(&self) -> Target {
        Target::Minion(self.side, self.slot)
    }

    /// Whether the event was caused by the reacting card's own controller.
    #[inline]
    pub fn mine(&self) -> bool {
        self.event.actor() == Some(self.side)
    }

    /// Whether the event happened to the reacting card itself.
    pub fn hit_me(&self) -> bool {
        match self.event {
            Event::Damaged { target, .. } | Event::Healed { target, .. } => target == self.me(),
            _ => false,
        }
    }
}

/// A triggered effect.
pub type Trigger = fn(&mut Game, &TriggerCtx);

/// How deep a chain of triggers may go before the engine stops.
///
/// Two triggers that feed each other would otherwise spin forever. Real cards
/// nest two or three deep at most, so this is a backstop against a bug rather
/// than a rule of the game.
const MAX_TRIGGER_DEPTH: u8 = 8;

/// Slot value marking a reactor as the equipped weapon rather than a board
/// position. A weapon has no slot, but it does react.
pub const WEAPON_SLOT: u8 = u8::MAX;
/// Slot value marking a reactor as the active Quest.
pub const QUEST_SLOT: u8 = u8::MAX - 1;
/// Slot value marking a reactor as the active Sidequest.
pub const SIDE_QUEST_SLOT: u8 = u8::MAX - 2;
/// Slot value marking a reactor as the equipped Hero Power. A Hero Power is
/// not a permanent either, but one of them reacts: Collapsing Star refreshes
/// itself whenever its owner summons a Demon.
pub const HERO_POWER_SLOT: u8 = u8::MAX - 3;

impl Game {
    /// Tell every permanent in play that `event` happened.
    ///
    /// The board is sampled first and each reactor re-checked before it fires,
    /// because an earlier trigger in the same sweep can kill a later one. A
    /// minion that died mid-sweep does not get to react.
    ///
    /// Deliberately does **not** remove dead bodies. Firing an event in the
    /// middle of an area effect would otherwise shift every board slot the
    /// effect had already collected — which is how Flamestrike came to hit the
    /// wrong minions. The engine sweeps once the current effect is finished.
    pub fn fire(&mut self, event: Event) {
        if self.trigger_depth >= MAX_TRIGGER_DEPTH || self.is_over() {
            return;
        }
        // Warptooth reacts from hand or deck, not from a board slot, and
        // Shadow of Demise reacts from hand alone -- neither can use the
        // reactor mechanism below at all. These are the deliberate
        // exceptions to "no ad-hoc hooks" this module otherwise holds to.
        // See `Game::tick_warptooth` and `Game::tick_shadow_of_demise`.
        if let Event::Damaged { target, .. } = event {
            let side = match target {
                Target::Hero(s) | Target::Minion(s, _) => s,
            };
            if side == self.current {
                self.tick_warptooth(side);
            }
        }
        if let Event::SpellCast { side, card } = event {
            self.tick_shadow_of_demise(side, card);
        }
        // Sample as (side, slot, card) rather than holding a reference: the
        // effects below take `&mut self`.
        let mut reactors: Inline<(Side, u8, CardId), { MAX_BOARD * 2 + 6 }> = Inline::new();
        // "Does this card react?" asked of up to twenty cards per event, and
        // an event fires on nearly every action in the game. The hook table
        // answers it from a byte instead of a walk into `BEHAVIOURS`; the
        // behaviour itself is fetched below, only for the few that do.
        let hooks = crate::cards::hooks();
        let reacts = |card: CardId| hooks[card.0 as usize] & crate::cards::HAS_TRIGGER != 0;
        for i in 0..2 {
            let side = Side::from_index(i);
            for (slot, m) in self.players[i].board.iter().enumerate() {
                // A card being played does not react to its own play; see
                // `Flags::BEING_PLAYED`. Every other event reaches it.
                if matches!(event, Event::CardPlayed { .. })
                    && m.flags.has(crate::state::Flags::BEING_PLAYED)
                {
                    continue;
                }
                if m.active() && reacts(m.card) {
                    reactors.push((side, slot as u8, m.card));
                }
            }
        }
        // Weapons react too: "after your hero attacks" is printed on more
        // weapons than minions.
        for i in 0..2 {
            if let Some(w) = self.players[i].weapon
                && reacts(w.card)
            {
                reactors.push((Side::from_index(i), WEAPON_SLOT, w.card));
            }
        }
        // A Quest or Sidequest tracks its own progress through this same
        // hook -- neither is a board permanent, so each gets a sentinel slot
        // of its own, the same trick the weapon above uses.
        for i in 0..2 {
            let side = Side::from_index(i);
            if let Some((card, _)) = self.players[i].quest
                && reacts(card)
            {
                reactors.push((side, QUEST_SLOT, card));
            }
            if let Some((card, _)) = self.players[i].sidequest
                && reacts(card)
            {
                reactors.push((side, SIDE_QUEST_SLOT, card));
            }
            // And the Hero Power, for the same reason: it is always in play
            // and it is never on the board.
            let hp = self.players[i].hero_power;
            if reacts(hp) {
                reactors.push((side, HERO_POWER_SLOT, hp));
            }
        }

        // No early return here: secrets are a separate zone and must get their
        // chance even when nothing on either board reacts.
        self.trigger_depth += 1;
        for (side, slot, card) in reactors.iter().copied() {
            // Still there, still the same card, still alive?
            let still_there = if slot == WEAPON_SLOT {
                self.player(side).weapon.is_some_and(|w| w.card == card)
            } else if slot == QUEST_SLOT {
                self.player(side).quest.is_some_and(|(c, _)| c == card)
            } else if slot == SIDE_QUEST_SLOT {
                self.player(side).sidequest.is_some_and(|(c, _)| c == card)
            } else if slot == HERO_POWER_SLOT {
                // A Hero Power can be replaced mid-sweep (Soul Immolation
                // swaps one in), so this is checked like any other reactor.
                self.player(side).hero_power == card
            } else {
                self.player(side)
                    .board
                    .get(slot as usize)
                    .is_some_and(|m| m.card == card && m.active())
            };
            if !still_there {
                continue;
            }
            if let Some(f) = behaviour_of(card).and_then(|b| b.trigger) {
                f(self, &TriggerCtx { side, slot, event });
                // Sandfury Aura: "Your minions' end of turn effects trigger
                // twice." A modifier on this sweep rather than an effect of
                // its own, because the only place that can run a trigger a
                // second time is the place that ran it the first.
                //
                // "Your minions'" is read strictly: a board slot, on its own
                // controller's end of turn. A weapon, a Quest or a Hero Power
                // reacting to the same event is not a minion and fires once.
                if matches!(event, Event::TurnEnd { side: whose } if whose == side)
                    && slot < MAX_BOARD as u8
                    && self.doubles_end_of_turn(side)
                    && self
                        .player(side)
                        .board
                        .get(slot as usize)
                        .is_some_and(|m| m.card == card && m.active())
                {
                    f(self, &TriggerCtx { side, slot, event });
                }
            }
            if self.is_over() {
                break;
            }
        }
        self.trigger_depth -= 1;
        self.fire_secrets(event);
    }

    /// Give both players' secrets a chance at `event`, popping any that fire.
    ///
    /// Secrets live in a player's zone rather than on the board, so they are a
    /// second sweep rather than part of the first — but they use the same
    /// events, so a new event kind needs no extra plumbing here either.
    fn fire_secrets(&mut self, event: Event) {
        if self.trigger_depth >= MAX_TRIGGER_DEPTH || self.is_over() {
            return;
        }
        let mut armed: Inline<(Side, CardId), { MAX_SECRETS * 2 }> = Inline::new();
        for i in 0..2 {
            let side = Side::from_index(i);
            for card in self.players[i].secrets.iter() {
                if behaviour_of(*card).and_then(|b| b.secret).is_some() {
                    armed.push((side, *card));
                }
            }
        }
        if armed.is_empty() {
            return;
        }
        self.trigger_depth += 1;
        for (side, card) in armed.iter().copied() {
            // Still armed? An earlier secret in the same sweep may have gone
            // off and, through its effect, removed this one.
            if !self.player(side).secrets.contains(&card) {
                continue;
            }
            if let Some(f) = behaviour_of(card).and_then(|b| b.secret)
                && f(self, side, event)
            {
                self.player_mut(side).secrets.remove_value(&card);
            }
            if self.is_over() {
                break;
            }
        }
        self.trigger_depth -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cards::{Class, by_name};
    use crate::state::{Flags, Permanent};

    fn game_with(mine: &[&str]) -> Game {
        let mut g = Game::new((Class::Mage, &[]), (Class::Mage, &[]), 1).unwrap();
        for n in mine {
            let mut m = Permanent::summon(by_name(n).unwrap());
            m.flags.remove(Flags::JUST_SUMMONED);
            g.players[0].board.push(m);
        }
        g
    }

    #[test]
    fn firing_with_no_reactors_is_a_no_op() {
        let mut g = game_with(&["Bloodfen Raptor"]);
        let before = g.players[0].board[0];
        g.fire(Event::TurnEnd {
            side: Side::Player0,
        });
        assert_eq!(g.players[0].board[0].atk, before.atk);
    }

    #[test]
    fn a_dormant_minion_does_not_react() {
        let mut g = game_with(&["Frail Ghoul"]); // dies at end of its turn
        g.players[0].board[0].flags.insert(Flags::DORMANT);
        g.fire(Event::TurnEnd {
            side: Side::Player0,
        });
        assert_eq!(
            g.players[0].board.len(),
            1,
            "a dormant minion is not in play"
        );
    }

    #[test]
    fn ctx_reports_ownership_and_self_hits() {
        let ctx = TriggerCtx {
            side: Side::Player0,
            slot: 2,
            event: Event::TurnEnd {
                side: Side::Player0,
            },
        };
        assert!(ctx.mine());
        assert!(!ctx.hit_me());
        assert_eq!(ctx.me(), Target::Minion(Side::Player0, 2));

        let other = TriggerCtx {
            event: Event::TurnEnd {
                side: Side::Player1,
            },
            ..ctx
        };
        assert!(!other.mine());

        let hit = TriggerCtx {
            event: Event::Damaged {
                target: Target::Minion(Side::Player0, 2),
                amount: 3,
            },
            ..ctx
        };
        assert!(hit.hit_me());
        assert!(!hit.mine(), "damage has no actor");
    }

    #[test]
    fn events_report_their_actor() {
        assert_eq!(
            Event::TurnStart {
                side: Side::Player1
            }
            .actor(),
            Some(Side::Player1)
        );
        assert_eq!(
            Event::Damaged {
                target: Target::Hero(Side::Player0),
                amount: 1
            }
            .actor(),
            None
        );
    }

    #[test]
    fn trigger_recursion_is_bounded() {
        // The depth guard is what stops two mutually-feeding triggers from
        // hanging a batch run. Firing from inside a trigger must terminate.
        let mut g = game_with(&["Bloodfen Raptor"]);
        g.trigger_depth = MAX_TRIGGER_DEPTH;
        g.fire(Event::TurnEnd {
            side: Side::Player0,
        });
        assert_eq!(
            g.trigger_depth, MAX_TRIGGER_DEPTH,
            "depth must be left as found"
        );
    }
}
