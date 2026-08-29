//! A policy that plans the rest of its turn before it acts.
//!
//! [`Scripted`](crate::agent::Scripted) scores one action at a time, which
//! cannot see that a buff wants a body under it, that a trade wants to happen
//! before the buff, or that the Hero Power is worth more after the board is
//! settled than before. This searches the sequences instead: it plays the
//! turn out on copies of the game and keeps the first action of the line that
//! ends best.
//!
//! It exists to answer one question -- how much is the greedy policy leaving
//! on the table? -- and the answer is a win rate against it on the same decks
//! and the same seeds. It is +37.5 points, and this is still a measuring
//! instrument rather than the engine's policy: two hundred times slower is a
//! product decision, not a code one. See the README.
//!
//! ## What it is not allowed to know
//!
//! Both policies are handed the whole `Game`, the opponent's hand and both
//! deck orders included. Greedy never looks; a search would, because a line
//! that plays Novice Engineer resolves a real draw off a real deck and can
//! prefer the line whose card happens to be good. That is not play skill, it
//! is peeking.
//!
//! The same goes for every other roll. A line that casts a Discover resolves
//! it off the real effects stream, so the search learns exactly which three
//! cards will be offered -- and would happily pick the line because of it.
//!
//! So the search runs on a copy with both streams replaced: the deck
//! reshuffled, and the effects generator reseeded. The line it picks is then
//! the line that is best against *a* deck order and *a* set of rolls rather
//! than *the* ones, which is what a player choosing without knowing has. The
//! opponent's hand is never read by the evaluation at all.
//!
//! What that costs is real and worth stating: the search chooses under a
//! single sample of the randomness rather than under its distribution.
//! Averaging over several samples looks like the fix and measures as nothing
//! at all -- one, four and eight samples all read the same win rate for eight
//! times the work, because within a single turn almost no randomness is
//! resolved. `samples` is still a knob so that claim stays re-runnable.

use crate::agent::{Scripted, Style, minion_value};
use crate::game::{Action, Agent, MAX_ACTIONS};
use crate::inline::Inline;
use crate::state::{Game, Outcome, Side};

/// The numbers the evaluation weighs a position by, against one point of
/// board -- `minion_value` is the unit, so it has no weight of its own.
///
/// These were invented. Three of them were picked by hand when the search
/// was written, to see whether searching helped at all, and the answer to
/// that (+37.5 points) says nothing about whether they are the right three
/// numbers. They are a parameter so that they can be measured against each
/// other in the same head-to-head the rest of the search was measured in --
/// see the README, and `tavernsim weights`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Weights {
    /// A point of my own hero's health or armour.
    ///
    /// The enemy hero's is weighted by `Style::face_weight` instead, which
    /// is what makes an aggro planner race and a control one trade -- that
    /// one is the archetype knob and is not swept here.
    pub own_health: f32,
    /// A card still in hand. Worth something, but less than a body on the
    /// board, or the search would rather hold than play.
    pub card: f32,
    /// Charged per mana crystal left unspent when the turn ends. Mana that
    /// ends the turn unspent bought nothing.
    pub unspent: f32,
}

impl Weights {
    /// What the search has been running with. Not a measured optimum -- the
    /// name says where they came from, and the sweep says what happened when
    /// they were questioned.
    pub const GUESSED: Weights = Weights {
        own_health: 0.35,
        card: 0.5,
        unspent: 0.15,
    };
}

impl Default for Weights {
    fn default() -> Weights {
        Weights::GUESSED
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
    /// One sample means choosing under *a* shuffle rather than under the
    /// distribution of shuffles, which is honest about what the player knows
    /// but noisy about what the line is worth. More samples cost budget that
    /// could have gone into depth instead; which trade wins is measured, not
    /// assumed -- see the README.
    pub samples: u8,
    /// Deepen a level at a time, completing each before starting the next,
    /// rather than running one depth-first pass until the budget is gone.
    ///
    /// Kept as a knob rather than hard-wired because the difference between
    /// the two is the whole reason this was rewritten, and a claim about it
    /// should stay reproducible from the code that makes it.
    pub iterative: bool,
    /// What the evaluation weighs a position by.
    pub weights: Weights,
    /// Falls back to greedy for the mulligan, which is a different question
    /// and not one a within-turn search can answer.
    greedy: Scripted,
    /// Bumped once per decision so that each search reseeds the effects
    /// stream differently. Carried on the agent rather than read off a clock,
    /// so a run stays reproducible from its seed.
    nonce: u64,
}

impl Planner {
    /// The settings the measurement found best; see the README table.
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
    /// Only public information: both boards, both hero totals, the size of
    /// my own hand. Never the opponent's hand, and never either deck.
    ///
    /// The numbers are deliberately the greedy policy's own -- `minion_value`
    /// for bodies and `Style::face_weight` for the race -- so that a win here
    /// is the search beating greedy at *sequencing* rather than at having
    /// been given a better idea of what a board is worth. Two evaluators
    /// would make the comparison meaningless.
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
        // Health is scored as a level rather than as damage dealt, because a
        // starting total is not always thirty -- Azalina sets forty, a Hero
        // card brings its own -- and sibling lines are only ever compared
        // against each other, where a shared offset cancels.
        let face = self.style.face_weight();
        v -= face * (g.player(foe).hero_hp + g.player(foe).armor) as f32;
        v += self.weights.own_health * (g.player(me).hero_hp + g.player(me).armor) as f32;
        v += self.weights.card * g.player(me).hand.len() as f32;
        v -= self.weights.unspent * g.player(me).mana as f32;
        v
    }

    /// Best value reachable from `g` within `depth` more actions.
    ///
    /// Ending the turn is always one of the options, so a line that has run
    /// out of useful moves scores where it stands rather than being forced to
    /// keep playing.
    /// Best value reachable from `g` within `depth` more actions, or `None`
    /// if the budget ran out before the level was finished.
    ///
    /// The `None` matters: a truncated level is not a worse answer, it is a
    /// different question -- the actions it did reach were scored against
    /// each other while the rest were not scored at all, so whichever
    /// happened to be enumerated first wins by default. The caller throws
    /// such a level away and keeps the last one that finished.
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

        // Mean score per root action, summed across determinizations. Summed
        // rather than averaged because every action is seen by every sample,
        // so the divisor is the same for all of them and cannot change the
        // ranking.
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
                // Re-searching the shallow levels costs a constant factor;
                // spending the whole budget on the first branch costs the
                // comparison itself.
                for depth in 1..=self.depth {
                    let mut next = [0.0f32; MAX_ACTIONS];
                    if !self.level(&root, me, legal, depth, &mut budget, &mut next) {
                        break;
                    }
                    scored = next;
                    have = true;
                }
            } else {
                // The original single depth-first pass, kept so the claim
                // that deepening is better stays checkable.
                have = self.level(&root, me, legal, self.depth, &mut budget, &mut scored);
                if !have {
                    // Depth-first has no completed level to fall back on, so
                    // it keeps the truncated one -- which is exactly the
                    // behaviour being compared against.
                    have = true;
                }
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
