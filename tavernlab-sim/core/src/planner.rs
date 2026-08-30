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
}

impl Weights {
    /// The baseline `tavernsim weights` states its sweep against.
    pub const GUESSED: Weights = Weights {
        own_health: 0.35,
        card: 0.5,
        unspent: 0.15,
    };

    /// What the search runs with.
    ///
    /// Only `card` differs from [`GUESSED`](Self::GUESSED): `own_health` and
    /// `unspent` are near-constant across the sibling lines of one turn, so
    /// they cancel rather than decide, and any value from 0 to 1 plays the
    /// same. `card` has a narrow peak around 2.0 and should be held loosely.
    pub const MEASURED: Weights = Weights {
        card: 2.0,
        ..Weights::GUESSED
    };
}

impl Default for Weights {
    fn default() -> Weights {
        Weights::MEASURED
    }
}

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
                Outcome::Win(w) if w == me => 1.0e6,
                Outcome::Draw => 0.0,
                _ => -1.0e6,
            };
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
        v
    }

    /// Best value reachable from `g` within `depth` more actions, or `None`
    /// if the budget ran out before the level was finished.
    ///
    /// Ending the turn is always one of the options, so a line that has run
    /// out of useful moves scores where it stands rather than being forced to
    /// keep playing.
    ///
    /// A truncated level is not a worse answer but a different question: the
    /// actions it reached were scored against each other while the rest were
    /// not scored at all. The caller discards it and keeps the last complete
    /// one.
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
                return None;
            }
            *budget -= 1;
            let mut c = *g;
            if !c.apply(a) {
                continue;
            }
            let s = self.search(&c, me, depth - 1, budget)?;
            if s > best {
                best = s;
            }
        }
        Some(best)
    }

    /// Score every root action at exactly `depth`, or give up on the level.
    ///
    /// `out` is only written when the whole level finished, so a caller can
    /// keep the previous one intact.
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
                Some(v) => scored[i] = v,
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
