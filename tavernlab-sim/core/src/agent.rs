//! A scripted policy.
//!
//! This is not meant to play well; it is meant to play *consistently*, so that
//! two decks compared against it differ because of the decks. It scores every
//! legal action and takes the best one, which keeps the policy in one function
//! instead of the ladder of special cases the Python agent grew.
//!
//! Scoring every action rather than following a fixed play order also means the
//! policy sees exactly the moves a search would see, so replacing it later with
//! something that looks ahead does not change what "legal" means.

use crate::cards::{Keywords, Kind};
use crate::game::{Action, Agent};
use crate::state::{Game, Permanent, Side, Target};

/// How the policy values face damage against board control.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Style {
    /// Race: push damage, trade only when forced.
    Aggro,
    /// Balance tempo against value.
    Midrange,
    /// Prioritise the board and the long game.
    Control,
}

impl Style {
    /// Weight applied to damage dealt to the enemy hero.
    fn face_weight(self) -> f32 {
        match self {
            Style::Aggro => 1.4,
            Style::Midrange => 0.9,
            Style::Control => 0.5,
        }
    }

    /// Mulligan cost ceiling.
    fn keep_threshold(self) -> i16 {
        match self {
            Style::Aggro => 2,
            Style::Midrange => 3,
            Style::Control => 3,
        }
    }
}

/// A greedy, deterministic policy.
#[derive(Clone, Copy, Debug)]
pub struct Scripted {
    pub style: Style,
}

impl Scripted {
    pub fn new(style: Style) -> Self {
        Self { style }
    }
}

/// How much a minion is worth having on the board.
///
/// Stats dominate; keywords add what they are roughly worth in stats. Kept
/// crude on purpose — a finely tuned heuristic here would be a hidden second
/// evaluation function competing with whatever search comes later.
fn minion_value(m: &Permanent) -> f32 {
    let mut v = m.atk as f32 + m.health() as f32;
    if m.has(Keywords::TAUNT) {
        v += 1.0;
    }
    if m.has(Keywords::DIVINE_SHIELD) {
        v += m.atk as f32 * 0.5 + 1.0;
    }
    if m.has(Keywords::LIFESTEAL) {
        v += 1.0;
    }
    if m.has(Keywords::POISONOUS) {
        v += 2.0;
    }
    if m.has(Keywords::WINDFURY) {
        v += m.atk as f32 * 0.5;
    }
    if m.has(Keywords::DEATHRATTLE) {
        v += 1.0;
    }
    if m.spell_damage > 0 {
        v += m.spell_damage as f32;
    }
    v
}

impl Scripted {
    fn score(&self, g: &Game, a: Action) -> f32 {
        let me = g.current;
        let foe = me.other();
        match a {
            // Spending mana is almost always right, and a bigger card is
            // usually the better use of a turn. Playing before attacking also
            // matters: a minion played first can be buffed by what follows.
            Action::Play { hand, .. } => {
                let Some(hc) = g.player(me).hand.get(hand as usize) else {
                    return f32::MIN;
                };
                let d = hc.card.def();
                let mut s = 40.0 + g.player(me).effective_cost(hc) as f32 * 2.0;
                if d.kind() == Kind::Minion {
                    s += (d.atk + d.hp) as f32 * 0.3;
                }
                s
            }

            Action::Attack { from, target } => {
                let Some(att) = g.player(me).board.get(from as usize) else {
                    return f32::MIN;
                };
                self.attack_score(g, att.atk, minion_value(att), att, target)
            }

            Action::HeroAttack { target } => {
                let atk = g.player(me).hero_attack();
                // The hero cannot die from a counterattack the way a minion
                // can, but the damage still costs real health.
                let hp_left = g.player(me).hero_hp + g.player(me).armor;
                let counter = match target {
                    Target::Hero(_) => 0,
                    Target::Minion(s, i) => g.player(s).board.get(i as usize).map_or(0, |m| m.atk),
                };
                if counter >= hp_left {
                    return f32::MIN; // never swing into lethal
                }
                let pseudo = Permanent::default();
                self.attack_score(g, atk, 0.0, &pseudo, target) - counter as f32 * 0.8
            }

            // Worth doing with leftover mana, never worth doing instead of
            // developing the board.
            Action::HeroPower { target } => {
                let mut s = 5.0;
                if let Some(Target::Minion(s_side, i)) = target {
                    // Only point damage at something it can finish.
                    if let Some(m) = g.player(s_side).board.get(i as usize) {
                        if s_side == foe && m.health() <= 1 {
                            s += 20.0;
                        } else if s_side == foe {
                            s += 2.0;
                        } else {
                            s -= 30.0; // do not shoot your own board
                        }
                    }
                }
                s
            }

            // A Location's ability is free, so using it is nearly always
            // right; the only question is whether the target is sensible,
            // which the target enumeration has already settled.
            Action::UseLocation { .. } => 30.0,

            // Banking mana into a card you cannot cast is better than
            // wasting it, but never better than actually developing.
            Action::Prepare { .. } => 3.0,

            Action::EndTurn => 1.0,
        }
    }

    /// Shared by minion and hero attacks.
    fn attack_score(
        &self,
        g: &Game,
        atk: i16,
        attacker_value: f32,
        attacker: &Permanent,
        target: Target,
    ) -> f32 {
        let me = g.current;
        let foe = me.other();
        match target {
            Target::Hero(_) => {
                let enemy_hp = g.player(foe).hero_hp + g.player(foe).armor;
                if atk >= enemy_hp {
                    return 10_000.0; // lethal
                }
                20.0 + atk as f32 * self.style.face_weight()
            }
            Target::Minion(s, i) => {
                let Some(def) = g.player(s).board.get(i as usize) else {
                    return f32::MIN;
                };
                let kills = atk >= def.health() && !def.has(Keywords::DIVINE_SHIELD)
                    || attacker.has(Keywords::POISONOUS);
                let dies = def.atk >= attacker.health() && attacker_value > 0.0;
                let mut s = 15.0;
                if kills {
                    s += minion_value(def) * 1.5;
                }
                if dies {
                    s -= attacker_value;
                }
                // A trade that kills nothing and loses the attacker is worse
                // than doing nothing at all.
                if !kills && dies {
                    s -= 20.0;
                }
                s
            }
        }
    }
}

impl Agent for Scripted {
    fn choose(&mut self, game: &Game, legal: &[Action]) -> Action {
        let mut best = Action::EndTurn;
        let mut best_score = f32::MIN;
        for &a in legal {
            let s = self.score(game, a);
            // Strictly greater keeps the choice deterministic: with equal
            // scores the earlier action wins, and enumeration order is fixed.
            if s > best_score {
                best_score = s;
                best = a;
            }
        }
        best
    }

    fn mulligan(&mut self, _game: &Game, drawn: &[crate::cards::CardId], _aggressive: bool) -> u32 {
        let t = self.style.keep_threshold();
        let mut keep = 0;
        for (i, c) in drawn.iter().enumerate() {
            if c.def().cost <= t {
                keep |= 1 << i;
            }
        }
        keep
    }
}

/// The side that won, as a score for player 0: 1.0 win, 0.5 draw, 0.0 loss.
pub fn score_for(outcome: crate::state::Outcome, side: Side) -> f32 {
    match outcome {
        crate::state::Outcome::Draw => 0.5,
        crate::state::Outcome::Win(w) if w == side => 1.0,
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cards::by_name;
    use crate::state::{Flags, Outcome};

    fn game_with(board_me: &[&str], board_foe: &[&str]) -> Game {
        let mut g = Game::new(
            (crate::cards::Class::Mage, &[]),
            (crate::cards::Class::Mage, &[]),
            1,
        )
        .unwrap();
        for n in board_me {
            let mut m = Permanent::summon(by_name(n).unwrap());
            m.flags.remove(Flags::JUST_SUMMONED);
            g.players[0].board.push(m);
        }
        for n in board_foe {
            let mut m = Permanent::summon(by_name(n).unwrap());
            m.flags.remove(Flags::JUST_SUMMONED);
            g.players[1].board.push(m);
        }
        g
    }

    #[test]
    fn lethal_beats_everything_else() {
        let mut g = game_with(&["Wolfrider"], &[]); // 3/1 Charge
        g.players[1].hero_hp = 3;
        let mut legal = crate::inline::Inline::new();
        g.legal_actions(&mut legal);
        let mut a = Scripted::new(Style::Control);
        let pick = a.choose(&g, legal.as_slice());
        assert_eq!(
            pick,
            Action::Attack {
                from: 0,
                target: Target::Hero(Side::Player1)
            }
        );
    }

    #[test]
    fn a_pointless_suicide_trade_is_rejected() {
        // 3/1 into a 5/5: kills nothing, loses the attacker. Ending the turn
        // has to score higher.
        let g = game_with(&["Wolfrider"], &["Boulderfist Ogre"]);
        let a = Scripted::new(Style::Control);
        let attack = Action::Attack {
            from: 0,
            target: Target::Minion(Side::Player1, 0),
        };
        assert!(a.score(&g, attack) < a.score(&g, Action::EndTurn));
    }

    #[test]
    fn a_favourable_trade_is_taken() {
        // 3/1 Charge into a 3/1: both die, but the exchange is even in stats
        // and removes a threat, so it beats passing.
        let g = game_with(&["Wolfrider"], &["Wolfrider"]);
        let a = Scripted::new(Style::Control);
        let attack = Action::Attack {
            from: 0,
            target: Target::Minion(Side::Player1, 0),
        };
        assert!(a.score(&g, attack) > a.score(&g, Action::EndTurn));
    }

    #[test]
    fn aggro_values_face_more_than_control_does() {
        let g = game_with(&["Wolfrider"], &[]);
        let face = Action::Attack {
            from: 0,
            target: Target::Hero(Side::Player1),
        };
        let aggro = Scripted::new(Style::Aggro).score(&g, face);
        let control = Scripted::new(Style::Control).score(&g, face);
        assert!(aggro > control);
    }

    #[test]
    fn the_hero_never_swings_into_its_own_death() {
        let mut g = game_with(&[], &["Boulderfist Ogre"]); // 6/7
        g.players[0].hero_hp = 5;
        g.players[0].weapon = Some(crate::state::Weapon::equip(
            by_name("Fiery War Axe").unwrap(),
        ));
        let mut a = Scripted::new(Style::Aggro);
        let swing = Action::HeroAttack {
            target: Target::Minion(Side::Player1, 0),
        };
        assert_eq!(a.score(&g, swing), f32::MIN);
        let mut legal = crate::inline::Inline::new();
        g.legal_actions(&mut legal);
        assert_ne!(a.choose(&g, legal.as_slice()), swing);
    }

    #[test]
    fn hero_power_never_targets_your_own_board() {
        let g = game_with(&["Bloodfen Raptor"], &[]);
        let a = Scripted::new(Style::Midrange);
        let own = Action::HeroPower {
            target: Some(Target::Minion(Side::Player0, 0)),
        };
        assert!(a.score(&g, own) < a.score(&g, Action::EndTurn));
    }

    #[test]
    fn choose_is_deterministic() {
        let g = game_with(&["Wolfrider", "Bloodfen Raptor"], &["Goldshire Footman"]);
        let mut legal = crate::inline::Inline::new();
        g.legal_actions(&mut legal);
        let mut a = Scripted::new(Style::Midrange);
        let first = a.choose(&g, legal.as_slice());
        for _ in 0..10 {
            assert_eq!(a.choose(&g, legal.as_slice()), first);
        }
    }

    #[test]
    fn score_for_reads_the_outcome_from_each_side() {
        assert_eq!(score_for(Outcome::Win(Side::Player0), Side::Player0), 1.0);
        assert_eq!(score_for(Outcome::Win(Side::Player0), Side::Player1), 0.0);
        assert_eq!(score_for(Outcome::Draw, Side::Player0), 0.5);
    }
}
