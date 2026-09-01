//! A policy that plans the rest of its turn before it acts.
//!
//! [`Scripted`](crate::agent::Scripted) scores one action at a time, which
//! cannot see that a buff wants a body under it, that a trade wants to happen
//! before the buff, or that the Hero Power is worth more after the board is
//! settled than before. This searches the sequences instead: it plays the
//! turn out on copies of the game and keeps the first action of the line that
//! ends best.
//!
//! Not the engine's policy: it costs about two hundred times a greedy
//! decision, which is affordable for one live decision and not for a batch.
//! `tavernsim policy` compares the two.
//!
//! ## What the search may not read
//!
//! Both policies are handed the whole [`Game`], the opponent's hand and both
//! deck orders included. A line that plays a card which draws resolves that
//! draw off the real deck, and a line that Discovers resolves it off the real
//! effects stream -- so a search reading them would choose a line for a card
//! it has no business knowing.
//!
//! Every line therefore runs on a copy with both streams replaced: the
//! searching player's deck reshuffled and the effects generator reseeded. The
//! line it picks is the best against *a* deck order and *a* set of rolls
//! rather than *the* ones. The opponent's hand is never read by the
//! evaluation at all.
//!
//! The cost of that is choosing under a single sample of the randomness
//! rather than under its distribution. [`Planner::samples`] averages over
//! several instead.

use crate::agent::{Scripted, Style, minion_value};
use crate::game::{Action, Agent, MAX_ACTIONS};
use crate::inline::Inline;
use crate::state::{Game, Outcome, Side};

/// The numbers the evaluation weighs a position by, against one point of
/// board -- `minion_value` is the unit, so it has no weight of its own.
///
/// A parameter rather than constants so that `tavernsim weights` can play
/// one value against another.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Weights {
    /// A point of my own hero's health or armour. The enemy hero's is
    /// weighted by [`Style::face_weight`] instead, which is the archetype
    /// knob and not part of this struct.
    pub own_health: f32,
    /// A card still in hand. Worth something, but less than a body on the
    /// board, or the search would rather hold than play.
    pub card: f32,
    /// Charged per mana crystal left unspent when the turn ends. Mana that
    /// ends the turn unspent bought nothing.
    pub unspent: f32,
    /// A secret set in your own zone.
    ///
    /// Priced rather than played out, because a secret pays off past this
    /// search's horizon: it fires on the opponent's turn, and the search
    /// stops at the end of yours. Without a price, setting one only ever
    /// moves a card out of hand -- see [`card`](Self::card) -- so every line
    /// that sets a secret scores below the line that does not, and a secret
    /// deck is advised to hold them all game.
    pub secret: f32,
}

impl Weights {
    /// The baseline `tavernsim weights` states its sweep against.
    pub const GUESSED: Weights = Weights {
        own_health: 0.35,
        card: 0.5,
        unspent: 0.15,
        secret: 0.0,
    };

    /// What the search runs with.
    ///
    /// Only `card` differs from [`GUESSED`](Self::GUESSED): `own_health` and
    /// `unspent` are near-constant across the sibling lines of one turn, so
    /// they cancel rather than decide, and any value from 0 to 1 plays the
    /// same. `card` has a narrow peak around 2.0 and should be held loosely.
    ///
    /// `secret` prices what the search cannot reach. It is held as loosely
    /// as `card`: every value from 4 to 12 played the same, and 4 is the
    /// smallest that bought the whole difference, so it is the one that
    /// distorts the rest of the evaluation least. Below 2 it bought nothing
    /// at all -- the weight was too small to flip a single decision, and the
    /// games came out identical.
    pub const MEASURED: Weights = Weights {
        card: 2.0,
        secret: 4.0,
        ..Weights::GUESSED
    };
}

impl Default for Weights {
    fn default() -> Weights {
        Weights::MEASURED
    }
}

/// The score a proven win is worth. Effectively infinite against the
/// heuristic below it, so a line that reaches this beats every line that
/// does not, however good the board looks on paper -- `search` and `level`
/// both short-circuit the instant they see it, rather than spending any more
/// budget on siblings that cannot possibly outscore it.
const WIN: f32 = 1.0e6;

/// A turn-planning policy.
#[derive(Clone, Copy, Debug)]
pub struct Planner {
    pub style: Style,
    /// How many positions one decision may look at. The search re-plans at
    /// every step, so this is the width of one decision and not of a turn.
    pub budget: u32,
    /// How many actions deep a line may go before it is evaluated where it
    /// stands. A turn with more real decisions than this still gets searched;
    /// it just gets searched again from one action further along.
    pub depth: u8,
    /// How many determinizations to average a decision over.
    ///
    /// One sample chooses under *a* shuffle rather than under the
    /// distribution of shuffles. More samples cost budget that could have
    /// gone into depth instead.
    pub samples: u8,
    /// Deepen a level at a time, completing each before starting the next,
    /// rather than running one depth-first pass until the budget is gone.
    pub iterative: bool,
    /// What the evaluation weighs a position by.
    pub weights: Weights,
    /// Falls back to greedy for the mulligan, which is a different question
    /// and not one a within-turn search can answer.
    greedy: Scripted,
    /// Bumped once per decision so that each search reseeds the effects
    /// stream differently. Carried on the agent rather than read off a clock,
    /// so a run is reproducible from its seed.
    nonce: u64,
}

impl Planner {
    /// A planner with the default weights and one determinization.
    pub fn new(style: Style, budget: u32, depth: u8) -> Self {
        Self {
            style,
            budget,
            depth,
            samples: 1,
            iterative: true,
            weights: Weights::default(),
            greedy: Scripted::new(style),
            nonce: 0,
        }
    }

    /// Every knob spelled out, for the runs that compare them.
    pub fn tuned(
        style: Style,
        budget: u32,
        depth: u8,
        samples: u8,
        iterative: bool,
        weights: Weights,
    ) -> Self {
        Self {
            samples: samples.max(1),
            iterative,
            weights,
            ..Self::new(style, budget, depth)
        }
    }

    /// The copy a line is played on: my deck reshuffled, the effects stream
    /// reseeded. See the module note on why both.
    fn determinize(&self, game: &Game, me: Side, sample: u8) -> Game {
        let mut root = *game;
        root.rngs.effects = crate::rng::Rand::new(
            self.nonce
                ^ ((game.turn as u64) << 32)
                ^ ((me.index() as u64) << 16)
                ^ ((sample as u64) << 8),
        );
        root.shuffle_deck(me);
        root
    }

    /// A lower bound only: whether the damage already legally available on
    /// `me`'s side of `g` -- no further search needed -- would kill the
    /// opponent outright.
    ///
    /// A leaf that recognises this scores as a proven win without spending
    /// any depth walking through the actual swings, which is exactly the
    /// case a shallow search otherwise misses: Windfury's second hit, a
    /// ready weapon and a wide board can add up to lethal well before
    /// `search` would ever simulate every attack down to `g.is_over()`.
    ///
    /// Never a false positive. A live Taunt has to be cleared first, which is
    /// a search question and not a static one. And an unrevealed secret --
    /// Ice Block above all -- is a fact this engine's log/memory-reconstructed
    /// `Game` cannot know (CLAUDE.md: the watcher never guesses), so *any*
    /// secret in the zone silences this rather than risking advice the real
    /// client can still falsify.
    fn static_lethal(&self, g: &Game, me: Side) -> bool {
        if g.current != me {
            return false;
        }
        let victim = g.player(me.other());
        if victim.has_taunt() || !victim.secrets.is_empty() {
            return false;
        }
        let mut dmg: i32 = 0;
        for m in g.player(me).board.iter().filter(|m| m.can_attack_face()) {
            dmg += m.atk as i32 * (m.max_attacks() - m.attacks_done) as i32;
        }
        if g.player(me).hero_can_attack() {
            dmg += g.player(me).hero_attack() as i32;
        }
        // Hero Power damage (Fireblast and its kin) has no existing "does
        // this deal N face damage for free" accessor to key off, so it stays
        // out of the bound rather than growing per-card infrastructure for
        // it -- an occasionally too conservative bound is fine; a wrong one
        // is advice a player has no way to see through.
        dmg >= (victim.hero_hp + victim.armor) as i32
    }

    /// What a finished turn is worth to `me`.
    ///
    /// Reads only public information: both boards, both hero totals, and the
    /// size of my own hand. Never the opponent's hand, and never either deck.
    ///
    /// Shares `minion_value` and [`Style::face_weight`] with the greedy
    /// policy, so that the two differ in how they search rather than in what
    /// they think a board is worth.
    fn eval(&self, g: &Game, me: Side) -> f32 {
        if let Some(o) = g.outcome {
            return match o {
                Outcome::Win(w) if w == me => WIN,
                Outcome::Draw => 0.0,
                _ => -WIN,
            };
        }
        if self.static_lethal(g, me) {
            return WIN;
        }
        let foe = me.other();
        let mut v = 0.0;
        for m in g.player(me).board.iter().filter(|m| m.active()) {
            v += minion_value(m);
        }
        for m in g.player(foe).board.iter().filter(|m| m.active()) {
            v -= minion_value(m);
        }
        // A level rather than damage dealt: a starting total is not always
        // thirty, and sibling lines are only compared against each other,
        // where a shared offset cancels.
        let face = self.style.face_weight();
        v -= face * (g.player(foe).hero_hp + g.player(foe).armor) as f32;
        v += self.weights.own_health * (g.player(me).hero_hp + g.player(me).armor) as f32;
        v += self.weights.card * g.player(me).hand.len() as f32;
        v -= self.weights.unspent * g.player(me).mana as f32;
        v += self.weights.secret * g.player(me).secrets.len() as f32;
        v
    }

    /// Best value reachable from `g` within `depth` more actions, or `None`
    /// if the budget ran out before the level was finished and no line
    /// reached a proven win along the way.
    ///
    /// Ending the turn is always one of the options, so a line that has run
    /// out of useful moves scores where it stands rather than being forced to
    /// keep playing.
    ///
    /// A truncated level is not a worse answer but a different question: the
    /// actions it reached were scored against each other while the rest were
    /// not scored at all. The caller discards it and keeps the last complete
    /// one -- unless `best` is already a proven win, which running the shared
    /// budget dry on the *remaining* siblings cannot retroactively un-prove.
    fn search(&self, g: &Game, me: Side, depth: u8, budget: &mut u32) -> Option<f32> {
        if depth == 0 || g.is_over() || g.current != me {
            return Some(self.eval(g, me));
        }
        let mut legal: Inline<Action, MAX_ACTIONS> = Inline::new();
        g.legal_actions(&mut legal);
        // The floor: stop here. Every other line has to beat standing still.
        let mut best = self.eval(g, me);
        for &a in legal.as_slice() {
            if a == Action::EndTurn {
                continue;
            }
            if *budget == 0 {
                return if best >= WIN { Some(best) } else { None };
            }
            *budget -= 1;
            let mut c = *g;
            if !c.apply(a) {
                continue;
            }
            match self.search(&c, me, depth - 1, budget) {
                Some(s) => {
                    if s > best {
                        best = s;
                    }
                    // Nothing left in `legal` can beat a proven win, so
                    // there is no reason to keep spending budget on it.
                    if best >= WIN {
                        return Some(best);
                    }
                }
                None => return if best >= WIN { Some(best) } else { None },
            }
        }
        Some(best)
    }

    /// Score every root action at exactly `depth`, or give up on the level.
    ///
    /// `out` is only written when the whole level finished -- or when one
    /// root action already proved a win, in which case nothing left in
    /// `legal` could possibly outscore it, so the rest are never even
    /// tried and cannot spend the shared budget down to the point where
    /// that win would otherwise be discarded along with them. Otherwise a
    /// caller can keep the previous level intact.
    fn level(
        &self,
        root: &Game,
        me: Side,
        legal: &[Action],
        depth: u8,
        budget: &mut u32,
        out: &mut [f32],
    ) -> bool {
        let stand_still = self.eval(root, me);
        let mut scored = [0.0f32; MAX_ACTIONS];
        for (i, &a) in legal.iter().enumerate() {
            if a == Action::EndTurn {
                scored[i] = stand_still;
                continue;
            }
            if *budget == 0 {
                return false;
            }
            *budget -= 1;
            let mut c = *root;
            if !c.apply(a) {
                // An action the copy refuses is one this line cannot use.
                scored[i] = f32::MIN;
                continue;
            }
            match self.search(&c, me, depth - 1, budget) {
                Some(v) => {
                    scored[i] = v;
                    if v >= WIN {
                        out[..legal.len()].copy_from_slice(&scored[..legal.len()]);
                        return true;
                    }
                }
                None => return false,
            }
        }
        out[..legal.len()].copy_from_slice(&scored[..legal.len()]);
        true
    }
}

impl Agent for Planner {
    fn choose(&mut self, game: &Game, legal: &[Action]) -> Action {
        let me = game.current;
        if legal.len() <= 1 {
            return legal.first().copied().unwrap_or(Action::EndTurn);
        }
        self.nonce = self.nonce.wrapping_add(1);

        // Score per root action, summed across determinizations. Every
        // action is seen by every sample, so the divisor is shared and
        // averaging would not change the ranking.
        let mut totals = [0.0f32; MAX_ACTIONS];
        let mut any = false;
        let per_sample = (self.budget / self.samples as u32).max(1);

        for sample in 0..self.samples {
            let root = self.determinize(game, me, sample);
            let mut budget = per_sample;
            let mut scored = [0.0f32; MAX_ACTIONS];
            let mut have = false;

            if self.iterative {
                // A level at a time, keeping the last one that finished.
                // Re-searching the shallow levels costs a constant factor.
                for depth in 1..=self.depth {
                    let mut next = [0.0f32; MAX_ACTIONS];
                    if !self.level(&root, me, legal, depth, &mut budget, &mut next) {
                        break;
                    }
                    scored = next;
                    have = true;
                }
            } else {
                // One depth-first pass. With no completed level to fall
                // back on it keeps the truncated one.
                self.level(&root, me, legal, self.depth, &mut budget, &mut scored);
                have = true;
            }
            if have {
                any = true;
                for (t, s) in totals.iter_mut().zip(scored.iter()).take(legal.len()) {
                    *t += *s;
                }
            }
        }

        if !any {
            return Action::EndTurn;
        }
        // Strictly greater, so ties fall to enumeration order and the policy
        // stays deterministic -- the same rule greedy follows.
        let mut best = Action::EndTurn;
        let mut best_score = f32::MIN;
        for (i, &a) in legal.iter().enumerate() {
            if totals[i] > best_score {
                best_score = totals[i];
                best = a;
            }
        }
        best
    }

    fn mulligan(&mut self, game: &Game, drawn: &[crate::cards::CardId], aggressive: bool) -> u32 {
        self.greedy.mulligan(game, drawn, aggressive)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cards::{Class, Keywords, by_name};
    use crate::state::{Flags, Permanent, Target, Weapon};

    /// An empty game, current == `Player0`, ready for a test to place
    /// minions directly onto either board.
    fn fixture() -> Game {
        Game::new((Class::Mage, &[]), (Class::Mage, &[]), 1).unwrap()
    }

    /// A battle-ready minion (summoning sickness already gone) with the
    /// given stats, built off a real card so it carries a valid `CardId`.
    fn minion(atk: i16, hp: i16) -> Permanent {
        let mut m = Permanent::summon(by_name("Bloodfen Raptor").unwrap());
        m.flags.remove(Flags::JUST_SUMMONED);
        m.atk = atk;
        m.max_hp = hp;
        m
    }

    /// Seven 1/1s on my board, the opponent's hero at exactly that much
    /// health and nothing else in the way -- the shape tests 1, 4 and 5 all
    /// start from.
    fn seven_ones_at_lethal() -> Game {
        let mut g = fixture();
        for _ in 0..7 {
            g.players[0].board.push(minion(1, 1));
        }
        g.players[1].hero_hp = 7;
        g.recompute_auras();
        g
    }

    // ---------------------------------------------------- Part A: fail-soft

    #[test]
    fn a_proven_win_survives_a_later_sibling_exhausting_the_budget() {
        let mut g = fixture();
        for _ in 0..3 {
            g.players[0].board.push(minion(1, 1));
        }
        g.players[1].board.push(minion(4, 1)); // an on-paper-attractive trade
        g.players[1].hero_hp = 1;
        // A secret keeps `static_lethal` (Part B) silent everywhere in this
        // tree, so the only way any action here scores >= WIN is by really
        // simulating the kill down to `g.outcome` -- which is what this test
        // means to exercise, in isolation from Part B's own shortcut.
        g.players[1].secrets.push(by_name("Counterspell").unwrap());
        g.recompute_auras();

        let mut legal: Inline<Action, MAX_ACTIONS> = Inline::new();
        g.legal_actions(&mut legal);
        // The trade is offered before the lethal face swing for the same
        // minion -- exactly the enumeration order the bug needs to bite.
        assert_eq!(
            legal.as_slice()[0],
            Action::Attack {
                from: 0,
                target: Target::Minion(Side::Player1, 0)
            },
            "test assumes the trade is tried before the winning swing"
        );
        assert_eq!(
            legal.as_slice()[1],
            Action::Attack {
                from: 0,
                target: Target::Hero(Side::Player1)
            },
            "test assumes this is the winning swing"
        );

        // At depth 1, `search` never recurses (depth - 1 == 0 stops it cold),
        // so every unit of `budget` here is spent by `level` itself, one per
        // action tried, in enumeration order. Budget 2 buys exactly the
        // trade (index 0, non-winning) and then the win (index 1); a
        // pre-fix `level` goes on to try index 2, finds `budget == 0`, and
        // discards everything scored so far -- the win included.
        let mut planner = Planner::new(Style::Midrange, 2, 3);
        let chosen = planner.choose(&g, legal.as_slice());

        assert_ne!(
            chosen,
            Action::EndTurn,
            "a proven win must not be discarded as unreachable"
        );
        let mut after = g;
        assert!(after.apply(chosen));
        assert_eq!(
            after.outcome,
            Some(Outcome::Win(Side::Player0)),
            "the chosen action must itself be part of the winning line"
        );
    }

    // The test above only ever calls `search` with `depth - 1 == 0`, so it
    // returns at the very first line ("depth == 0 ... return Some(eval)")
    // without ever reaching `search`'s *own* loop -- every budget unit it
    // spends is spent by `level`, and `search`'s two fail-soft sites
    // (`budget == 0` at the top of the loop, and a recursive call's `None`)
    // never run. These two go straight at `search`, deep enough that its
    // own loop is the one that has to preserve the win, so a regression
    // that reverts only `search`'s half of the fix -- leaving `level`'s
    // intact -- fails here even though it keeps passing the test above.

    #[test]
    fn search_keeps_its_own_stand_still_win_when_handed_no_budget() {
        // The floor at this node (`static_lethal`, Part B) is already a
        // proven win before `search` looks at a single sibling. Handing it
        // a budget of 0 means the first non-EndTurn action it considers
        // hits `if *budget == 0` immediately -- the pre-fix line there was
        // an unconditional `return None`, which would have discarded this
        // win without `level` ever being involved.
        let g = seven_ones_at_lethal();
        let planner = Planner::new(Style::Midrange, 100, 3);
        let mut budget = 0u32;
        assert_eq!(
            planner.search(&g, Side::Player0, 2, &mut budget),
            Some(WIN),
            "search's own budget == 0 check must not discard a win already \
             standing at this node"
        );
    }

    #[test]
    fn search_keeps_a_proven_win_past_a_child_that_honestly_returns_none() {
        // Root: `static_lethal` is already true standing still (seven
        // attackers are already enough face damage without moving one).
        // `push_attack_targets` offers the enemy minion before the face
        // target for the same attacker, so the first action `search` tries
        // spends attacker 0 on that trade -- taking its damage out of the
        // bound without taking anything off the opponent's face. At the
        // resulting child, `static_lethal` is honestly false (six 1-attack
        // minions against 7 remaining health), and that recursive call is
        // left with budget == 0, so it returns a perfectly correct `None`
        // -- not a win, just out of budget. The root must still report the
        // win it already had *before* trying that child, which pre-fix
        // `search` discarded by propagating the child's `None` straight
        // through `?`.
        let mut g = seven_ones_at_lethal();
        g.players[1].board.push(minion(5, 1)); // the trade tried first
        g.recompute_auras();

        let planner = Planner::new(Style::Midrange, 100, 3);
        let mut budget = 1u32;
        assert_eq!(
            planner.search(&g, Side::Player0, 2, &mut budget),
            Some(WIN),
            "a proven win standing at the root must survive a child that \
             genuinely ran out of budget without finding one itself"
        );
    }

    // ------------------------------------------------ Part B: static lethal

    #[test]
    fn static_lethal_prefers_face_over_an_attractive_trade() {
        let mut g = seven_ones_at_lethal();
        g.players[1].board.push(minion(5, 1)); // dies to one swing, tempting
        g.recompute_auras();

        let mut legal: Inline<Action, MAX_ACTIONS> = Inline::new();
        g.legal_actions(&mut legal);
        let trade = Action::Attack {
            from: 0,
            target: Target::Minion(Side::Player1, 0),
        };
        assert_eq!(
            legal.as_slice()[0],
            trade,
            "test assumes the trade is offered first"
        );

        let mut planner = Planner::new(Style::Midrange, 200, 3);
        let chosen = planner.choose(&g, legal.as_slice());
        assert_ne!(chosen, trade, "must not spend the first action on the trade");
        assert_eq!(
            chosen,
            Action::Attack {
                from: 0,
                target: Target::Hero(Side::Player1)
            }
        );
    }

    #[test]
    fn static_lethal_counts_windfurys_second_swing() {
        let mut g = fixture();
        let mut m = minion(4, 4);
        m.keywords.insert(Keywords::WINDFURY);
        g.players[0].board.push(m);
        g.players[1].hero_hp = 8; // 4 * 2, not 4 * 1
        g.recompute_auras();

        let planner = Planner::new(Style::Midrange, 100, 3);
        assert!(planner.static_lethal(&g, Side::Player0));
    }

    #[test]
    fn static_lethal_counts_the_ready_weapon() {
        let mut g = fixture();
        g.players[0].board.push(minion(3, 3));
        g.players[1].hero_hp = 5;
        g.recompute_auras();

        let planner = Planner::new(Style::Midrange, 100, 3);
        assert!(
            !planner.static_lethal(&g, Side::Player0),
            "3 attack alone falls short of 5"
        );

        let mut weapon = Weapon::equip(by_name("Fiery War Axe").unwrap());
        weapon.atk = 2;
        g.players[0].weapon = Some(weapon);
        assert!(
            planner.static_lethal(&g, Side::Player0),
            "the weapon supplies exactly the rest"
        );
    }

    #[test]
    fn static_lethal_stays_silent_behind_a_secret() {
        let mut g = seven_ones_at_lethal();
        g.players[1].secrets.push(by_name("Counterspell").unwrap());

        let planner = Planner::new(Style::Midrange, 100, 3);
        assert!(
            !planner.static_lethal(&g, Side::Player0),
            "an unrevealed secret could still block the swing"
        );
    }

    #[test]
    fn static_lethal_stays_silent_behind_a_taunt() {
        let mut g = seven_ones_at_lethal();
        let mut taunt = minion(1, 1);
        taunt.keywords.insert(Keywords::TAUNT);
        g.players[1].board.push(taunt);
        g.recompute_auras();

        let planner = Planner::new(Style::Midrange, 100, 3);
        assert!(
            !planner.static_lethal(&g, Side::Player0),
            "the taunt has to be cleared first, which is a search question, not a static one"
        );
    }
}
