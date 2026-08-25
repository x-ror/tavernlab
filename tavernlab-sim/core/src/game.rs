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
    Flags, Game, HandCard, MAX_BOARD, MAX_HAND, MAX_MANA, Outcome, Pending, PendingKind, Permanent,
    Player, Side, TURN_LIMIT, Target, Weapon,
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
    #[default]
    EndTurn,
}

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

        // The Coin goes to whoever is on the draw.
        if let Some(coin) = by_name("The Coin") {
            let p = self.player_mut(first.other());
            p.hand.push(HandCard::new(coin));
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
        p.hero_attacks_done = 0;
        p.hero_bonus_atk = 0;
        p.cards_played_turn = 0;
        p.spells_cast_turn = 0;
        p.schools_cast_turn = 0;
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
        self.draw(side, 1);
        self.board_dirty = true;
        for entry in fired.iter().copied() {
            match entry.kind {
                PendingKind::TempCrystal => self.gain_temp_mana(side, entry.amount),
                PendingKind::SummonToken => {
                    self.summon(side, entry.card);
                }
                PendingKind::HeroDamage => self.damage_hero(side, entry.amount),
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
        self.sweep_deaths();
    }

    /// Play one full game against two agents, returning how it ended.
    pub fn run(&mut self, first: Side, agents: &mut [&mut dyn Agent; 2]) -> Outcome {
        self.start(first, agents);
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
            let needs_board = matches!(d.kind(), Kind::Minion | Kind::Location);
            if needs_board && me.board.is_full() {
                continue;
            }
            let beh = behaviour_of(c.card);
            match d.kind() {
                Kind::Minion | Kind::Weapon | Kind::Location => {}
                // An unimplemented spell would cost mana and do nothing, which
                // is worse than not being offered at all. A secret counts as
                // implemented through its own hook rather than a cast effect.
                Kind::Spell
                    if beh.is_some_and(|b| {
                        b.spell.is_some() || b.secret.is_some() || b.choose.is_some()
                    }) => {}
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
        if me.hero_power_uses == 0 && me.mana >= me.hero_power.def().cost {
            match hero_power_target(me.hero_power) {
                HpTarget::None => {
                    out.push(Action::HeroPower { target: None });
                }
                HpTarget::Any => {
                    for t in self.targetable(true) {
                        out.push(Action::HeroPower { target: Some(t) });
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
            Action::HeroPower { target } => self.use_hero_power(target),
            Action::UseLocation { slot, target } => self.use_location(slot as usize, target),
            Action::Prepare { hand } => self.prepare_card(hand as usize),
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
                    b.spell.is_some() || b.secret.is_some() || b.choose.is_some()
                }) =>
            {
                return false;
            }
            Kind::Hero | Kind::HeroPower => return false,
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
        }
        if def.overload > 0 {
            p.overload_next += def.overload as i16;
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

        let mut slot = None;
        let mut broken_weapon = None;
        match def.kind() {
            Kind::Minion | Kind::Location => {
                let m = Permanent::summon(hc.card);
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
            Kind::Spell => {}
            _ => unreachable!("filtered above"),
        }
        self.board_dirty = true;
        if let Some(old) = broken_weapon {
            self.fire_weapon_deathrattle(side, old);
        }

        if let Some(mode) = chosen {
            let ctx = Ctx {
                card: hc.card,
                side,
                target,
                source: slot,
                outcast,
                dying: None,
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
                }
            } else {
                (mode.effect)(self, &ctx);
            }
        } else if let Some(b) = beh {
            let ctx = Ctx {
                card: hc.card,
                side,
                target,
                source: slot,
                outcast,
                dying: None,
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
                }
            } else if let Some(f) = b.battlecry {
                f(self, &ctx);
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

    /// Put a card in hand, or burn it if the hand is full.
    pub fn give_card(&mut self, side: Side, card: CardId) -> bool {
        let p = self.player_mut(side);
        if p.hand.len() >= MAX_HAND {
            return false; // overdraw burns the card
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

        self.board_dirty = true;
        self.sweep_deaths();
        // Fired once the exchange has fully resolved: this is what "after
        // your hero attacks" listens to, and it must not see a board that
        // still holds the bodies.
        self.fire(Event::AfterAttack {
            attacker: attacker_t,
            defender: target,
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

    // -------------------------------------------------- damage and healing

    /// Deal `amount` damage. Returns whether damage actually landed, which is
    /// what Poisonous and Lifesteal key off — a Divine Shield pop is not a hit.
    pub fn deal_damage(&mut self, target: Target, amount: i16) -> bool {
        if amount <= 0 {
            return false;
        }
        match target {
            Target::Hero(s) => {
                self.damage_hero(s, amount);
                true
            }
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

    /// Damage a hero, spending armor first.
    pub fn damage_hero(&mut self, side: Side, amount: i16) {
        if amount <= 0 {
            return;
        }
        let p = self.player_mut(side);
        let absorbed = amount.min(p.armor);
        p.armor -= absorbed;
        p.hero_hp -= amount - absorbed;
        self.check_over();
        self.fire(Event::Damaged {
            target: Target::Hero(side),
            amount,
        });
    }

    pub fn heal_hero(&mut self, side: Side, amount: i16) {
        let p = self.player_mut(side);
        let before = p.hero_hp;
        p.hero_hp = (p.hero_hp + amount).min(crate::state::START_HP);
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
                        },
                    );
                }
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
                },
            );
        }
    }

    fn check_over(&mut self) {
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

    fn use_hero_power(&mut self, target: Option<Target>) -> bool {
        let side = self.current;
        let me = self.player(side);
        let hp = me.hero_power;
        let cost = hp.def().cost;
        if me.hero_power_uses > 0 || me.mana < cost {
            return false;
        }
        if hero_power_target(hp) == HpTarget::Any && target.is_none() {
            return false;
        }
        let p = self.player_mut(side);
        p.mana -= cost;
        p.hero_power_uses += 1;

        let foe = side.other();
        match hp.info().name {
            "Fireblast" => {
                if let Some(t) = target {
                    self.deal_damage(t, 1);
                }
            }
            "Steady Shot" => self.damage_hero(foe, 2),
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
            "Ghoul Charge" => {
                if let Some(c) = by_name("Frail Ghoul") {
                    self.summon(side, c);
                }
                self.damage_hero(side, 1);
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
