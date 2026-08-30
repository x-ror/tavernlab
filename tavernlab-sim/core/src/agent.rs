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

use crate::cards::{Keywords, Kind, TargetSpec, behaviour_of};
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
    pub(crate) fn face_weight(self) -> f32 {
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
pub(crate) fn minion_value(m: &Permanent) -> f32 {
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

/// Whether the card could have been pointed at the other side instead.
///
/// A spec that only ever allows friendly targets is a card meant for its own
/// side, and asking whether it harms would answer "no" every time for the cost
/// of a game copy. These are the specs where both sides are legal, and so the
/// only ones where the choice of side can be wrong.
fn could_hit_an_enemy(spec: TargetSpec) -> bool {
    matches!(
        spec,
        TargetSpec::AnyCharacter
            | TargetSpec::AnyMinion
            | TargetSpec::DamagedMinion
            | TargetSpec::UndamagedMinion
            | TargetSpec::LegendaryMinion
            | TargetSpec::MinionAtkAtMost(_)
            | TargetSpec::MinionAtkAtLeast(_)
    )
}

/// Whether `a` is a spell aimed at its caster's own hero that hurts it.
///
/// Both halves matter. A card that could only ever target a friendly
/// character is one meant for its own side, and a minion's Battlecry pointed
/// at your own hero is ordinarily a heal or a buff; the case worth a game copy
/// is a spell that could have gone at the enemy instead.
fn self_harming_play(g: &Game, a: Action) -> bool {
    let Action::Play { hand, target, .. } = a else {
        return false;
    };
    let me = g.current;
    if target != Some(Target::Hero(me)) {
        return false;
    }
    let Some(hc) = g.player(me).hand.get(hand as usize) else {
        return false;
    };
    if hc.card.def().kind() != Kind::Spell {
        return false;
    }
    let Some(spec) = behaviour_of(hc.card).map(|b| b.target) else {
        return false;
    };
    could_hit_an_enemy(spec) && harms(g, a, Target::Hero(me))
}

/// Whether playing `a` leaves the thing it hit worse off than it is now.
///
/// The engine answers, on a copy: health for a hero, health plus attack for a
/// minion, and a minion that is no longer there at all has certainly been
/// harmed. `Game` is a couple of kilobytes and copies by memcpy -- the whole
/// reason the state is flat -- so this is one memcpy and one turn's worth of
/// effect resolution, and it is asked only where the side of the target could
/// be wrong.
fn harms(g: &Game, a: Action, t: Target) -> bool {
    let worth = |g: &Game| -> Option<i32> {
        match t {
            Target::Hero(s) => Some((g.player(s).hero_hp + g.player(s).armor) as i32),
            Target::Minion(s, i) => g
                .player(s)
                .board
                .get(i as usize)
                .map(|m| (m.health() + m.atk) as i32),
        }
    };
    let before = match worth(g) {
        Some(v) => v,
        None => return false,
    };
    let mut probe = *g;
    if !probe.apply(a) {
        return false;
    }
    // Gone is worse than anything a number could say.
    worth(&probe).is_none_or(|after| after < before)
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
                if d.kind() == Kind::Weapon {
                    // Equipping breaks whatever is already in the hand, so the
                    // question is not what the new weapon is worth but what
                    // the swap is worth. The policy did not ask, so a second
                    // copy of the weapon you are holding scored exactly what
                    // the first one did -- and a real plan read
                    // "play Corpse Cannon, play Corpse Cannon", throwing away
                    // three swings for nothing.
                    //
                    // A weapon is worth the damage it can still deal.
                    let now = g
                        .player(me)
                        .weapon
                        .as_ref()
                        .map_or(0, |w| w.atk.max(0) * w.durability.max(0));
                    let next = (d.atk + hc.atk as i16).max(0) * (d.dur + hc.hp as i16).max(0);
                    // A Battlecry can be the whole reason to equip, and it
                    // fires whatever the weapon is worth; without one, an
                    // equal-or-worse swap is a strict loss and belongs below
                    // ending the turn rather than merely behind other plays.
                    let battlecry = behaviour_of(hc.card).and_then(|b| b.battlecry).is_some();
                    // Only a swap can be a loss. With nothing equipped there
                    // is nothing to break, and `next <= now` would otherwise
                    // refuse a weapon with no Attack at all -- which some
                    // carry, because the effect is the whole card.
                    let swapping = g.player(me).weapon.is_some();
                    if swapping && next <= now && !battlecry {
                        return 0.5;
                    }
                    // And nothing else: an upgrade keeps the score it had.
                    // Weighting the size of the upgrade would reorder every
                    // weapon against every other card.
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
            Action::HeroPower { target, second } => {
                let mut s = 5.0;
                match target {
                    Some(Target::Minion(s_side, i)) => {
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
                    // Whose hero to aim at was the one target this never
                    // checked. "Any character" is the same list for Fireblast
                    // and Lesser Heal, so with both faces scoring alike the
                    // pick fell to enumeration order -- and a Mage with an
                    // empty enemy board burned its own face for one, every
                    // turn it had the mana spare.
                    Some(Target::Hero(t_side)) => {
                        let p = g.player(me);
                        let hp = if second { p.second_hero_power } else { Some(p.hero_power) };
                        let harms = hp.is_some_and(crate::game::hero_power_harms);
                        if harms == (t_side == me) {
                            s -= 30.0;
                        }
                    }
                    None => {}
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

            // Trading is what a Tradeable card is for when it is stuck: a
            // removal spell with nothing to point at cannot be cast at all,
            // and is worth exactly one mana and a fresh card. A card that
            // could simply be played is scored below every play, so the
            // option never crowds out the board — a minion's body still
            // comes down even when its Battlecry has no target.
            Action::Trade { hand } => {
                let Some(hc) = g.player(me).hand.get(hand as usize) else {
                    return f32::MIN;
                };
                let spec = behaviour_of(hc.card).map_or(TargetSpec::None, |b| b.target);
                let uncastable = hc.card.def().kind() == Kind::Spell
                    && spec.needed()
                    && !g.targetable(true).any(|t| spec.matches(g, me, t));
                if uncastable {
                    12.0
                } else if g.player(me).effective_cost(hc) > g.player(me).mana {
                    4.0
                } else {
                    2.0
                }
            }

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
        // Which side of a two-sided card to aim at is not in the corpus:
        // `AnyCharacter` is the spec on Fireball and on a heal alike. Rather
        // than guess the polarity from the printed text, play the action on
        // a copy and see what it did to the hero.
        //
        // Asked of the action that has already won rather than of every
        // action scored, because scoring runs for every legal action of every
        // step and a game copy there is expensive. Scored twice rather than
        // keeping a runner-up, which would cost a compare on every action to
        // save work on the rare step where the winner points the wrong
        // way.
        let mut skip: Option<Action> = None;
        for _ in 0..2 {
            let mut best = Action::EndTurn;
            let mut best_score = f32::MIN;
            for &a in legal {
                if skip == Some(a) {
                    continue;
                }
                let s = self.score(game, a);
                // Strictly greater keeps the choice deterministic: with equal
                // scores the earlier action wins, and enumeration order is
                // fixed.
                if s > best_score {
                    best_score = s;
                    best = a;
                }
            }
            if !self_harming_play(game, best) {
                return best;
            }
            // One retry: the second pick cannot be the same action, and a
            // second self-harming one is a position with nothing better.
            skip = Some(best);
        }
        Action::EndTurn
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

/// How good a whole position is for `side`, in rough stat-points.
///
/// The engine needs this for exactly one thing: Rewind, which asks the player
/// to look at what a card just did and decide whether to take it or roll
/// again. Somebody has to answer that, and the only honest way to say what
/// this program answers with is to write the answer down.
///
/// It is deliberately the same crude currency `minion_value` uses, and it is
/// as blind as `minion_value` is -- see the note there. Two known gaps, both
/// recorded in `APPROXIMATE` on the cards they reach:
///
///   * three of the eight Bonus Effects are invisible to `minion_value`, so a
///     Rewind that only changed which Bonus Effects landed can compare two
///     outcomes as equal when they are not;
///   * nothing here looks at the *deck*, so a Rewind that changed what is
///     left to draw is judged only by what is already on the table.
///
/// Both make the choice worse than a player's, never better, which is the
/// direction this whole engine errs in.
pub fn position_value(g: &Game, side: Side) -> f32 {
    // A finished game dominates everything else: no board is worth losing on.
    if let Some(o) = g.outcome {
        return (score_for(o, side) - 0.5) * 1000.0;
    }
    let me = g.player(side);
    let foe = g.player(side.other());
    let mut v = 0.0;
    for m in me.board.iter() {
        if m.is_minion() && m.active() {
            v += minion_value(m);
        }
    }
    for m in foe.board.iter() {
        if m.is_minion() && m.active() {
            v -= minion_value(m);
        }
    }
    // Health is worth less per point than a body is: a 4/4 on the board does
    // more than four points of face damage would have.
    v += (me.hero_hp + me.armor) as f32 * 0.5;
    v -= (foe.hero_hp + foe.armor) as f32 * 0.5;
    // A card in hand is worth about a small body, plus a quarter of what the
    // game itself prices it at -- so a Rewind that changed *which* card was
    // drawn is not a tie. Cost is the only measure of a card in hand this
    // engine has that the card data actually carries.
    for hc in me.hand.iter() {
        v += 1.0 + (hc.card.def().cost + hc.cost_delta).max(0) as f32 * 0.25;
    }
    if let Some(w) = me.weapon {
        v += (w.atk * w.durability) as f32 * 0.5;
    }
    // Theirs counts too: a card that arms both players (Stadium Announcer)
    // is a worse roll when it arms them better.
    if let Some(w) = foe.weapon {
        v -= (w.atk * w.durability) as f32 * 0.5;
    }
    v += me.crystals as f32 * 0.25;
    v
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
            second: false,
        };
        assert!(a.score(&g, own) < a.score(&g, Action::EndTurn));
    }

    #[test]
    fn a_harming_hero_power_never_points_at_your_own_face() {
        // Fireblast and Lesser Heal share one target list -- any character --
        // and the sides mean opposite things. With both faces scoring alike
        // the pick fell to enumeration order, and a Mage with an empty enemy
        // board shot itself for one every turn it had the mana spare.
        let g = game_with(&[], &[]);
        let a = Scripted::new(Style::Midrange);
        let at = |side| Action::HeroPower {
            target: Some(Target::Hero(side)),
            second: false,
        };
        assert!(
            a.score(&g, at(Side::Player1)) > a.score(&g, at(Side::Player0)),
            "Fireblast goes at them, not at you"
        );
        assert!(a.score(&g, at(Side::Player0)) < a.score(&g, Action::EndTurn));
    }

    #[test]
    fn a_healing_hero_power_never_points_at_theirs() {
        let mut g = game_with(&[], &[]);
        for i in 0..2 {
            g.players[i].hero_power =
                by_name("Lesser Heal").expect("the corpus has Lesser Heal");
        }
        let a = Scripted::new(Style::Midrange);
        let at = |side| Action::HeroPower {
            target: Some(Target::Hero(side)),
            second: false,
        };
        assert!(
            a.score(&g, at(Side::Player0)) > a.score(&g, at(Side::Player1)),
            "Lesser Heal goes at you, not at them"
        );
    }

    #[test]
    fn a_second_copy_of_the_weapon_you_are_wearing_is_not_played() {
        // From a real plan: "play Corpse Cannon, play Corpse Cannon". The
        // second equip breaks the first, and both are 1/3 -- three swings
        // thrown away for nothing.
        let mut g = game_with(&[], &[]);
        let cannon = by_name("Corpse Cannon").expect("the corpus has Corpse Cannon");
        g.players[0].hand.push(crate::state::HandCard::new(cannon));
        g.players[0].hand.push(crate::state::HandCard::new(cannon));
        let a = Scripted::new(Style::Midrange);
        let play = |hand| Action::Play {
            hand,
            target: None,
            position: u8::MAX,
            choice: u8::MAX,
        };
        assert!(
            a.score(&g, play(0)) > a.score(&g, Action::EndTurn),
            "with no weapon on, equipping is the right play"
        );

        g.players[0].weapon = Some(crate::state::Weapon::equip(cannon));
        assert!(
            a.score(&g, play(0)) < a.score(&g, Action::EndTurn),
            "with the same weapon already on, it is a strict loss"
        );
    }

    #[test]
    fn a_better_weapon_still_replaces_a_worse_one() {
        let mut g = game_with(&[], &[]);
        let big = by_name("Arcanite Reaper").expect("a 5/2");
        let small = by_name("Fiery War Axe").expect("a 3/2");
        g.players[0].hand.push(crate::state::HandCard::new(big));
        g.players[0].weapon = Some(crate::state::Weapon::equip(small));
        let a = Scripted::new(Style::Midrange);
        let play = Action::Play {
            hand: 0,
            target: None,
            position: u8::MAX,
            choice: u8::MAX,
        };
        assert!(
            a.score(&g, play) > a.score(&g, Action::EndTurn),
            "ten damage over six is an upgrade, and the swap is worth it"
        );
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
