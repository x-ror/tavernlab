//! The rules.
//!
//! Everything a turn can do is an [`Action`], and [`Game::apply`] is the only
//! way state changes. That is not ceremony: it is what lets a search enumerate
//! moves later without a second, subtly different code path — the mistake that
//! makes most game AIs disagree with their own engine.
//!
//! Damage, healing and death removal all funnel through a small number of
//! functions here, because the interesting rules (Divine Shield, Immune,
//! Poisonous, Lifesteal, Armor) are exactly the ones that get forgotten when
//! damage is applied ad hoc at a dozen call sites.

use crate::cards::{CardId, Class, Ctx, Keywords, Kind, Races, TargetSpec, behaviour_of, by_name};
use crate::events::Event;
use crate::inline::Inline;
use crate::rng::Rand;
use crate::state::{
    Flags, Game, HandCard, MAX_BOARD, MAX_DECK, MAX_HAND, MAX_MANA, Marks, Outcome, Pending,
    PendingKind, Permanent, Player, Side, TURN_LIMIT, Target, Weapon,
};

/// The most actions a position can offer.
///
/// Today's worst case is around ninety: seven attackers against eight targets,
/// a hero swing, ten cards and a hero power. Targeted spells and choose-one
/// will multiply the card half by the target count, so the buffer is sized for
/// that rather than for the present.
pub const MAX_ACTIONS: usize = 512;

/// One decision.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Action {
    /// Play the card at `hand` index, optionally at a board `position`.
    Play {
        hand: u8,
        target: Option<Target>,
        position: u8,
        /// Which Choose One mode, or `u8::MAX` for a card that has none.
        choice: u8,
    },
    /// Attack with the minion in board slot `from`.
    Attack {
        from: u8,
        target: Target,
    },
    /// Attack with the hero (needs a weapon or an attack buff).
    HeroAttack {
        target: Target,
    },
    HeroPower {
        target: Option<Target>,
        /// Use the second Hero Power (Blood Doctor Thal'ena) rather than the
        /// class one. Always `false` for a player with no second power.
        second: bool,
    },
    /// Activate a Location in board slot `slot`.
    UseLocation {
        slot: u8,
        target: Option<Target>,
    },
    /// Prepare: spend all remaining mana to discount the card at `hand`.
    Prepare {
        hand: u8,
    },
    /// Trade: pay 1 mana to shuffle the Tradeable card at `hand` back into
    /// the deck and draw one. Not a play — the card itself never resolves.
    Trade {
        hand: u8,
    },
    #[default]
    EndTurn,
}

/// What a Trade costs. One mana, on every Tradeable card printed so far.
pub const TRADE_COST: i16 = 1;

/// A policy. The engine calls this; it never calls the engine's internals.
pub trait Agent {
    /// Choose from `legal`, which always contains at least [`Action::EndTurn`].
    fn choose(&mut self, game: &Game, legal: &[Action]) -> Action;

    /// Which of the `drawn` cards to keep, as a bitmask over their indices.
    /// The default keeps everything cheap enough to play early.
    fn mulligan(&mut self, _game: &Game, drawn: &[CardId], aggressive: bool) -> u32 {
        let threshold = if aggressive { 2 } else { 3 };
        let mut keep = 0;
        for (i, c) in drawn.iter().enumerate() {
            if c.def().cost <= threshold {
                keep |= 1 << i;
            }
        }
        keep
    }
}

impl Game {
    /// A new game. Decks are card lists; they are shuffled by [`Game::start`].
    pub fn new(
        deck0: (Class, &[CardId]),
        deck1: (Class, &[CardId]),
        seed: u64,
    ) -> Result<Game, &'static str> {
        let p0 = Player::new(deck0.0, hero_power_for(deck0.0)?, deck0.1);
        let p1 = Player::new(deck1.0, hero_power_for(deck1.0)?, deck1.1);
        Ok(Game {
            players: [p0, p1],
            current: Side::Player0,
            turn: 0,
            outcome: None,
            rngs: crate::rng::Rngs::new(seed),
            board_dirty: true,
            deaths_this_turn: 0,
            trigger_depth: 0,
            countered: false,
        })
    }

    // ------------------------------------------------------------- setup

    /// Shuffle, mulligan, hand out the Coin. `first` decides who leads, which
    /// the caller supplies rather than rolling so a batch can balance it —
    /// who goes first is a large, avoidable source of variance.
    pub fn start(&mut self, first: Side, agents: &mut [&mut dyn Agent; 2]) {
        self.current = first;
        for i in 0..2 {
            let mut deck = self.players[i].deck;
            self.rngs.library[i].shuffle(deck.as_mut_slice());
            self.players[i].deck = deck;
        }

        for (order, side) in [first, first.other()].into_iter().enumerate() {
            let n = if order == 0 { 3 } else { 4 };
            self.mulligan(side, n, agents[side.index()]);
        }

        // Captured before Start of Game runs, so an effect that changes a
        // hand this turn (King Llane) is not mistaken for part of it.
        for side in [Side::Player0, Side::Player1] {
            let p = self.player_mut(side);
            p.starting_hand = p.hand.iter().map(|hc| hc.card).collect();
        }

        self.fire_start_of_game();

        // The Coin goes to whoever is on the draw.
        if let Some(coin) = by_name("The Coin") {
            let p = self.player_mut(first.other());
            p.hand.push(HandCard::new(coin));
        }
    }

    /// Fire every Start of Game effect, for every copy in a player's opening
    /// hand or deck.
    ///
    /// Both sides are snapshotted before any of them run, so an effect that
    /// moves a card into the opponent's deck (King Llane) cannot cause it to
    /// be discovered and fired a second time by that opponent's own scan.
    fn fire_start_of_game(&mut self) {
        let mut queued: Inline<(Side, CardId), { 2 * (MAX_HAND + MAX_DECK) }> = Inline::new();
        for side in [Side::Player0, Side::Player1] {
            for hc in self.player(side).hand.iter() {
                if behaviour_of(hc.card)
                    .and_then(|b| b.start_of_game)
                    .is_some()
                {
                    queued.push((side, hc.card));
                }
            }
            for &card in self.player(side).deck.iter() {
                if behaviour_of(card).and_then(|b| b.start_of_game).is_some() {
                    queued.push((side, card));
                }
            }
        }
        for (side, card) in queued.iter().copied() {
            if let Some(f) = behaviour_of(card).and_then(|b| b.start_of_game) {
                f(
                    self,
                    &Ctx {
                        card,
                        side,
                        target: None,
                        source: None,
                        outcast: false,
                        dying: None,
                        marks: Marks::NONE,
                        mana_spent: 0,
                    },
                );
            }
        }
    }

    fn mulligan(&mut self, side: Side, n: usize, agent: &mut dyn Agent) {
        let i = side.index();
        let mut drawn: Inline<CardId, 5> = Inline::new();
        for _ in 0..n {
            if let Some(c) = self.players[i].deck.pop() {
                drawn.push(c);
            }
        }
        let aggressive = false;
        let snapshot = *self;
        let keep_mask = agent.mulligan(&snapshot, drawn.as_slice(), aggressive);

        // Returned cards go back before the reshuffle, exactly as the real
        // mulligan works: you cannot draw the card you just threw away until
        // the deck is shuffled again.
        for (k, c) in drawn.iter().enumerate() {
            if keep_mask & (1 << k) == 0 {
                self.players[i].deck.push(*c);
            }
        }
        let mut deck = self.players[i].deck;
        self.rngs.library[i].shuffle(deck.as_mut_slice());
        self.players[i].deck = deck;

        for (k, c) in drawn.iter().enumerate() {
            if keep_mask & (1 << k) != 0 {
                self.players[i].hand.push(HandCard::new(*c));
            }
        }
        // Top up to the mulligan size with fresh cards.
        while self.players[i].hand.len() < n {
            match self.players[i].deck.pop() {
                Some(c) => {
                    self.players[i].hand.push(HandCard::new(c));
                }
                None => break,
            }
        }
    }


    /// What the card at `hand_idx` costs right now.
    ///
    /// The per-copy `cost_delta` on the card in hand covers effects that have
    /// already been applied to it; the behaviour's `cost_delta` is read live,
    /// because conditions like "if you're holding a Dragon" stop applying the
    /// moment that Dragon is played.
    pub fn card_cost(&self, side: Side, hand_idx: usize) -> i16 {
        let Some(hc) = self.player(side).hand.get(hand_idx) else {
            return 0;
        };
        if pays_with_corpses(hc.card) {
            // Free in Mana terms; legal_actions and play_card separately
            // gate and spend the same number in Corpses instead.
            return 0;
        }
        let mut cost = hc.card.def().cost + hc.cost_delta;
        if let Some(f) = behaviour_of(hc.card).and_then(|b| b.cost_delta) {
            cost += f(self, side, hand_idx);
        }
        if hc.card.def().kind() == Kind::Spell {
            cost -= self.player(side).next_spell_discount;
        }
        if hc.card.def().kind() == Kind::Minion && hc.card.def().races.any(Races::BEAST) {
            cost -= self.player(side).next_beast_discount;
        }
        // Mug's Magic (Mug'Zee's Passive Hero Power): the first minion each
        // turn costs 2 less, from turn 3 on. Checked by name rather than a
        // stored flag, the same way `hero_power_target` reads the equipped
        // power -- "which power" already lives in `hero_power`.
        if hc.card.def().kind() == Kind::Minion
            && self.turn >= 3
            && !self.player(side).first_minion_discounted_turn
            && self.player(side).hero_power.name() == "Mug's Magic"
        {
            cost -= 2;
        }
        // Naralex, Herald of the Flights: the first Dragon each turn costs a
        // flat 1, not "1 less" -- checked live against the board, since
        // Naralex can leave play mid-turn and take the discount with it,
        // unlike Mug's Magic above which lives on the Hero Power slot.
        if hc.card.def().kind() == Kind::Minion
            && hc.card.def().races.any(Races::DRAGON)
            && !self.player(side).dragon_discounted_turn
            && self
                .player(side)
                .board
                .iter()
                .any(|m| m.card.name() == "Naralex, Herald of the Flights")
        {
            cost = 1;
        }
        if hc.card.def().kind() == Kind::Spell {
            cost += self.player(side).spell_tax_active;
        }
        cost.max(0)
    }

    // -------------------------------------------------------- turn cycle

    pub fn begin_turn(&mut self) {
        let side = self.current;
        let p = self.player_mut(side);
        if p.crystals < MAX_MANA {
            p.crystals += 1;
        }
        p.overload_now = p.overload_next;
        p.overload_next = 0;
        p.mana = (p.crystals - p.overload_now).max(0);
        p.hero_power_uses = 0;
        p.second_hero_power_uses = 0;
        p.friendly_damaged_turn = 0;
        p.hero_attacks_done = 0;
        p.hero_bonus_atk = 0;
        p.cards_played_turn = 0;
        p.spells_cast_turn = 0;
        p.schools_cast_turn = 0;
        p.first_minion_discounted_turn = false;
        p.dragon_discounted_turn = false;
        // A tax queued for this turn becomes active for it; this also clears
        // last turn's, since the promotion always overwrites regardless of
        // whether anything queued a new one.
        p.spell_tax_active = p.spell_tax_pending;
        p.spell_tax_pending = 0;
        for m in p.board.iter_mut() {
            m.attacks_done = 0;
            m.flags.remove(Flags::JUST_SUMMONED);
            m.flags.remove(Flags::ATTACKED);
            m.flags.remove(Flags::USED);
            m.cooldown = m.cooldown.saturating_sub(1);
            if m.dormant > 0 {
                m.dormant -= 1;
                if m.dormant == 0 {
                    m.flags.remove(Flags::DORMANT);
                    // A minion waking from dormancy is newly in play.
                    m.flags.insert(Flags::JUST_SUMMONED);
                }
            }
        }
        // Tick every effect queued against this player's own turns: each
        // fires once here, and stays queued (with one fewer turn left) if it
        // is not yet spent. Collected first and fired after `p` is dropped,
        // since firing needs the whole `Game` (summoning, damaging a hero).
        let mut fired: Inline<Pending, 4> = Inline::new();
        let mut remaining: Inline<Pending, 4> = Inline::new();
        for entry in p.pending.iter().copied() {
            fired.push(entry);
            if entry.turns_left > 1 {
                remaining.push(Pending {
                    turns_left: entry.turns_left - 1,
                    ..entry
                });
            }
        }
        p.pending = remaining;

        self.deaths_this_turn = 0;
        // Godfrey the Betrayer: one overdrawn card returns per turn, ahead of
        // the guaranteed draw below so it does not have to compete with it
        // for the same slot. Discounted permanently, not just this turn.
        let p = self.player_mut(side);
        if p.godfrey_active && p.hand.len() < MAX_HAND {
            if let Some(card) = p.overdrawn.pop() {
                let mut hc = HandCard::new(card);
                hc.cost_delta = -1;
                self.player_mut(side).hand.push(hc);
            }
        }
        // Irida Sinseeker: two cards from the Void, same "ahead of the
        // guaranteed draw" ordering as Godfrey's return just above.
        for _ in 0..2 {
            let Some(card) = self.player_mut(side).void.pop() else {
                break;
            };
            self.give_card(side, card);
        }
        self.draw(side, 1);
        self.board_dirty = true;
        for entry in fired.iter().copied() {
            match entry.kind {
                PendingKind::TempCrystal => self.gain_temp_mana(side, entry.amount),
                PendingKind::SummonToken => {
                    self.summon(side, entry.card);
                }
                PendingKind::HeroDamage => {
                    self.damage_hero(side, entry.amount);
                }
                PendingKind::None => {}
            }
        }
        self.fire(Event::TurnStart { side });
    }

    pub fn end_turn(&mut self) {
        let side = self.current;
        // Fired before anything is cleaned up, so an end-of-turn effect sees
        // the board as the player left it.
        self.fire(Event::TurnEnd { side });
        let p = self.player_mut(side);
        // Frozen characters thaw at the end of their controller's turn, unless
        // they were frozen during it.
        for m in p.board.iter_mut() {
            if m.flags.has(Flags::FROZE_THIS_TURN) {
                m.flags.remove(Flags::FROZE_THIS_TURN);
            } else {
                m.flags.remove(Flags::FROZEN);
            }
            // Take back exactly what "this turn only" gave.
            m.atk -= m.temp_atk;
            m.temp_atk = 0;
            if m.flags.has(Flags::DOOMED) {
                m.flags.insert(Flags::PENDING_DESTROY);
            }
        }
        if p.hero_froze_this_turn {
            p.hero_froze_this_turn = false;
        } else {
            p.hero_frozen = false;
        }
        p.hero_bonus_atk = 0;
        p.played_races_last = p.played_races_turn;
        p.played_races_turn = Races::NONE;
        p.next_spell_discount = 0;
        p.next_beast_discount = 0;
        // The Fins Beyond Time: swap back whatever hand this turn started
        // with, discarding the temporary starting-hand copies and anything
        // drawn into them since.
        if let Some(saved) = p.swapped_hand.take() {
            p.hand = saved;
        }
        // Cursed Chains: any minion stolen from `side` returns now that
        // side's own turn has ended, wherever it currently sits on the
        // thief's board -- found by the flag on the permanent itself, not
        // by card identity, so two copies of the same card are never
        // confused with each other.
        let thief = side.other();
        let mut returning: Inline<Permanent, MAX_BOARD> = Inline::new();
        let mut i = 0;
        while i < self.player(thief).board.len() {
            if self.player(thief).board[i].stolen_from == Some(side) {
                if let Some(mut m) = self.player_mut(thief).board.remove(i) {
                    m.stolen_from = None;
                    returning.push(m);
                }
            } else {
                i += 1;
            }
        }
        if !returning.is_empty() {
            for m in returning.iter().copied() {
                // A full board loses the return rather than the minion: if
                // there is nowhere to put it, it simply stays with the
                // thief instead of vanishing outright.
                if !self.player_mut(side).board.push(m) {
                    self.player_mut(thief).board.push(m);
                }
            }
            self.board_dirty = true;
            self.recompute_auras();
        }
        self.sweep_deaths();
    }

    /// Play one full game against two agents, returning how it ended.
    pub fn run(&mut self, first: Side, agents: &mut [&mut dyn Agent; 2]) -> Outcome {
        self.start(first, agents);
        self.play_out(agents)
    }

    /// Everything after [`start`](Self::start): turns until someone wins.
    ///
    /// Split out so a caller that needs to *see* the opening — telemetry
    /// wants the post-mulligan hand and the shuffled deck before a card is
    /// drawn from it — does not have to re-implement the turn loop and drift
    /// out of step with it.
    pub fn play_out(&mut self, agents: &mut [&mut dyn Agent; 2]) -> Outcome {
        while !self.is_over() {
            self.turn += 1;
            if self.turn > TURN_LIMIT {
                self.outcome = Some(Outcome::Draw);
                break;
            }
            self.begin_turn();
            if self.is_over() {
                break;
            }
            self.take_turn(agents[self.current.index()]);
            if self.is_over() {
                break;
            }
            self.end_turn();
            self.current = self.current.other();
        }
        self.outcome.unwrap_or(Outcome::Draw)
    }

    /// Let one agent act until it ends the turn.
    ///
    /// The step cap is a safety net against a policy that loops on an action
    /// with no effect; a real turn uses a handful of steps.
    pub fn take_turn(&mut self, agent: &mut dyn Agent) {
        let mut legal: Inline<Action, MAX_ACTIONS> = Inline::new();
        for _ in 0..128 {
            if self.is_over() {
                return;
            }
            self.legal_actions(&mut legal);
            let choice = agent.choose(self, legal.as_slice());
            if choice == Action::EndTurn {
                return;
            }
            if !self.apply(choice) {
                // A policy that proposes an illegal action would otherwise
                // spin; ending the turn is the safe interpretation.
                return;
            }
        }
    }

    // ------------------------------------------------------------ actions

    /// Every legal action in this position, including [`Action::EndTurn`].
    pub fn legal_actions(&self, out: &mut Inline<Action, MAX_ACTIONS>) {
        out.clear();
        if self.is_over() {
            out.push(Action::EndTurn);
            return;
        }
        let me = self.me();
        let side = self.current;

        // --- cards in hand
        for (i, c) in me.hand.iter().enumerate() {
            if c.locked_turn == self.turn {
                continue;
            }
            let d = c.card.def();
            if self.card_cost(side, i) > me.mana {
                continue;
            }
            if pays_with_corpses(c.card) && me.corpses < d.cost {
                continue;
            }
            let needs_board = matches!(d.kind(), Kind::Minion | Kind::Location);
            if needs_board && me.board.is_full() {
                continue;
            }
            let beh = behaviour_of(c.card);
            match d.kind() {
                Kind::Minion | Kind::Weapon | Kind::Location => {}
                // An unimplemented spell would cost mana and do nothing, which
                // is worse than not being offered at all. A secret or a
                // Quest/Sidequest counts as implemented through its own hook
                // rather than a cast effect -- a Quest's progress lives in
                // its `trigger`, the only hook it needs.
                Kind::Spell
                    if beh.is_some_and(|b| {
                        b.spell.is_some()
                            || b.secret.is_some()
                            || b.choose.is_some()
                            || ((d.keywords.has(Keywords::QUEST)
                                || d.keywords.has(Keywords::SIDE_QUEST))
                                && b.trigger.is_some())
                    }) => {}
                // A Hero card is played from hand like any other: it hands
                // its controller armor and fires its Battlecry. One with no
                // Battlecry would be armor and nothing else, and this engine
                // has none such -- so it is offered only when implemented,
                // the same rule spells follow.
                Kind::Hero if beh.is_some_and(|b| b.battlecry.is_some()) => {}
                Kind::Spell | Kind::Hero | Kind::HeroPower => continue,
            }

            // A secret already in the zone cannot be set again, and the zone
            // holds five. Both are ordinary game rules, so the card is simply
            // not offered.
            if d.kind() == Kind::Spell && d.keywords.has(Keywords::SECRET) {
                let zone = &me.secrets;
                if zone.len() >= crate::state::MAX_SECRETS || zone.contains(&c.card) {
                    continue;
                }
                out.push(Action::Play {
                    hand: i as u8,
                    target: None,
                    position: u8::MAX,
                    choice: u8::MAX,
                });
                continue;
            }
            // A Quest and a Sidequest each occupy their own single slot,
            // separate from each other and from Secrets.
            if d.kind() == Kind::Spell && d.keywords.has(Keywords::QUEST) {
                if me.quest.is_some() {
                    continue;
                }
                out.push(Action::Play {
                    hand: i as u8,
                    target: None,
                    position: u8::MAX,
                    choice: u8::MAX,
                });
                continue;
            }
            if d.kind() == Kind::Spell && d.keywords.has(Keywords::SIDE_QUEST) {
                if me.sidequest.is_some() {
                    continue;
                }
                out.push(Action::Play {
                    hand: i as u8,
                    target: None,
                    position: u8::MAX,
                    choice: u8::MAX,
                });
                continue;
            }

            // Choose One: each mode is its own action, because the two halves
            // can want different things pointed at.
            if let Some(modes) = beh.and_then(|b| b.choose) {
                for (k, mode) in modes.iter().enumerate() {
                    if !mode.target.needed() {
                        out.push(Action::Play {
                            hand: i as u8,
                            target: None,
                            position: u8::MAX,
                            choice: k as u8,
                        });
                        continue;
                    }
                    let mut any = false;
                    for t in self.targetable(true) {
                        if mode.target.matches(self, side, t) {
                            any = true;
                            out.push(Action::Play {
                                hand: i as u8,
                                target: Some(t),
                                position: u8::MAX,
                                choice: k as u8,
                            });
                        }
                    }
                    // A minion with an untargetable mode still comes down.
                    if !any && d.kind() != Kind::Spell {
                        out.push(Action::Play {
                            hand: i as u8,
                            target: None,
                            position: u8::MAX,
                            choice: k as u8,
                        });
                    }
                }
                continue;
            }

            let spec = beh.map_or(TargetSpec::None, |b| b.target);
            if !spec.needed() {
                out.push(Action::Play {
                    hand: i as u8,
                    target: None,
                    position: u8::MAX,
                    choice: u8::MAX,
                });
                continue;
            }
            let mut any = false;
            for t in self.targetable(true) {
                if spec.matches(self, side, t) {
                    any = true;
                    out.push(Action::Play {
                        hand: i as u8,
                        target: Some(t),
                        position: u8::MAX,
                        choice: u8::MAX,
                    });
                }
            }
            // A targeted *battlecry* is optional: the minion still comes down
            // when there is nothing to point at, and the battlecry is skipped.
            // A targeted *spell* cannot be cast at all.
            if !any && d.kind() != Kind::Spell {
                out.push(Action::Play {
                    hand: i as u8,
                    target: None,
                    position: u8::MAX,
                    choice: u8::MAX,
                });
            }
        }

        // --- attacks
        let must_taunt = self.player(side.other()).has_taunt();
        for (i, m) in me.board.iter().enumerate() {
            if !m.can_attack() {
                continue;
            }
            self.push_attack_targets(out, must_taunt, m.can_attack_face(), |t| Action::Attack {
                from: i as u8,
                target: t,
            });
        }
        if me.hero_can_attack() {
            self.push_attack_targets(out, must_taunt, true, |t| Action::HeroAttack { target: t });
        }

        // --- Prepare
        // Only offered when the card cannot be played this turn anyway:
        // banking mana into something already affordable is never right, and
        // offering it would multiply the branching factor for nothing.
        if me.mana > 0 {
            for (i, hc) in me.hand.iter().enumerate() {
                if hc.card.def().keywords.has(Keywords::PREPARE)
                    && hc.locked_turn != self.turn
                    && self.card_cost(side, i) > me.mana
                {
                    out.push(Action::Prepare { hand: i as u8 });
                }
            }
        }

        // --- Trade
        // Costs a mana of its own and is always available while you have one,
        // even for a card you could afford to play: trading a removal spell
        // you have no target for is the whole point of the keyword.
        if me.mana >= TRADE_COST && me.deck.len() < MAX_DECK {
            for (i, hc) in me.hand.iter().enumerate() {
                if hc.card.def().keywords.has(Keywords::TRADEABLE) && hc.locked_turn != self.turn {
                    out.push(Action::Trade { hand: i as u8 });
                }
            }
        }

        // --- locations already in play
        for (i, m) in me.board.iter().enumerate() {
            if m.kind() != Kind::Location || !m.active() {
                continue;
            }
            if m.flags.has(Flags::USED) || m.cooldown > 0 {
                continue;
            }
            let beh = behaviour_of(m.card);
            if beh.and_then(|b| b.spell).is_none() {
                continue;
            }
            let spec = beh.map_or(TargetSpec::None, |b| b.target);
            if !spec.needed() {
                out.push(Action::UseLocation {
                    slot: i as u8,
                    target: None,
                });
                continue;
            }
            for t in self.targetable(true) {
                if spec.matches(self, side, t) {
                    out.push(Action::UseLocation {
                        slot: i as u8,
                        target: Some(t),
                    });
                }
            }
        }

        // --- hero power
        let affordable = |hp: CardId| {
            if pays_with_corpses(hp) {
                me.corpses >= hp.def().cost
            } else {
                me.mana >= hp.def().cost
            }
        };
        if me.hero_power_uses == 0
            && affordable(me.hero_power)
            && !is_passive_hero_power(me.hero_power)
        {
            match hero_power_target(me.hero_power) {
                HpTarget::None => {
                    out.push(Action::HeroPower {
                        target: None,
                        second: false,
                    });
                }
                HpTarget::Any => {
                    for t in self.targetable(true) {
                        out.push(Action::HeroPower {
                            target: Some(t),
                            second: false,
                        });
                    }
                }
            }
        }
        if let Some(hp2) = me.second_hero_power
            && me.second_hero_power_uses == 0
            && affordable(hp2)
        {
            match hero_power_target(hp2) {
                HpTarget::None => {
                    out.push(Action::HeroPower {
                        target: None,
                        second: true,
                    });
                }
                HpTarget::Any => {
                    for t in self.targetable(true) {
                        out.push(Action::HeroPower {
                            target: Some(t),
                            second: true,
                        });
                    }
                }
            }
        }

        out.push(Action::EndTurn);
    }

    fn push_attack_targets(
        &self,
        out: &mut Inline<Action, MAX_ACTIONS>,
        must_taunt: bool,
        face_ok: bool,
        mk: impl Fn(Target) -> Action,
    ) {
        let foe = self.current.other();
        let them = self.player(foe);
        for (j, d) in them.board.iter().enumerate() {
            if !d.active() || !d.is_minion() || d.has(Keywords::STEALTH) {
                continue;
            }
            if must_taunt && !d.has(Keywords::TAUNT) {
                continue;
            }
            out.push(mk(Target::Minion(foe, j as u8)));
        }
        if face_ok && !must_taunt {
            out.push(mk(Target::Hero(foe)));
        }
    }

    /// Characters an effect may point at. `spell_like` applies the Elusive
    /// rule — "can't be targeted by spells or Hero Powers".
    pub fn targetable(&self, spell_like: bool) -> impl Iterator<Item = Target> + '_ {
        let me = self.current;
        let foe = me.other();
        let mine = self
            .player(me)
            .board
            .iter()
            .enumerate()
            .filter_map(move |(i, m)| {
                (m.active() && m.is_minion() && !(spell_like && m.has(Keywords::ELUSIVE)))
                    .then_some(Target::Minion(me, i as u8))
            });
        let theirs = self
            .player(foe)
            .board
            .iter()
            .enumerate()
            .filter_map(move |(i, m)| {
                (m.active()
                    && m.is_minion()
                    && !m.has(Keywords::STEALTH)
                    && !(spell_like && m.has(Keywords::ELUSIVE)))
                .then_some(Target::Minion(foe, i as u8))
            });
        [Target::Hero(me), Target::Hero(foe)]
            .into_iter()
            .chain(mine)
            .chain(theirs)
    }

    /// Execute an action. Returns false if it was not legal, leaving the state
    /// untouched.
    pub fn apply(&mut self, a: Action) -> bool {
        if self.is_over() {
            return false;
        }
        match a {
            Action::Play {
                hand,
                target,
                position,
                choice,
            } => self.play_card(hand as usize, target, position, choice),
            Action::Attack { from, target } => self.attack_with(Some(from as usize), target),
            Action::HeroAttack { target } => self.attack_with(None, target),
            Action::HeroPower { target, second } => self.use_hero_power(target, second),
            Action::UseLocation { slot, target } => self.use_location(slot as usize, target),
            Action::Prepare { hand } => self.prepare_card(hand as usize),
            Action::Trade { hand } => self.trade_card(hand as usize),
            Action::EndTurn => true,
        }
    }

    // -------------------------------------------------------- playing cards

    fn play_card(
        &mut self,
        hand_idx: usize,
        target: Option<Target>,
        position: u8,
        choice: u8,
    ) -> bool {
        let side = self.current;
        let Some(hc) = self.player(side).hand.get(hand_idx).copied() else {
            return false;
        };
        let cost = self.card_cost(side, hand_idx);
        if cost > self.player(side).mana || hc.locked_turn == self.turn {
            return false;
        }
        let def = hc.card.def();
        let beh = behaviour_of(hc.card);

        match def.kind() {
            Kind::Minion | Kind::Location if self.player(side).board.is_full() => return false,
            Kind::Spell
                if !beh.is_some_and(|b| {
                    b.spell.is_some()
                        || b.secret.is_some()
                        || b.choose.is_some()
                        || ((def.keywords.has(Keywords::QUEST)
                            || def.keywords.has(Keywords::SIDE_QUEST))
                            && b.trigger.is_some())
                }) =>
            {
                return false;
            }
            Kind::Hero if !beh.is_some_and(|b| b.battlecry.is_some()) => return false,
            Kind::HeroPower => return false,
            _ => {}
        }
        // The target is re-checked here rather than trusted: a search or a
        // replay reaches `apply` without ever calling `legal_actions`.
        // With a mode chosen, its requirement replaces the card's own.
        let chosen = beh
            .and_then(|b| b.choose)
            .and_then(|modes| modes.get(choice as usize));
        let spec = match chosen {
            Some(mode) => mode.target,
            None => beh.map_or(TargetSpec::None, |b| b.target),
        };
        let target = match target {
            Some(t) if spec.needed() && spec.matches(self, side, t) => Some(t),
            Some(_) => return false,
            None if spec.needed() && def.kind() == Kind::Spell => return false,
            None => None,
        };

        // Outcast is about where the card sat in hand, so it has to be read
        // before the card leaves it. A one-card hand is both ends at once.
        let hand_len = self.player(side).hand.len();
        let outcast = hand_idx == 0 || hand_idx + 1 == hand_len;

        // Everything above is pure validation; this is the last check before
        // any state changes, so a card paid for in Corpses cannot spend them
        // and then fail some later legality check.
        if pays_with_corpses(hc.card) && !self.spend_corpses(side, def.cost) {
            return false;
        }

        let p = self.player_mut(side);
        p.mana -= cost;
        if def.kind() == Kind::Spell {
            // A pending discount is spent by the first spell that uses it.
            p.next_spell_discount = 0;
        }
        p.hand.remove(hand_idx);
        p.cards_played_turn += 1;
        if def.kind() == Kind::Spell {
            p.spells_cast_turn += 1;
            p.schools_cast_turn |= 1 << def.school;
        }
        if def.kind() == Kind::Minion {
            p.played_races_turn |= def.races;
            if def.races.any(Races::BEAST) {
                // The discount is spent by the first Beast that uses it.
                p.next_beast_discount = 0;
            }
            p.minions_played_total = p.minions_played_total.saturating_add(1);
            // Spent by the first minion regardless of whether Mug's Magic was
            // even equipped yet -- matching how `next_beast_discount` above
            // clears unconditionally too.
            p.first_minion_discounted_turn = true;
            if def.races.any(Races::DRAGON)
                && p.board
                    .iter()
                    .any(|m| m.card.name() == "Naralex, Herald of the Flights")
            {
                p.dragon_discounted_turn = true;
            }
        }
        if def.overload > 0 {
            p.overload_next += def.overload as i16;
        }
        // "While holding this" marks, applied to whatever is left in hand —
        // the card just played has already been removed above.
        if def.kind() == Kind::Minion {
            for other in p.hand.iter_mut() {
                other.marks.insert(Marks::PLAYED_MINION);
            }
        }
        // Corpses-paid cards already cost 0 Mana here, so this stays correct
        // without a separate check (Merithra of the Dream).
        for other in p.hand.iter_mut() {
            other.mana_spent_while_held = other.mana_spent_while_held.saturating_add(cost);
        }
        if def.class() != Class::Neutral && def.class() != p.class {
            for other in p.hand.iter_mut() {
                other.marks.insert(Marks::PLAYED_OPPONENT_CARD);
            }
        }
        for other in p.hand.iter_mut() {
            if def.cost > other.card.def().cost {
                other.marks.insert(Marks::PLAYED_HIGHER_COST);
            }
        }

        // A secret is set, not cast: it goes to its own zone and waits.
        if def.kind() == Kind::Spell && def.keywords.has(Keywords::SECRET) {
            self.player_mut(side).secrets.push(hc.card);
            self.fire(Event::CardPlayed {
                side,
                card: hc.card,
            });
            self.sweep_deaths();
            return true;
        }
        // A Quest or Sidequest is set too, in its own single-card slot; its
        // progress lives entirely in its `trigger`, fired like any other
        // reactor's (see `Game::fire`).
        if def.kind() == Kind::Spell && def.keywords.has(Keywords::QUEST) {
            self.player_mut(side).quest = Some((hc.card, 0));
            self.fire(Event::CardPlayed {
                side,
                card: hc.card,
            });
            self.sweep_deaths();
            return true;
        }
        if def.kind() == Kind::Spell && def.keywords.has(Keywords::SIDE_QUEST) {
            self.player_mut(side).sidequest = Some((hc.card, 0));
            self.fire(Event::CardPlayed {
                side,
                card: hc.card,
            });
            self.sweep_deaths();
            return true;
        }

        let mut slot = None;
        let mut broken_weapon = None;
        match def.kind() {
            Kind::Minion | Kind::Location => {
                let mut m = Permanent::summon(hc.card);
                // Cleared once this card's own CardPlayed event has gone out;
                // see `Flags::BEING_PLAYED`.
                m.flags.insert(Flags::BEING_PLAYED);
                if position == u8::MAX {
                    p.board.push(m);
                    slot = Some(p.board.len() as u8 - 1);
                } else {
                    let at = (position as usize).min(p.board.len());
                    p.board.insert(at, m);
                    slot = Some(at as u8);
                }
            }
            Kind::Weapon => {
                // Equipping over an existing weapon breaks it, same as any
                // other way a weapon can leave play.
                broken_weapon = p.weapon.replace(Weapon::equip(hc.card));
            }
            // A Hero card replaces the hero's armor and Hero Power, not its
            // health: the printed Health on a hero card is the starting
            // total for a new game, not something a card played on turn ten
            // hands back. The Hero Power comes from the card's own Battlecry
            // (`Deathwing, Worldbreaker` equips Ruthless), because which
            // power a hero grants is not in the corpus as a field.
            Kind::Hero => p.armor += def.armor,
            Kind::Spell => {}
            _ => unreachable!("filtered above"),
        }
        self.board_dirty = true;
        if let Some(old) = broken_weapon {
            self.fire_weapon_deathrattle(side, old);
        }

        // Zee's Might (Mug'Zee's other Passive Hero Power): every fifth
        // minion played triggers its own Battlecry a second time. Read once,
        // before either effect below can change the board under it.
        let double_battlecry = def.kind() == Kind::Minion
            && self.player(side).hero_power.name() == "Zee's Might"
            && self.player(side).minions_played_total % 5 == 0;
        // Sinestra: a spell from a class other than the caster's own casts
        // twice. Read the same way, before either effect below resolves.
        let double_spell = def.kind() == Kind::Spell
            && def.class() != Class::Neutral
            && def.class() != self.player(side).class
            && self
                .player(side)
                .board
                .iter()
                .any(|m| m.card.name() == "Sinestra");

        if let Some(mode) = chosen {
            let ctx = Ctx {
                card: hc.card,
                side,
                target,
                source: slot,
                outcast,
                dying: None,
                marks: hc.marks,
                mana_spent: hc.mana_spent_while_held,
            };
            if def.kind() == Kind::Spell {
                self.countered = false;
                self.fire(Event::SpellCasting {
                    side,
                    card: hc.card,
                });
                let countered = self.countered;
                self.countered = false;
                if !countered {
                    (mode.effect)(self, &ctx);
                    if double_spell {
                        (mode.effect)(self, &ctx);
                    }
                }
            } else {
                (mode.effect)(self, &ctx);
                if double_battlecry {
                    (mode.effect)(self, &ctx);
                }
            }
        } else if let Some(b) = beh {
            let ctx = Ctx {
                card: hc.card,
                side,
                target,
                source: slot,
                outcast,
                dying: None,
                marks: hc.marks,
                mana_spent: hc.mana_spent_while_held,
            };
            if def.kind() == Kind::Spell {
                // Counterspell gets its chance before the spell resolves.
                self.countered = false;
                self.fire(Event::SpellCasting {
                    side,
                    card: hc.card,
                });
                let countered = self.countered;
                self.countered = false;
                if let Some(f) = b.spell
                    && !countered
                {
                    f(self, &ctx);
                    if double_spell {
                        f(self, &ctx);
                    }
                }
            } else if let Some(f) = b.battlecry {
                f(self, &ctx);
                if double_battlecry {
                    f(self, &ctx);
                }
            }
        }
        self.sweep_deaths();

        // Order follows the game: the minion is already in play before anything
        // reacts to it, and a spell has finished resolving before "after you
        // cast a spell" sees it. Wild Pyromancer depends on exactly this.
        if matches!(def.kind(), Kind::Minion | Kind::Location) {
            self.fire(Event::MinionSummoned {
                side,
                card: hc.card,
                slot: slot.unwrap_or(0),
            });
        }
        if def.kind() == Kind::Spell {
            self.fire(Event::SpellCast {
                side,
                card: hc.card,
            });
        }
        self.fire(Event::CardPlayed {
            side,
            card: hc.card,
        });
        for m in self.player_mut(side).board.iter_mut() {
            m.flags.remove(Flags::BEING_PLAYED);
        }
        // One sweep, once everything the card set in motion has resolved.
        self.sweep_deaths();
        true
    }


    /// Prepare: bank the mana you have left into a discount on one card.
    ///
    /// The rule — taken from the reference engine, since the printed text is
    /// only the bare keyword — is `cost_delta -= mana + 1`, and the card is
    /// locked for the rest of the turn, so it cannot be banked into and then
    /// played at the reduced price on the same turn.
    fn prepare_card(&mut self, hand_idx: usize) -> bool {
        let side = self.current;
        let Some(hc) = self.player(side).hand.get(hand_idx).copied() else {
            return false;
        };
        if !hc.card.def().keywords.has(Keywords::PREPARE) || hc.locked_turn == self.turn {
            return false;
        }
        let mana = self.player(side).mana;
        if mana <= 0 {
            return false;
        }
        let turn = self.turn;
        let p = self.player_mut(side);
        p.hand[hand_idx].cost_delta -= mana + 1;
        p.hand[hand_idx].locked_turn = turn;
        p.mana = 0;
        true
    }

    /// Trade: one mana to put a Tradeable card back into the deck and draw.
    ///
    /// The card is shuffled in rather than put on top, and the draw happens
    /// afterwards — so trading can hand the same card straight back, exactly
    /// as it can in the game.
    fn trade_card(&mut self, hand_idx: usize) -> bool {
        let side = self.current;
        let Some(hc) = self.player(side).hand.get(hand_idx).copied() else {
            return false;
        };
        if !hc.card.def().keywords.has(Keywords::TRADEABLE) || hc.locked_turn == self.turn {
            return false;
        }
        if self.player(side).mana < TRADE_COST || self.player(side).deck.len() >= MAX_DECK {
            return false;
        }
        let p = self.player_mut(side);
        p.mana -= TRADE_COST;
        p.hand.remove(hand_idx);
        // A traded card goes back as a plain card: whatever discount or mark
        // it was carrying in hand is gone, because the deck holds card ids
        // and nothing else.
        self.shuffle_into_deck(side, hc.card);
        self.draw(side, 1);
        true
    }

    /// Put a card in hand, or burn it if the hand is full.
    ///
    /// Godfrey the Betrayer changes only the "burn" half: instead of vanishing,
    /// the card waits in `overdrawn` for space, returned discounted from
    /// `begin_turn`. Everything that reaches hand this way -- a draw, a
    /// Discover, any other `give_card` caller -- is "overdrawn" the same way
    /// a fatigue-style empty draw is not: this only ever fires when a card
    /// existed and had nowhere to go, which is exactly what the card means.
    /// Credit `amount` of mana spent to every card currently in `side`'s
    /// hand (Merithra of the Dream). Called from wherever mana is actually
    /// deducted -- a card play, a Hero Power -- rather than from `give_card`,
    /// since a card just added to hand should not count mana spent to add it.
    fn add_mana_spent_to_hand(&mut self, side: Side, amount: i16) {
        for hc in self.player_mut(side).hand.iter_mut() {
            hc.mana_spent_while_held = hc.mana_spent_while_held.saturating_add(amount);
        }
    }

    pub fn give_card(&mut self, side: Side, card: CardId) -> bool {
        let p = self.player_mut(side);
        if p.hand.len() >= MAX_HAND {
            if p.godfrey_active {
                p.overdrawn.push(card);
            }
            return false;
        }
        p.hand.push(HandCard::new(card))
    }

    /// Draw `n` cards, taking fatigue for each empty draw.
    pub fn draw(&mut self, side: Side, n: usize) {
        for _ in 0..n {
            let card = self.player_mut(side).deck.pop();
            match card {
                Some(c) => {
                    self.give_card(side, c);
                    self.fire(Event::CardDrawn { side });
                }
                None => {
                    let p = self.player_mut(side);
                    p.fatigue += 1;
                    let dmg = p.fatigue;
                    self.damage_hero(side, dmg);
                }
            }
            if self.is_over() {
                return;
            }
        }
    }

    /// Summon a minion for `side`, if there is room.
    pub fn summon(&mut self, side: Side, card: CardId) -> bool {
        let p = self.player_mut(side);
        if p.board.is_full() {
            return false;
        }
        let ok = p.board.push(Permanent::summon(card));
        let slot = p.board.len() as u8 - 1;
        self.board_dirty = true;
        // Before the summon event, so anything reacting to the arrival already
        // sees the aura the new minion projects and receives.
        self.recompute_auras();
        if ok {
            self.fire(Event::MinionSummoned { side, card, slot });
        }
        ok
    }

    // -------------------------------------------------------------- combat

    /// `from` is a board slot, or `None` for a hero attack.
    fn attack_with(&mut self, from: Option<usize>, target: Target) -> bool {
        let side = self.current;
        let foe = side.other();

        // Legality, including Taunt, which is enforced here and not left to
        // the action enumerator — a search or a replay can call `apply`
        // directly and must not be able to skip a Taunt.
        let must_taunt = self.player(foe).has_taunt();
        match target {
            Target::Hero(s) => {
                if s != foe || must_taunt {
                    return false;
                }
            }
            Target::Minion(s, i) => {
                let Some(d) = self.player(s).board.get(i as usize) else {
                    return false;
                };
                if s != foe || !d.active() || !d.is_minion() || d.has(Keywords::STEALTH) {
                    return false;
                }
                if must_taunt && !d.has(Keywords::TAUNT) {
                    return false;
                }
            }
        }

        let (atk, poison, lifesteal) = match from {
            Some(i) => {
                let Some(m) = self.player(side).board.get(i) else {
                    return false;
                };
                if !m.can_attack() {
                    return false;
                }
                if matches!(target, Target::Hero(_)) && !m.can_attack_face() {
                    return false;
                }
                (
                    m.atk,
                    m.has(Keywords::POISONOUS),
                    m.has(Keywords::LIFESTEAL),
                )
            }
            None => {
                if !self.player(side).hero_can_attack() {
                    return false;
                }
                let w = self.player(side).weapon;
                (
                    self.player(side).hero_attack(),
                    w.is_some_and(|w| w.card.def().keywords.has(Keywords::POISONOUS)),
                    w.is_some_and(|w| w.card.def().keywords.has(Keywords::LIFESTEAL)),
                )
            }
        };

        // Secrets that react to being attacked go off here, before any damage.
        // They may remove the attacker (Vaporize) or otherwise change the
        // board, so everything is re-validated afterwards.
        let attacker_t = match from {
            Some(i) => Target::Minion(side, i as u8),
            None => Target::Hero(side),
        };
        self.fire(Event::AttackDeclared {
            attacker: attacker_t,
            defender: target,
        });
        self.sweep_deaths();
        if self.is_over() || !self.attack_still_legal(from, target) {
            return true;
        }

        // Mark the attack before damage: a minion that dies mid-combat has
        // still used its swing.
        match from {
            Some(i) => {
                let m = &mut self.player_mut(side).board[i];
                m.attacks_done += 1;
                m.flags.insert(Flags::ATTACKED);
                m.keywords.remove(Keywords::STEALTH);
            }
            None => {
                self.player_mut(side).hero_attacks_done += 1;
            }
        }

        // The defender strikes back simultaneously, so its attack is read
        // before either side takes damage.
        let counter = match target {
            Target::Hero(_) => 0,
            Target::Minion(s, i) => self.player(s).board[i as usize].atk,
        };
        let (counter_poison, counter_lifesteal) = match target {
            Target::Hero(_) => (false, false),
            Target::Minion(s, i) => {
                let d = &self.player(s).board[i as usize];
                (d.has(Keywords::POISONOUS), d.has(Keywords::LIFESTEAL))
            }
        };

        let dealt = self.deal_damage(target, atk);
        if dealt && poison {
            self.poison(target);
        }
        if dealt && lifesteal && atk > 0 {
            self.heal_hero(side, atk);
        }

        if counter > 0 {
            let back = match from {
                Some(i) => Target::Minion(side, i as u8),
                None => Target::Hero(side),
            };
            let hit = self.deal_damage(back, counter);
            if hit && counter_poison {
                self.poison(back);
            }
            if hit && counter_lifesteal {
                self.heal_hero(foe, counter);
            }
        }

        // A hero swing spends weapon durability whether or not it connected.
        if from.is_none() {
            let mut spent = false;
            if let Some(w) = self.player_mut(side).weapon.as_mut() {
                w.durability -= 1;
                spent = w.durability <= 0;
            }
            if spent {
                self.destroy_weapon(side);
            }
        }

        // Captured before the sweep below removes the body.
        let defender_died = match target {
            Target::Minion(s, i) => self
                .player(s)
                .board
                .get(i as usize)
                .is_some_and(|m| m.is_dead()),
            Target::Hero(_) => false,
        };

        self.board_dirty = true;
        self.sweep_deaths();
        // Fired once the exchange has fully resolved: this is what "after
        // your hero attacks" listens to, and it must not see a board that
        // still holds the bodies.
        self.fire(Event::AfterAttack {
            attacker: attacker_t,
            defender: target,
            defender_died,
        });
        self.sweep_deaths();
        true
    }


    /// Whether a declared attack is still legal after secrets have resolved.
    ///
    /// A secret can destroy the attacker, bounce it, or clear the defender in
    /// between declaration and damage, so nothing about the exchange may be
    /// assumed once `AttackDeclared` has been fired.
    fn attack_still_legal(&self, from: Option<usize>, target: Target) -> bool {
        match from {
            Some(i) => match self.player(self.current).board.get(i) {
                Some(m) => {
                    if !m.can_attack() {
                        return false;
                    }
                }
                None => return false,
            },
            None => {
                if !self.player(self.current).hero_can_attack() {
                    return false;
                }
            }
        }
        match target {
            Target::Hero(_) => true,
            Target::Minion(s, i) => self
                .player(s)
                .board
                .get(i as usize)
                .is_some_and(|d| d.active() && d.is_minion()),
        }
    }

    /// Force the minion at `attacker` to deal its current attack to
    /// `defender` and take the defender's attack back, ignoring Taunt,
    /// summoning sickness, and whether it has already attacked -- for a card
    /// that assigns the target itself (Emergency Surgery, Spire of
    /// Solitude) rather than letting the normal action space choose it.
    ///
    /// A deliberate simplification against `attack_with`: no secrets fire,
    /// and there is no weapon durability to spend, since the attacker here
    /// is always a minion, never a hero.
    pub fn forced_attack(&mut self, attacker: (Side, u8), defender: Target) {
        let Some(m) = self
            .player(attacker.0)
            .board
            .get(attacker.1 as usize)
            .copied()
        else {
            return;
        };
        if !m.is_minion() || m.atk <= 0 {
            return;
        }
        let attacker_t = Target::Minion(attacker.0, attacker.1);
        let defender_side = match defender {
            Target::Hero(s) => s,
            Target::Minion(s, _) => s,
        };
        let (counter, counter_poison, counter_lifesteal) = match defender {
            Target::Hero(_) => (0, false, false),
            Target::Minion(s, i) => match self.player(s).board.get(i as usize) {
                Some(d) => (
                    d.atk,
                    d.has(Keywords::POISONOUS),
                    d.has(Keywords::LIFESTEAL),
                ),
                None => return,
            },
        };

        let dealt = self.deal_damage(defender, m.atk);
        if dealt && m.has(Keywords::POISONOUS) {
            self.poison(defender);
        }
        if dealt && m.has(Keywords::LIFESTEAL) && m.atk > 0 {
            self.heal_hero(attacker.0, m.atk);
        }
        if counter > 0 {
            let hit = self.deal_damage(attacker_t, counter);
            if hit && counter_poison {
                self.poison(attacker_t);
            }
            if hit && counter_lifesteal {
                self.heal_hero(defender_side, counter);
            }
        }
        self.board_dirty = true;
        self.sweep_deaths();
    }

    // -------------------------------------------------- damage and healing

    /// Deal `amount` damage. Returns whether damage actually landed, which is
    /// what Poisonous and Lifesteal key off — a Divine Shield pop is not a hit.
    pub fn deal_damage(&mut self, target: Target, amount: i16) -> bool {
        if amount <= 0 {
            return false;
        }
        match target {
            Target::Hero(s) => self.damage_hero(s, amount),
            Target::Minion(s, i) => {
                let Some(m) = self.player_mut(s).board.get_mut(i as usize) else {
                    return false;
                };
                if m.has(Keywords::IMMUNE) {
                    return false;
                }
                if m.has(Keywords::DIVINE_SHIELD) {
                    m.keywords.remove(Keywords::DIVINE_SHIELD);
                    return false;
                }
                m.damage += amount;
                self.fire(Event::Damaged { target, amount });
                true
            }
        }
    }

    /// Damage a hero, spending armor first. Returns whether damage actually
    /// landed, matching [`Game::deal_damage`] — a hero Divine Shield pop is
    /// not a hit, the same way a minion's is not.
    pub fn damage_hero(&mut self, side: Side, amount: i16) -> bool {
        if amount <= 0 {
            return false;
        }
        let p = self.player_mut(side);
        if p.hero_divine_shield {
            p.hero_divine_shield = false;
            return false;
        }
        let absorbed = amount.min(p.armor);
        p.armor -= absorbed;
        p.hero_hp -= amount - absorbed;
        self.check_over();
        self.fire(Event::Damaged {
            target: Target::Hero(side),
            amount,
        });
        true
    }

    pub fn heal_hero(&mut self, side: Side, amount: i16) {
        let p = self.player_mut(side);
        let before = p.hero_hp;
        // The ceiling is the starting total, except for a hero already above
        // it: Story of Amara sets Health to 40, and a heal must never be the
        // thing that takes those ten points away again.
        let cap = crate::state::START_HP.max(p.hero_hp);
        p.hero_hp = (p.hero_hp + amount).min(cap);
        // Healing a character already at full health is not a heal, and must
        // not wake "whenever a character is healed".
        let restored = p.hero_hp - before;
        if restored > 0 {
            self.fire(Event::Healed {
                target: Target::Hero(side),
                amount: restored,
            });
        }
    }

    pub fn heal(&mut self, target: Target, amount: i16) {
        if amount <= 0 {
            return;
        }
        match target {
            Target::Hero(s) => self.heal_hero(s, amount),
            Target::Minion(s, i) => {
                let Some(m) = self.player_mut(s).board.get_mut(i as usize) else {
                    return;
                };
                let before = m.damage;
                m.damage = (m.damage - amount).max(0);
                let restored = before - m.damage;
                if restored > 0 {
                    self.fire(Event::Healed {
                        target,
                        amount: restored,
                    });
                }
            }
        }
    }

    pub fn gain_armor(&mut self, side: Side, amount: i16) {
        self.player_mut(side).armor += amount;
    }

    /// Destroy outright, as Poisonous does. Heroes are unaffected.
    fn poison(&mut self, target: Target) {
        if let Target::Minion(s, i) = target
            && let Some(m) = self.player_mut(s).board.get_mut(i as usize)
            && !m.has(Keywords::CANT_BE_DESTROYED)
        {
            m.flags.insert(Flags::PENDING_DESTROY);
        }
    }

    /// Freeze a character. Heroes freeze too — that is the whole point of
    /// aiming a Frostbolt at a face.
    pub fn freeze(&mut self, target: Target) {
        // Read whose turn it is before borrowing: a character frozen during
        // its own controller's turn does not thaw at the end of it.
        let current = self.current;
        match target {
            Target::Hero(s) => {
                let p = self.player_mut(s);
                p.hero_frozen = true;
                if s == current {
                    p.hero_froze_this_turn = true;
                }
            }
            Target::Minion(s, i) => {
                if let Some(m) = self.player_mut(s).board.get_mut(i as usize) {
                    m.flags.insert(Flags::FROZEN);
                    if s == current {
                        m.flags.insert(Flags::FROZE_THIS_TURN);
                    }
                }
            }
        }
    }

    /// Remove dead minions and settle the game if a hero fell.
    ///
    /// Loops because a death can kill something else once deathrattles exist;
    /// the bound stops a pathological chain from hanging a batch run.
    pub fn sweep_deaths(&mut self) {
        // Bounded because a deathrattle can kill something else, which can kill
        // something else again; the cap stops a pathological chain from hanging
        // a batch run. It is a backstop, not a rule.
        for _ in 0..16 {
            let mut dying: Inline<(Side, CardId, u8, Permanent), { MAX_BOARD * 2 }> = Inline::new();
            for i in 0..2 {
                let side = Side::from_index(i);
                for (slot, m) in self.players[i].board.iter().enumerate() {
                    if m.is_dead() {
                        dying.push((side, m.card, slot as u8, *m));
                    }
                }
            }
            if dying.is_empty() {
                break;
            }
            self.deaths_this_turn = self.deaths_this_turn.saturating_add(dying.len() as u8);
            for i in 0..2 {
                self.players[i].board.retain(|m| !m.is_dead());
            }
            self.board_dirty = true;

            // Deathrattles fire once the bodies have left the board, in board
            // order. A minion summoned by one therefore lands at the end rather
            // than in the vacated slot — a documented simplification, and the
            // only place board position is not preserved exactly.
            for (side, card, slot, body) in dying.iter().copied() {
                if let Some(f) = behaviour_of(card).and_then(|b| b.deathrattle) {
                    f(
                        self,
                        &Ctx {
                            card,
                            side,
                            target: None,
                            source: Some(slot),
                            outcast: false,
                            dying: Some(body),
                            marks: Marks::NONE,
                            mana_spent: 0,
                        },
                    );
                }
                if self.is_over() {
                    return;
                }
            }
            // Reborn brings the body back once the deathrattle has run, with
            // one Health and without the keyword, so it cannot come back
            // twice. It returns as a fresh copy: buffs and enchantments are
            // lost, which is what the real rule does too. A minion that was
            // granted Reborn (Haunt) comes back the same way, which is why
            // this reads the dying permanent's live keywords rather than the
            // card's printed ones.
            for (side, card, _, body) in dying.iter().copied() {
                if !body.has(Keywords::REBORN) || !body.is_minion() {
                    continue;
                }
                if self.player(side).board.is_full() {
                    continue;
                }
                let mut back = Permanent::summon(card);
                back.keywords.remove(Keywords::REBORN);
                back.damage = (back.max_hp - 1).max(0);
                let p = self.player_mut(side);
                p.board.push(back);
                let slot = p.board.len() as u8 - 1;
                self.board_dirty = true;
                self.recompute_auras();
                self.fire(Event::MinionSummoned { side, card, slot });
                if self.is_over() {
                    return;
                }
            }
            // Death triggers come after every deathrattle in the batch, so a
            // board wipe grows Flesheating Ghoul once per body rather than
            // interleaving with the rattles.
            // Corpses are a rule, not a card: the Death Knight banks one for
            // every friendly minion that dies, however it died.
            for (side, _, _, _) in dying.iter().copied() {
                if self.player(side).class == Class::DeathKnight {
                    self.player_mut(side).corpses += 1;
                }
            }
            for (side, card, _, _) in dying.iter().copied() {
                self.fire(Event::MinionDied { side, card });
                if self.is_over() {
                    return;
                }
            }
        }
        // An aura source may have just left play, so the board's stats are
        // stale until this runs. Recomputing here covers every resolution
        // point, because every one of them ends in a sweep.
        self.recompute_auras();
        self.check_over();
    }

    /// Fire a weapon's deathrattle, if it has one. Called from wherever a
    /// weapon actually leaves play -- breaking from durability loss, being
    /// destroyed, or being replaced by a new one -- rather than from
    /// `sweep_deaths`, because a weapon is not a board permanent and never
    /// appears in its sweep.
    pub(crate) fn fire_weapon_deathrattle(&mut self, side: Side, w: Weapon) {
        if let Some(f) = behaviour_of(w.card).and_then(|b| b.deathrattle) {
            f(
                self,
                &Ctx {
                    card: w.card,
                    side,
                    target: None,
                    source: Some(crate::events::WEAPON_SLOT),
                    outcast: false,
                    dying: None,
                    marks: Marks::NONE,
                    mana_spent: 0,
                },
            );
        }
    }

    pub(crate) fn check_over(&mut self) {
        if self.outcome.is_some() {
            return;
        }
        let dead0 = self.players[0].is_dead();
        let dead1 = self.players[1].is_dead();
        self.outcome = match (dead0, dead1) {
            (true, true) => Some(Outcome::Draw),
            (true, false) => Some(Outcome::Win(Side::Player1)),
            (false, true) => Some(Outcome::Win(Side::Player0)),
            (false, false) => None,
        };
    }

    // ---------------------------------------------------------- hero power

    fn use_hero_power(&mut self, target: Option<Target>, second: bool) -> bool {
        let side = self.current;
        let me = self.player(side);
        let Some(hp) = (if second {
            me.second_hero_power
        } else {
            Some(me.hero_power)
        }) else {
            return false;
        };
        let cost = hp.def().cost;
        let uses = if second {
            me.second_hero_power_uses
        } else {
            me.hero_power_uses
        };
        if uses > 0 {
            return false;
        }
        let corpse_paid = pays_with_corpses(hp);
        if corpse_paid {
            if me.corpses < cost {
                return false;
            }
        } else if me.mana < cost {
            return false;
        }
        if hero_power_target(hp) == HpTarget::Any && target.is_none() {
            return false;
        }
        if corpse_paid {
            self.spend_corpses(side, cost);
        } else {
            self.player_mut(side).mana -= cost;
            self.add_mana_spent_to_hand(side, cost);
        }
        let p = self.player_mut(side);
        if second {
            p.second_hero_power_uses += 1;
        } else {
            p.hero_power_uses += 1;
        }

        let foe = side.other();
        match hp.info().name {
            "Fireblast" => {
                if let Some(t) = target {
                    self.deal_damage(t, 1);
                }
            }
            "Steady Shot" => {
                self.damage_hero(foe, 2);
            }
            "Life Tap" => {
                self.damage_hero(side, 2);
                if !self.is_over() {
                    self.draw(side, 1);
                }
            }
            "Lesser Heal" => self.heal(target.unwrap_or(Target::Hero(side)), 2),
            "Armor Up!" => self.gain_armor(side, 2),
            "Reinforce" => {
                if let Some(c) = by_name("Silver Hand Recruit") {
                    self.summon(side, c);
                }
            }
            "Totemic Call" => self.summon_random_totem(side),
            "Dagger Mastery" => {
                if let Some(c) = by_name("Wicked Knife") {
                    self.player_mut(side).weapon = Some(Weapon::equip(c));
                }
            }
            "Shapeshift" => {
                let p = self.player_mut(side);
                p.hero_bonus_atk += 1;
                p.armor += 1;
            }
            "Demon Claws" => self.player_mut(side).hero_bonus_atk += 1,
            // Deathwing's own power, equipped by his Battlecry.
            "Ruthless" => self.player_mut(side).hero_bonus_atk += 5,
            "Ghoul Charge" => {
                if let Some(c) = by_name("Frail Ghoul") {
                    self.summon(side, c);
                }
                self.damage_hero(side, 1);
            }
            "Vampyr's Kiss" => {
                if let Some(t) = target {
                    self.buff(t, 3, 0);
                }
            }
            // The one Hero Power that grows: Soul Immolation raises it by 1
            // every cast after the one that granted it. The corpus writes the
            // base damage as a script placeholder (`Deal @ damage`) and gives
            // no number, so the base is 1 -- the smallest value the card's
            // own "increase its damage by 1" is written against.
            "Collapsing Star" => {
                let damage = 1 + self.player(side).hero_power_bonus;
                if let Some(t) = self.random_enemy(side) {
                    self.deal_damage(t, damage);
                }
            }
            _ => {}
        }
        self.sweep_deaths();
        self.fire(Event::HeroPowerUsed { side });
        true
    }


    /// Activate a Location.
    ///
    /// A Location's activated ability lives in the card's `spell` hook: a
    /// Location is never a spell, so reusing the slot is unambiguous and saves
    /// a near-identical field on every card in the table.
    ///
    /// Using one costs no mana, spends a point of durability, and puts the
    /// Location on cooldown until its controller's next turn.
    fn use_location(&mut self, slot: usize, target: Option<Target>) -> bool {
        let side = self.current;
        let Some(loc) = self.player(side).board.get(slot).copied() else {
            return false;
        };
        if loc.kind() != Kind::Location
            || !loc.active()
            || loc.flags.has(Flags::USED)
            || loc.cooldown > 0
        {
            return false;
        }
        let beh = behaviour_of(loc.card);
        let Some(f) = beh.and_then(|b| b.spell) else {
            return false;
        };
        let spec = beh.map_or(TargetSpec::None, |b| b.target);
        let target = match target {
            Some(t) if spec.needed() && spec.matches(self, side, t) => Some(t),
            Some(_) => return false,
            None if spec.needed() => return false,
            None => None,
        };

        {
            let m = &mut self.player_mut(side).board[slot];
            m.flags.insert(Flags::USED);
            m.cooldown = 1;
            // Durability is tracked as damage against `max_hp`, so a spent
            // Location dies through the ordinary death sweep.
            m.damage += 1;
        }
        f(
            self,
            &Ctx {
                card: loc.card,
                side,
                target,
                source: Some(slot as u8),
                outcast: false,
                dying: None,
                marks: Marks::NONE,
                mana_spent: 0,
            },
        );
        self.board_dirty = true;
        self.sweep_deaths();
        true
    }

    /// Totemic Call summons one totem the player does not already have.
    fn summon_random_totem(&mut self, side: Side) {
        const TOTEMS: [&str; 4] = [
            "Searing Totem",
            "Stoneclaw Totem",
            "Healing Totem",
            "Strength Totem",
        ];
        let mut available: Inline<CardId, 4> = Inline::new();
        for name in TOTEMS {
            let Some(c) = by_name(name) else { continue };
            if !self.player(side).board.iter().any(|m| m.card == c) {
                available.push(c);
            }
        }
        if available.is_empty() {
            return;
        }
        let pick = self.rngs.effects.index(available.len());
        self.summon(side, available[pick]);
    }
}

/// Whether a hero power needs a target.
#[derive(PartialEq, Eq)]
enum HpTarget {
    None,
    Any,
}

fn hero_power_target(hp: CardId) -> HpTarget {
    match hp.info().name {
        "Fireblast" | "Lesser Heal" => HpTarget::Any,
        _ => HpTarget::None,
    }
}

impl Game {
    /// Count one more of `side`'s own characters taking damage this turn,
    /// and if that makes four, summon Warptooth from wherever it is sitting
    /// -- hand or deck -- if it is anywhere to be found. A card no longer in
    /// either zone (already on the board, or never drawn from a deck that
    /// does not run it) is simply not found, so this needs no separate
    /// "already summoned" flag.
    pub(crate) fn tick_warptooth(&mut self, side: Side) {
        let p = self.player_mut(side);
        p.friendly_damaged_turn = p.friendly_damaged_turn.saturating_add(1);
        if p.friendly_damaged_turn != 4 {
            return;
        }
        let Some(warptooth) = by_name("Warptooth") else {
            return;
        };
        if let Some(idx) = self
            .player(side)
            .hand
            .iter()
            .position(|hc| hc.card == warptooth)
        {
            self.player_mut(side).hand.remove(idx);
            self.summon(side, warptooth);
            return;
        }
        if let Some(idx) = self.player(side).deck.position(&warptooth) {
            self.player_mut(side).deck.remove(idx);
            self.summon(side, warptooth);
        }
    }

    /// Every copy of Shadow of Demise in `side`'s hand becomes a fresh copy
    /// of `cast`, whatever spell they were a moment ago. A full reset
    /// (`HandCard::new`), not just swapping `card`: a cost delta, a Prepare
    /// lock or a mark belonged to the old identity, not the new one.
    pub(crate) fn tick_shadow_of_demise(&mut self, side: Side, cast: CardId) {
        let Some(shadow) = by_name("Shadow of Demise") else {
            return;
        };
        for hc in self.player_mut(side).hand.iter_mut() {
            if hc.card == shadow {
                *hc = HandCard::new(cast);
            }
        }
    }
}

/// Whether `card` is paid for with Corpses instead of Mana (Reanimated
/// Pterrordax; Blood Doctor Thal'ena's granted second Hero Power, Vampyr's
/// Kiss). The amount is the card's own printed cost, same number, different
/// resource, so nothing else needs to know "how many".
fn pays_with_corpses(card: CardId) -> bool {
    matches!(card.name(), "Reanimated Pterrordax" | "Vampyr's Kiss")
}

/// A Hero Power with no `Action::HeroPower` to take at all (Mug'Zee's
/// Passives: Mug's Magic discounts a minion automatically, Zee's Might
/// doubles a Battlecry automatically). Both apply themselves from inside
/// `card_cost`/`play_card` by reading `hero_power` directly, so this only
/// needs to keep `legal_actions` from offering a button that does nothing.
fn is_passive_hero_power(card: CardId) -> bool {
    matches!(card.name(), "Mug's Magic" | "Zee's Might")
}

/// The basic hero power for a class.
pub fn hero_power_for(class: Class) -> Result<CardId, &'static str> {
    let name = match class {
        Class::Mage => "Fireblast",
        Class::Hunter => "Steady Shot",
        Class::Warlock => "Life Tap",
        Class::Priest => "Lesser Heal",
        Class::Warrior => "Armor Up!",
        Class::Paladin => "Reinforce",
        Class::Shaman => "Totemic Call",
        Class::Rogue => "Dagger Mastery",
        Class::Druid => "Shapeshift",
        Class::DemonHunter => "Demon Claws",
        Class::DeathKnight => "Ghoul Charge",
        Class::Neutral | Class::Dream | Class::Whizbang => {
            return Err("no hero power for this class");
        }
    };
    by_name(name).ok_or("hero power missing from the card table")
}

/// Shuffle a deck list in place with a caller-supplied generator. Exposed so a
/// batch runner can build decks reproducibly outside a game.
pub fn shuffle_deck(rng: &mut Rand, deck: &mut [CardId]) {
    rng.shuffle(deck);
}

/// True when `card` may legally sit in a `class` deck.
pub fn deck_legal(card: CardId, class: Class) -> bool {
    let d = card.def();
    d.collectible && d.deckable() && d.playable_by(class)
}

/// The most a board can hold, re-exported for callers building fixtures.
pub const BOARD_LIMIT: usize = MAX_BOARD;
