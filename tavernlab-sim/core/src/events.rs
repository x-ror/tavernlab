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
}

impl Event {
    /// The player this event belongs to, for "your"/"enemy" wording.
    pub fn actor(self) -> Option<Side> {
        match self {
            Event::TurnStart { side }
            | Event::TurnEnd { side }
            | Event::MinionSummoned { side, .. }
            | Event::CardPlayed { side, .. }
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
        // Sample as (side, slot, card) rather than holding a reference: the
        // effects below take `&mut self`.
        let mut reactors: Inline<(Side, u8, CardId), { MAX_BOARD * 2 + 2 }> = Inline::new();
        for i in 0..2 {
            let side = Side::from_index(i);
            for (slot, m) in self.players[i].board.iter().enumerate() {
                if m.active() && behaviour_of(m.card).and_then(|b| b.trigger).is_some() {
                    reactors.push((side, slot as u8, m.card));
                }
            }
        }
        // Weapons react too: "after your hero attacks" is printed on more
        // weapons than minions.
        for i in 0..2 {
            if let Some(w) = self.players[i].weapon
                && behaviour_of(w.card).and_then(|b| b.trigger).is_some()
            {
                reactors.push((Side::from_index(i), WEAPON_SLOT, w.card));
            }
        }

        // No early return here: secrets are a separate zone and must get their
        // chance even when nothing on either board reacts.
        self.trigger_depth += 1;
        for (side, slot, card) in reactors.iter().copied() {
            // Still there, still the same card, still alive?
            let still_there = if slot == WEAPON_SLOT {
                self.player(side).weapon.is_some_and(|w| w.card == card)
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
