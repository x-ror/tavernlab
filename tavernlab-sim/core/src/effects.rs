//! The verbs card behaviour is written in.
//!
//! Six thousand cards only becomes tractable if the median card is one line, so
//! the work is in getting this vocabulary right rather than in any individual
//! card. Everything here is a method on [`Game`] that a card effect can call:
//! deal damage, draw, summon, buff, silence, destroy. A card that needs
//! something not expressible here is a signal that a verb is missing, not a
//! reason to reach into the state directly.
//!
//! Spell Damage is applied *here*, once, rather than at each call site — that
//! is exactly the rule everyone forgets on the twentieth damage card.

use crate::cards::{CardId, Keywords, Kind};
use crate::inline::Inline;
use crate::state::{DeckCard, Flags, Game, MAX_BOARD, Side, Target};

/// Which characters an area effect covers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Area {
    /// Every minion on both boards.
    AllMinions,
    EnemyMinions,
    FriendlyMinions,
    /// Enemy minions and the enemy hero.
    AllEnemies,
    /// Everything, both heroes included.
    Everything,
}

impl Game {
    // ------------------------------------------------------------- damage

    /// Atiesh the Greatstaff: doubles a spell's own damage, applied before
    /// Spell Power stacks on top of it -- a card that deals more and a card
    /// that deals harder are different bonuses, and the corpus text puts
    /// Atiesh's before the caster's Spell Damage in effect the same way a
    /// multiplier applies before an addition.
    ///
    /// Healing is not doubled: unlike damage, this engine's `heal`/`heal_hero`
    /// have no spell-specific wrapper the way `spell_damage` does for combat
    /// damage, so nothing marks a call here as coming from a spell rather
    /// than a battlecry or Lifesteal. Building that distinction for one
    /// card's other half was not worth the blast radius across every
    /// existing healing effect. See APPROXIMATE.
    fn wielding_atiesh(&self, side: Side) -> bool {
        self.player(side)
            .weapon
            .is_some_and(|w| w.card.name() == "Atiesh the Greatstaff")
    }

    /// Damage from a spell or hero power, boosted by the caster's Spell Damage.
    ///
    /// Every spell that deals damage goes through this. `deal_damage` is the
    /// raw form and does not apply the bonus, because combat damage must not.
    pub fn spell_damage(&mut self, side: Side, target: Option<Target>, base: i16) -> bool {
        let Some(t) = target else { return false };
        let base = if self.wielding_atiesh(side) { base * 2 } else { base };
        let amount = base + self.player(side).spell_power();
        self.deal_damage(t, amount)
    }

    /// Spell damage spread over an area.
    pub fn spell_damage_area(&mut self, side: Side, area: Area, base: i16) {
        let base = if self.wielding_atiesh(side) { base * 2 } else { base };
        let amount = base + self.player(side).spell_power();
        self.damage_area(side, area, amount);
    }

    /// Damage every character in `area`.
    ///
    /// Targets are collected before any damage lands, so a minion that dies
    /// part-way through still contributes to who was hit — the alternative
    /// silently skips minions when the board shifts underneath the loop.
    pub fn damage_area(&mut self, side: Side, area: Area, amount: i16) {
        if amount <= 0 {
            return;
        }
        let mut hits: Inline<Target, { MAX_BOARD * 2 + 2 }> = Inline::new();
        self.collect_area(side, area, &mut hits);
        for t in hits.iter() {
            self.deal_damage(*t, amount);
        }
    }

    /// The characters an area covers, in board order.
    pub fn collect_area<const N: usize>(
        &self,
        side: Side,
        area: Area,
        out: &mut Inline<Target, N>,
    ) {
        let foe = side.other();
        let push_board = |out: &mut Inline<Target, N>, s: Side| {
            for (i, m) in self.player(s).board.iter().enumerate() {
                if m.active() && m.is_minion() {
                    out.push(Target::Minion(s, i as u8));
                }
            }
        };
        match area {
            Area::AllMinions => {
                push_board(out, side);
                push_board(out, foe);
            }
            Area::EnemyMinions => push_board(out, foe),
            Area::FriendlyMinions => push_board(out, side),
            Area::AllEnemies => {
                push_board(out, foe);
                out.push(Target::Hero(foe));
            }
            Area::Everything => {
                push_board(out, side);
                push_board(out, foe);
                out.push(Target::Hero(side));
                out.push(Target::Hero(foe));
            }
        }
    }

    /// Damage split one point at a time among random characters in `area`.
    ///
    /// Re-picks after every point, so a target that dies stops absorbing —
    /// which is how the real thing behaves and why this cannot be written as
    /// "pick n targets, then hit them".
    pub fn damage_split(&mut self, side: Side, area: Area, base: i16) {
        let base = if self.wielding_atiesh(side) { base * 2 } else { base };
        let total = base + self.player(side).spell_power();
        for _ in 0..total {
            let mut pool: Inline<Target, { MAX_BOARD * 2 + 2 }> = Inline::new();
            self.collect_area(side, area, &mut pool);
            if pool.is_empty() {
                return;
            }
            let pick = self.rngs.effects.index(pool.len());
            self.deal_damage(pool[pick], 1);
            // Split damage is the one effect that must see the board update
            // between points: a target that died stops absorbing.
            self.sweep_deaths();
            if self.is_over() {
                return;
            }
        }
    }

    // -------------------------------------------------------------- cards

    /// Draw `n` cards for the caster.
    pub fn draw_cards(&mut self, side: Side, n: usize) {
        self.draw(side, n);
    }

    /// Put a token in hand by card id, if the hand has room.
    pub fn give_token(&mut self, side: Side, card: CardId) -> bool {
        self.give_card(side, card)
    }

    // ------------------------------------------------------------ summons

    /// Summon `n` copies of a token. Stops when the board fills.
    ///
    /// Takes a [`CardId`] rather than a string id so the card is resolved
    /// where it is named: [`cards::token`](crate::cards::token) is a `const
    /// fn`, and an id that no longer exists fails the build instead of
    /// summoning nothing at runtime.
    pub fn summon_token(&mut self, side: Side, card: CardId, n: usize) -> usize {
        let mut made = 0;
        for _ in 0..n {
            if !self.summon(side, card) {
                break;
            }
            made += 1;
        }
        made
    }

    /// Summon `n` of the token `card` creates.
    ///
    /// The card's own `childIds` say what it makes, so "Battlecry: Summon a
    /// 1/1 Murloc Scout" is a lookup rather than a token id written into the
    /// row -- and a token renamed upstream keeps working, because the link is
    /// data rather than a string. A card lists its upgraded printings among
    /// its children too, so only the minions are considered.
    pub fn summon_child(&mut self, side: Side, card: CardId, n: usize) -> usize {
        match card.summonable_children().next() {
            Some(token) => self.summon_token(side, token, n),
            None => 0,
        }
    }

    /// Summon one of `card`'s minions at random -- Animal Companion's three
    /// Beasts, and every card printed like it.
    pub fn summon_random_child(&mut self, side: Side, card: CardId) -> bool {
        let n = card.summonable_children().count();
        if n == 0 {
            return false;
        }
        let pick = self.rngs.effects.index(n);
        match card.summonable_children().nth(pick) {
            Some(token) => self.summon(side, token),
            None => false,
        }
    }

    /// Summon a copy of an existing minion, keeping only its printed stats.
    pub fn summon_copy(&mut self, side: Side, card: CardId) -> bool {
        self.summon(side, card)
    }

    // -------------------------------------------------------------- buffs

    /// Permanently add attack and health.
    pub fn buff(&mut self, target: Target, atk: i16, hp: i16) {
        if let Target::Minion(s, i) = target
            && let Some(m) = self.player_mut(s).board.get_mut(i as usize)
        {
            m.atk += atk;
            m.max_hp += hp;
        }
    }

    /// Grant keywords to a minion.
    pub fn grant(&mut self, target: Target, kw: Keywords) {
        if let Target::Minion(s, i) = target
            && let Some(m) = self.player_mut(s).board.get_mut(i as usize)
        {
            m.keywords.insert(kw);
        }
    }

    /// The eight keywords a "Bonus Effect" can be.
    ///
    /// The corpus does not carry this pool: it holds no enchantment cards at
    /// all -- only heroes, hero powers, minions, spells, locations and
    /// weapons -- so no card text names what a Bonus Effect draws from. The
    /// list was supplied by the player who reported the gap, and every one of
    /// the eight is a keyword the engine already models, so nothing here is
    /// an invented effect: only the membership of the pool comes from
    /// outside the card data.
    pub const BONUS_EFFECTS: [Keywords; 8] = [
        Keywords::TAUNT,
        Keywords::WINDFURY,
        Keywords::DIVINE_SHIELD,
        Keywords::POISONOUS,
        Keywords::ELUSIVE,
        Keywords::RUSH,
        Keywords::LIFESTEAL,
        Keywords::REBORN,
    ];

    /// Give a minion one random Bonus Effect it does not already have.
    ///
    /// Drawn from the ones it lacks rather than from all eight, so "two
    /// random Bonus Effects" really is two: a bitset cannot hold the same
    /// keyword twice, and rolling a duplicate would quietly halve the card.
    /// Returns false when the minion already has all eight, or is not there.
    pub fn give_bonus_effect(&mut self, target: Target) -> bool {
        let Target::Minion(s, i) = target else {
            return false;
        };
        let Some(m) = self.player(s).board.get(i as usize) else {
            return false;
        };
        let mut missing: Inline<u8, 8> = Inline::new();
        for (k, kw) in Self::BONUS_EFFECTS.iter().enumerate() {
            if !m.has(*kw) {
                missing.push(k as u8);
            }
        }
        if missing.is_empty() {
            return false;
        }
        let pick = self.rngs.effects.index(missing.len());
        self.grant(target, Self::BONUS_EFFECTS[missing[pick] as usize]);
        true
    }

    /// Give a minion `n` distinct random Bonus Effects.
    pub fn give_bonus_effects(&mut self, target: Target, n: usize) -> usize {
        (0..n).take_while(|_| self.give_bonus_effect(target)).count()
    }

    /// Which Bonus Effects a minion is carrying -- the ones granted to it,
    /// not the ones its own card prints.
    ///
    /// A body that prints Taunt has not been "given" Taunt, so stealing its
    /// Bonus Effects must not take it (Violet Punisher). The difference is
    /// readable straight off the card: whatever it has beyond what it was
    /// printed with was granted.
    pub fn bonus_effects_on(&self, target: Target) -> Keywords {
        let Target::Minion(s, i) = target else {
            return Keywords::NONE;
        };
        let Some(m) = self.player(s).board.get(i as usize) else {
            return Keywords::NONE;
        };
        let printed = m.card.def().keywords;
        let mut out = Keywords::NONE;
        for kw in Self::BONUS_EFFECTS {
            if m.has(kw) && !printed.has(kw) {
                out.insert(kw);
            }
        }
        out
    }

    /// Buff every minion in an area.
    pub fn buff_area(&mut self, side: Side, area: Area, atk: i16, hp: i16) {
        let mut hits: Inline<Target, { MAX_BOARD * 2 + 2 }> = Inline::new();
        self.collect_area(side, area, &mut hits);
        for t in hits.iter() {
            self.buff(*t, atk, hp);
        }
    }

    // ------------------------------------------------------------ removal

    /// Destroy a minion outright, ignoring its health.
    pub fn destroy(&mut self, target: Target) {
        if let Target::Minion(s, i) = target
            && let Some(m) = self.player_mut(s).board.get_mut(i as usize)
            && !m.has(Keywords::CANT_BE_DESTROYED)
        {
            m.flags.insert(Flags::PENDING_DESTROY);
        }
    }

    /// Silence a minion.
    pub fn silence(&mut self, target: Target) {
        if let Target::Minion(s, i) = target
            && let Some(m) = self.player_mut(s).board.get_mut(i as usize)
        {
            m.silence();
        }
    }

    /// Return a minion to its owner's hand, or burn it if the hand is full.
    pub fn bounce(&mut self, target: Target) {
        if let Target::Minion(s, i) = target
            && let Some(m) = self.player(s).board.get(i as usize).copied()
        {
            self.player_mut(s).board.remove(i as usize);
            let mut hc = crate::state::HandCard::new(m.card);
            if m.flags.has(Flags::NOT_FROM_DECK) {
                hc.marks.insert(crate::state::Marks::NOT_FROM_DECK);
            }
            self.give_hand_card(s, hc);
            self.board_dirty = true;
        }
    }

    // -------------------------------------------------------------- other

    /// Give the caster temporary mana this turn only (The Coin, Innervate).
    pub fn gain_temp_mana(&mut self, side: Side, n: i16) {
        self.player_mut(side).mana += n;
    }

    /// Permanently gain an empty mana crystal, up to the cap.
    pub fn gain_crystal(&mut self, side: Side, n: i16) {
        let p = self.player_mut(side);
        p.crystals = (p.crystals + n).min(crate::state::MAX_MANA);
    }

    /// Equip a weapon by card id, replacing whatever is held. A replaced
    /// weapon breaks, so its deathrattle fires first.
    pub fn equip(&mut self, side: Side, card: CardId) {
        if card.def().kind() == Kind::Weapon {
            let old = self
                .player_mut(side)
                .weapon
                .replace(crate::state::Weapon::equip(card));
            if let Some(w) = old {
                self.fire_weapon_deathrattle(side, w);
            }
        }
    }

    /// Give the hero attack for this turn only.
    pub fn hero_attack_bonus(&mut self, side: Side, n: i16) {
        self.player_mut(side).hero_bonus_atk += n;
    }

    /// Add corpses, the Death Knight resource.
    pub fn gain_corpses(&mut self, side: Side, n: i16) {
        self.player_mut(side).corpses += n;
    }

    /// How many minions the side has in play. Used by conditional cards.
    pub fn minion_count(&self, side: Side) -> usize {
        self.player(side).minions().count()
    }

    /// A random live minion belonging to `side`, if any.
    pub fn random_minion(&mut self, side: Side) -> Option<Target> {
        let mut pool: Inline<Target, MAX_BOARD> = Inline::new();
        for (i, m) in self.player(side).board.iter().enumerate() {
            if m.active() && m.is_minion() {
                pool.push(Target::Minion(side, i as u8));
            }
        }
        if pool.is_empty() {
            return None;
        }
        let pick = self.rngs.effects.index(pool.len());
        Some(pool[pick])
    }

    /// Attack for this turn only.
    pub fn buff_temp_atk(&mut self, target: Target, n: i16) {
        match target {
            Target::Hero(s) => self.player_mut(s).hero_bonus_atk += n,
            Target::Minion(s, i) => {
                if let Some(m) = self.player_mut(s).board.get_mut(i as usize) {
                    m.atk += n;
                    m.temp_atk += n;
                }
            }
        }
    }

    /// Temporary attack for a whole area — Savage Roar, Bloodlust.
    pub fn buff_temp_area(&mut self, side: Side, area: Area, n: i16) {
        let mut hits: Inline<Target, { MAX_BOARD * 2 + 2 }> = Inline::new();
        self.collect_area(side, area, &mut hits);
        for t in hits.iter() {
            self.buff_temp_atk(*t, n);
        }
    }

    /// Set a minion's attack outright, as Humility does. The change is
    /// permanent, so it is not recorded as a temporary buff.
    pub fn set_attack(&mut self, target: Target, n: i16) {
        if let Target::Minion(s, i) = target
            && let Some(m) = self.player_mut(s).board.get_mut(i as usize)
        {
            m.atk = n;
            m.temp_atk = 0;
        }
    }

    /// Set a minion's Health outright, as Equality does.
    ///
    /// Sets the maximum and clears damage: "change the Health to 1" leaves a
    /// 1/1 at full health, not a damaged minion that a heal could rescue.
    pub fn set_health(&mut self, target: Target, n: i16) {
        if let Target::Minion(s, i) = target
            && let Some(m) = self.player_mut(s).board.get_mut(i as usize)
        {
            m.max_hp = n;
            m.damage = 0;
        }
    }

    /// Replace a minion with a different card in the same slot, as Polymorph
    /// and Hex do. Buffs, damage and keywords are all discarded — the new
    /// minion is freshly summoned, and counts as such for summoning sickness.
    pub fn transform(&mut self, target: Target, card: CardId) {
        if let Target::Minion(s, i) = target
            && (i as usize) < self.player(s).board.len()
        {
            self.player_mut(s).board[i as usize] = crate::state::Permanent::summon(card);
            self.board_dirty = true;
        }
    }

    /// Heal a minion to its full health.
    pub fn restore_full(&mut self, target: Target) {
        match target {
            Target::Hero(s) => self.player_mut(s).hero_hp = crate::state::START_HP,
            Target::Minion(s, i) => {
                if let Some(m) = self.player_mut(s).board.get_mut(i as usize) {
                    m.damage = 0;
                }
            }
        }
    }

    /// Destroy a player's weapon, firing its deathrattle first.
    pub fn destroy_weapon(&mut self, side: Side) {
        if let Some(w) = self.player_mut(side).weapon.take() {
            self.fire_weapon_deathrattle(side, w);
        }
    }

    /// Add attack to the held weapon, if there is one.
    pub fn buff_weapon(&mut self, side: Side, atk: i16, durability: i16) {
        if let Some(w) = self.player_mut(side).weapon.as_mut() {
            w.atk += atk;
            w.durability += durability;
        }
    }

    /// Freeze every character in an area.
    pub fn freeze_area(&mut self, side: Side, area: Area) {
        let mut hits: Inline<Target, { MAX_BOARD * 2 + 2 }> = Inline::new();
        self.collect_area(side, area, &mut hits);
        for t in hits.iter() {
            self.freeze(*t);
        }
    }

    /// Damage `n` distinct random enemy minions — Multi-Shot, Cleave.
    ///
    /// Distinct, and chosen before any damage lands, which is what makes this
    /// different from calling `damage_split`.
    pub fn damage_random_enemy_minions(&mut self, side: Side, n: usize, amount: i16) {
        let foe = side.other();
        let live: Inline<u8, MAX_BOARD> = self
            .player(foe)
            .board
            .iter()
            .enumerate()
            .filter(|(_, m)| m.active() && m.is_minion())
            .map(|(i, _)| i as u8)
            .collect();
        if live.is_empty() {
            return;
        }
        let mut picks = [0u32; MAX_BOARD];
        let taken = self
            .rngs
            .effects
            .sample_indices(live.len(), &mut picks[..n.min(live.len())]);
        let amount = if self.wielding_atiesh(side) { amount * 2 } else { amount };
        let boosted = amount + self.player(side).spell_power();
        for &p in picks.iter().take(taken) {
            self.deal_damage(Target::Minion(foe, live[p as usize]), boosted);
        }
    }

    /// Re-apply every continuous aura from scratch.
    ///
    /// Idempotent by construction: each minion's current aura contribution is
    /// taken back before the fresh one is added, so running this twice changes
    /// nothing and running it late can only leave stats stale rather than
    /// compounding an error. Called wherever the board changes.
    ///
    /// Cost is at most fourteen minions against fourteen possible sources —
    /// under two hundred pure comparisons, against a game that takes tens of
    /// microseconds. A dirty-flag scheme would be bookkeeping to avoid work
    /// that does not measurably exist.
    pub fn recompute_auras(&mut self) {
        // 1. take back what auras previously granted
        for i in 0..2 {
            for m in self.players[i].board.iter_mut() {
                m.atk -= m.aura_atk;
                m.max_hp -= m.aura_hp;
                m.aura_atk = 0;
                m.aura_hp = 0;
            }
        }
        // 2. what each minion grants itself ("Has +3 Attack while damaged").
        // Every bonus is read before any is applied, so one minion's grant
        // cannot be an input to another's -- see `Bonus` on why that matters.
        // The count of live sources is kept on the game so the damage path can
        // skip this whole pass with a single comparison.
        let mut grants: Inline<(usize, usize, i16, i16), { MAX_BOARD * 2 }> = Inline::new();
        let mut conditional = 0u8;
        for i in 0..2 {
            let side = Side::from_index(i);
            for slot in 0..self.players[i].board.len() {
                let m = self.players[i].board[slot];
                if !m.active() || m.flags.has(Flags::SILENCED) {
                    continue;
                }
                let Some(f) = crate::cards::behaviour_of(m.card).and_then(|b| b.bonus) else {
                    continue;
                };
                conditional += 1;
                let (a, h) = f(self, side, slot as u8, &m);
                if a != 0 || h != 0 {
                    grants.push((i, slot, a, h));
                }
            }
        }
        self.conditional = conditional;
        for (i, slot, a, h) in grants.iter().copied() {
            let m = &mut self.players[i].board[slot];
            m.atk += a;
            m.max_hp += h;
            m.aura_atk += a;
            m.aura_hp += h;
        }

        // 3. collect live aura sources; a silenced minion projects nothing
        // Stored as the source card rather than its function: a bare `fn`
        // pointer has no `Default`, and re-reading it is one array index.
        let mut sources: Inline<(Side, u8, CardId), { MAX_BOARD * 2 }> = Inline::new();
        for i in 0..2 {
            let side = Side::from_index(i);
            for (slot, m) in self.players[i].board.iter().enumerate() {
                if !m.active() || m.flags.has(Flags::SILENCED) {
                    continue;
                }
                if crate::cards::behaviour_of(m.card).and_then(|b| b.aura).is_some() {
                    sources.push((side, slot as u8, m.card));
                }
            }
        }
        if sources.is_empty() {
            return;
        }
        // 4. apply
        for (src_side, src_slot, src_card) in sources.iter().copied() {
            let Some(f) = crate::cards::behaviour_of(src_card).and_then(|b| b.aura) else {
                continue;
            };
            for i in 0..2 {
                let side = Side::from_index(i);
                for slot in 0..self.players[i].board.len() {
                    let m = self.players[i].board[slot];
                    if !m.active() {
                        continue;
                    }
                    let (a, h) = f(src_side, src_slot, side, slot as u8, &m);
                    if a == 0 && h == 0 {
                        continue;
                    }
                    let m = &mut self.players[i].board[slot];
                    m.atk += a;
                    m.max_hp += h;
                    m.aura_atk += a;
                    m.aura_hp += h;
                }
            }
        }
    }

    /// Draw the first card in the deck matching `pred`, chosen at random among
    /// the matches. Returns whether anything was drawn.
    ///
    /// "Draw a Beast" with no Beast left draws nothing at all — it does not
    /// fall back to a normal draw.
    pub fn draw_matching(&mut self, side: Side, pred: fn(&crate::cards::CardDef) -> bool) -> bool {
        let mut matches: Inline<u16, { crate::state::MAX_DECK }> = Inline::new();
        for (i, c) in self.player(side).deck.iter().enumerate() {
            if pred(c.def()) {
                matches.push(i as u16);
            }
        }
        if matches.is_empty() {
            return false;
        }
        let pick = self.rngs.effects.index(matches.len());
        let at = matches[pick] as usize;
        let card = self.player(side).deck[at];
        self.player_mut(side).deck.remove(at);
        self.give_hand_card(side, card.to_hand());
        self.fire(crate::events::Event::CardDrawn { side });
        true
    }

    /// Discard a random card from hand. Returns whether there was one.
    pub fn discard_random(&mut self, side: Side) -> bool {
        let n = self.player(side).hand.len();
        if n == 0 {
            return false;
        }
        let pick = self.rngs.effects.index(n);
        self.player_mut(side).hand.remove(pick);
        true
    }

    /// Offer three cards from a filtered pool and take one.
    ///
    /// **Simplification, deliberate and visible.** The real game lets the
    /// player choose; here the engine picks the highest-cost option it could
    /// plausibly play, resolved from the effect RNG. That makes Discover cards
    /// playable at slightly below optimal strength rather than unplayable, and
    /// it stays deterministic for a given seed. Wiring the choice back to the
    /// policy needs an agent hook inside effect resolution, which is a change
    /// to how effects are called rather than a change here.
    pub fn discover(&mut self, side: Side, pred: fn(&crate::cards::CardDef) -> bool) -> bool {
        self.player_mut(side).discovered_turn = true;
        let pool = crate::cards::discover_pool(pred);
        if pool.is_empty() {
            return false;
        }
        let mut offered = [0u32; 3];
        let n = self.rngs.effects.sample_indices(pool.len(), &mut offered);
        let crystals = self.player(side).crystals;
        let best = offered[..n]
            .iter()
            .map(|&i| pool[i as usize])
            .max_by_key(|c| {
                let d = c.def();
                // Prefer something castable soon, then the biggest of those.
                (d.cost <= crystals + 1, d.cost)
            });
        match best {
            Some(c) => self.give_card(side, c),
            None => false,
        }
    }

    /// Discover a card and put it on the bottom of the deck instead of into
    /// hand, with `atk`/`hp` already written on it (Kaldorei Cultivator).
    ///
    /// The offer is made the same way [`Game::discover`] makes it -- three
    /// candidates, the best castable one taken -- because the pick is the
    /// same decision wherever the card ends up.
    pub fn discover_to_deck_bottom(
        &mut self,
        side: Side,
        pred: fn(&crate::cards::CardDef) -> bool,
        atk: i16,
        hp: i16,
    ) -> bool {
        self.player_mut(side).discovered_turn = true;
        let pool = crate::cards::discover_pool(pred);
        if pool.is_empty() {
            return false;
        }
        let mut offered = [0u32; 3];
        let n = self.rngs.effects.sample_indices(pool.len(), &mut offered);
        let crystals = self.player(side).crystals;
        let best = offered[..n]
            .iter()
            .map(|&i| pool[i as usize])
            .max_by_key(|c| {
                let d = c.def();
                (d.cost <= crystals + 1, d.cost)
            });
        let Some(card) = best else { return false };
        let mut dc = DeckCard::new(card);
        dc.enchant(atk, hp);
        self.put_deck_card_on_bottom(side, dc)
    }

    // ---------------------------------------------------- deck enchanting

    /// Give `atk`/`hp` to every card in the deck for which `pred` holds.
    /// Returns how many were buffed.
    ///
    /// The stats sit on the deck card and fold into the body when it is
    /// drawn and played, or straight onto the board if something summons it
    /// out of the deck.
    pub fn enchant_deck_where(
        &mut self,
        side: Side,
        pred: impl Fn(&crate::cards::CardDef) -> bool,
        atk: i16,
        hp: i16,
    ) -> usize {
        let mut n = 0;
        for dc in self.player_mut(side).deck.iter_mut() {
            if pred(dc.card.def()) {
                dc.enchant(atk, hp);
                n += 1;
            }
        }
        n
    }

    /// Give `atk`/`hp` to the topmost `n` cards matching `pred`.
    ///
    /// The top of the deck is the end of the array -- the end [`Game::draw`]
    /// pops from -- so this walks backwards. Returns how many were buffed,
    /// which is fewer than `n` when the deck runs out of matches.
    pub fn enchant_deck_top(
        &mut self,
        side: Side,
        n: usize,
        pred: impl Fn(&crate::cards::CardDef) -> bool,
        atk: i16,
        hp: i16,
    ) -> usize {
        let mut done = 0;
        for dc in self.player_mut(side).deck.iter_mut().rev() {
            if done == n {
                break;
            }
            if pred(dc.card.def()) {
                dc.enchant(atk, hp);
                done += 1;
            }
        }
        done
    }

    /// Give `atk`/`hp` to every card in hand for which `pred` holds.
    pub fn enchant_hand_where(
        &mut self,
        side: Side,
        pred: impl Fn(&crate::cards::CardDef) -> bool,
        atk: i16,
        hp: i16,
    ) -> usize {
        let mut n = 0;
        for hc in self.player_mut(side).hand.iter_mut() {
            if pred(hc.card.def()) {
                hc.enchant(atk, hp);
                n += 1;
            }
        }
        n
    }

    /// Set what the bottom `n` cards of the deck cost, whatever they were
    /// printed at (Krona, Keeper of Eons). The bottom is index 0.
    pub fn set_deck_bottom_cost(&mut self, side: Side, n: usize, cost: i16) -> usize {
        let mut done = 0;
        for dc in self.player_mut(side).deck.iter_mut() {
            if done == n {
                break;
            }
            dc.set_cost(cost);
            done += 1;
        }
        done
    }

    /// Destroy every card in `side`'s deck that was not in the list the deck
    /// was built from (Steamcleaner). Returns how many went.
    pub fn destroy_shuffled_in(&mut self, side: Side) -> usize {
        let before = self.player(side).deck.len();
        self.player_mut(side).deck.retain(|dc| dc.started_here);
        before - self.player(side).deck.len()
    }

    /// Shuffle `n` random implemented cards matching `pred` into the deck,
    /// running `enchant` on each before it goes in.
    ///
    /// The hook is how "shuffle in five minions … double their stats" gets
    /// written without inventing a number: the doubling is read off each
    /// card's own printed body.
    pub fn shuffle_random_into_deck_where(
        &mut self,
        side: Side,
        n: usize,
        pred: fn(&crate::cards::CardDef) -> bool,
        enchant: fn(&mut DeckCard),
    ) -> usize {
        let pool = crate::cards::discover_pool(pred);
        if pool.is_empty() {
            return 0;
        }
        let mut done = 0;
        for _ in 0..n {
            let pick = self.rngs.effects.index(pool.len());
            let mut dc = DeckCard::new(pool[pick]);
            enchant(&mut dc);
            if !self.shuffle_deck_card(side, dc) {
                break;
            }
            done += 1;
        }
        done
    }

    /// Reduce what every card in hand matching `pred` costs, for that copy
    /// only. Returns how many were discounted.
    pub fn discount_hand_where(
        &mut self,
        side: Side,
        pred: impl Fn(&crate::state::HandCard) -> bool,
        by: i16,
    ) -> usize {
        let mut n = 0;
        for hc in self.player_mut(side).hand.iter_mut() {
            if pred(hc) {
                hc.cost_delta -= by;
                n += 1;
            }
        }
        n
    }

    /// Draw a card that either did or did not start in the deck, restricted
    /// to those matching `pred`.
    ///
    /// Like [`Game::draw_matching`], a miss draws nothing at all rather than
    /// falling back to a normal draw: "draw a spell that didn't start in
    /// your deck" with no such spell there draws none.
    pub fn draw_by_origin(
        &mut self,
        side: Side,
        started: bool,
        pred: impl Fn(&crate::cards::CardDef) -> bool,
    ) -> bool {
        let mut matches: Inline<u16, { crate::state::MAX_DECK }> = Inline::new();
        for (i, dc) in self.player(side).deck.iter().enumerate() {
            if dc.started_here == started && pred(dc.def()) {
                matches.push(i as u16);
            }
        }
        if matches.is_empty() {
            return false;
        }
        let pick = self.rngs.effects.index(matches.len());
        let at = matches[pick] as usize;
        let card = self.player(side).deck[at];
        self.player_mut(side).deck.remove(at);
        self.give_hand_card(side, card.to_hand());
        self.fire(crate::events::Event::CardDrawn { side });
        true
    }

    /// Refresh spent mana, up to the crystals owned.
    pub fn refresh_mana(&mut self, side: Side, n: i16) {
        let p = self.player_mut(side);
        p.mana = (p.mana + n).min(p.crystals);
    }

    /// Let the hero power be used again this turn.
    pub fn refresh_hero_power(&mut self, side: Side) {
        self.player_mut(side).hero_power_uses = 0;
    }

    /// Spell Damage the hero carries in its own right, on top of the board.
    pub fn give_spell_power(&mut self, side: Side, n: i16) {
        self.player_mut(side).spell_power_bonus += n;
    }

    /// Spells cast by this player so far this turn.
    #[inline]
    pub fn spells_cast_turn(&self, side: Side) -> i16 {
        self.player(side).spells_cast_turn as i16
    }

    /// Put `n` random Secrets of the player's class into play, skipping any
    /// already armed -- a Secret that duplicates one on the board does
    /// nothing in the real game, so rolling one is a wasted cast, not a
    /// second copy.
    pub fn cast_random_secrets(&mut self, side: Side, n: usize) -> usize {
        let class = self.player(side).class;
        let pool = crate::cards::discover_pool(move |d| {
            d.keywords.has(crate::cards::Keywords::SECRET) && d.class() == class
        });
        if pool.is_empty() {
            return 0;
        }
        let mut cast = 0;
        for _ in 0..n {
            if self.player(side).secrets.len() >= crate::state::MAX_SECRETS {
                break;
            }
            let mut choices: crate::inline::Inline<CardId, 32> = crate::inline::Inline::new();
            for c in &pool {
                if !self.player(side).secrets.iter().any(|s| s.name() == c.name()) {
                    choices.push(*c);
                }
            }
            if choices.is_empty() {
                break;
            }
            let pick = self.rngs.effects.index(choices.len());
            self.player_mut(side).secrets.push(choices[pick]);
            cast += 1;
        }
        cast
    }

    /// Whether the player is holding a card of a given tribe.
    pub fn holding_race(&self, side: Side, race: crate::cards::Races) -> bool {
        self.player(side)
            .hand
            .iter()
            .any(|h| h.card.def().races.any(race))
    }

    /// Return every minion in an area to its owner's hand.
    pub fn bounce_area(&mut self, side: Side, area: Area) {
        let mut hits: Inline<Target, { MAX_BOARD * 2 + 2 }> = Inline::new();
        self.collect_area(side, area, &mut hits);
        // Highest slot first: bouncing shifts the ones after it.
        for t in hits.iter().rev() {
            self.bounce(*t);
        }
    }

    /// Destroy every minion in an area for which `pred` holds.
    pub fn destroy_area_where(
        &mut self,
        side: Side,
        area: Area,
        pred: fn(&crate::state::Permanent) -> bool,
    ) {
        let mut hits: Inline<Target, { MAX_BOARD * 2 + 2 }> = Inline::new();
        self.collect_area(side, area, &mut hits);
        for t in hits.iter() {
            if let Target::Minion(s, i) = *t
                && let Some(m) = self.player(s).board.get(i as usize)
                && pred(m)
            {
                self.destroy(*t);
            }
        }
    }

    /// The enemy character with the least health, hero included.
    pub fn lowest_health_enemy(&self, side: Side) -> Option<Target> {
        let foe = side.other();
        let mut best: Option<(i16, Target)> = Some((
            self.player(foe).hero_hp + self.player(foe).armor,
            Target::Hero(foe),
        ));
        for (i, m) in self.player(foe).board.iter().enumerate() {
            if !m.active() || !m.is_minion() {
                continue;
            }
            let h = m.health();
            if best.is_none_or(|(bh, _)| h < bh) {
                best = Some((h, Target::Minion(foe, i as u8)));
            }
        }
        best.map(|(_, t)| t)
    }

    /// Buff a random friendly minion of a tribe, other than `except`.
    pub fn buff_random_race(
        &mut self,
        side: Side,
        race: crate::cards::Races,
        except: Option<Target>,
        atk: i16,
        hp: i16,
    ) -> bool {
        let mut pool: Inline<Target, MAX_BOARD> = Inline::new();
        for (i, m) in self.player(side).board.iter().enumerate() {
            let t = Target::Minion(side, i as u8);
            if m.active() && m.is_minion() && m.races().any(race) && Some(t) != except {
                pool.push(t);
            }
        }
        if pool.is_empty() {
            return false;
        }
        let pick = self.rngs.effects.index(pool.len());
        self.buff(pool[pick], atk, hp);
        true
    }

    /// Summon a token that starts Dormant for `turns` of its controller.
    ///
    /// A dormant minion is on the board and takes a slot, but cannot be
    /// attacked, targeted, or attack — `Permanent::active` is the single place
    /// that decides so.
    pub fn summon_dormant(&mut self, side: Side, card: CardId, turns: u8) -> bool {
        let p = self.player_mut(side);
        if p.board.is_full() {
            return false;
        }
        let mut m = crate::state::Permanent::summon(card);
        m.dormant = turns;
        m.flags.insert(Flags::DORMANT);
        let ok = p.board.push(m);
        self.board_dirty = true;
        ok
    }

    /// Put an existing minion to sleep for `turns` of its controller.
    pub fn make_dormant(&mut self, target: Target, turns: u8) {
        if let Target::Minion(s, i) = target
            && let Some(m) = self.player_mut(s).board.get_mut(i as usize)
        {
            m.dormant = turns;
            m.flags.insert(Flags::DORMANT);
            self.board_dirty = true;
        }
    }

    /// How many cards the player is holding.
    pub fn hand_size(&self, side: Side) -> i16 {
        self.player(side).hand.len() as i16
    }

    /// Summon a random one of several token ids, Dormant for `turns`.
    ///
    /// Dreadseeds come in three flavours and the card picks one at random, so
    /// this takes the set rather than a single id.
    pub fn summon_random_dormant(&mut self, side: Side, cards: &[CardId], turns: u8) -> bool {
        if cards.is_empty() {
            return false;
        }
        let pick = self.rngs.effects.index(cards.len());
        self.summon_dormant(side, cards[pick], turns)
    }

    /// Spend corpses, if there are enough. Returns whether they were spent.
    pub fn spend_corpses(&mut self, side: Side, n: i16) -> bool {
        if self.player(side).corpses < n {
            return false;
        }
        self.player_mut(side).corpses -= n;
        true
    }

    /// Whether a minion of `race` was played on the controller's previous
    /// turn — the Kindred condition.
    pub fn kindred(&self, side: Side, race: crate::cards::Races) -> bool {
        self.player(side).played_races_last.any(race)
    }

    /// [`discover`](Self::discover) with a closure rather than a plain `fn`,
    /// for the cards whose pool depends on the position — "from another
    /// class" has to know which class you are.
    pub fn discover_where(&mut self, side: Side, pred: impl Fn(&crate::cards::CardDef) -> bool) -> bool {
        self.player_mut(side).discovered_turn = true;
        let pool = crate::cards::discover_pool(pred);
        if pool.is_empty() {
            return false;
        }
        let mut offered = [0u32; 3];
        let n = self.rngs.effects.sample_indices(pool.len(), &mut offered);
        let crystals = self.player(side).crystals;
        let best = offered[..n]
            .iter()
            .map(|&i| pool[i as usize])
            .max_by_key(|c| {
                let d = c.def();
                (d.cost <= crystals + 1, d.cost)
            });
        match best {
            Some(c) => self.give_card(side, c),
            None => false,
        }
    }

    /// Summon a minion straight out of your own deck, chosen at random among
    /// those matching `pred`. Returns whether one was found.
    pub fn summon_from_deck(
        &mut self,
        side: Side,
        pred: impl Fn(&crate::cards::CardDef) -> bool,
    ) -> bool {
        let mut matches: Inline<u16, { crate::state::MAX_DECK }> = Inline::new();
        for (i, c) in self.player(side).deck.iter().enumerate() {
            if c.def().kind() == crate::cards::Kind::Minion && pred(c.def()) {
                matches.push(i as u16);
            }
        }
        if matches.is_empty() {
            return false;
        }
        let pick = self.rngs.effects.index(matches.len());
        let at = matches[pick] as usize;
        let card = self.player(side).deck[at];
        self.player_mut(side).deck.remove(at);
        self.summon_with(side, card.card, card.atk as i16, card.hp as i16)
    }

    /// Discard a random card from hand matching `pred`. Returns whether there
    /// was one to discard.
    pub fn discard_random_where(
        &mut self,
        side: Side,
        pred: impl Fn(&crate::cards::CardDef) -> bool,
    ) -> bool {
        let mut matches: Inline<u16, { crate::state::MAX_HAND }> = Inline::new();
        for (i, h) in self.player(side).hand.iter().enumerate() {
            if pred(h.card.def()) {
                matches.push(i as u16);
            }
        }
        if matches.is_empty() {
            return false;
        }
        let pick = self.rngs.effects.index(matches.len());
        self.player_mut(side).hand.remove(matches[pick] as usize);
        true
    }

    /// Discover among the cards left in your own deck, moving the pick to hand.
    ///
    /// Distinct from [`Game::discover`], which offers from the whole card pool:
    /// this one can only find what you actually still have.
    pub fn discover_from_deck(
        &mut self,
        side: Side,
        pred: fn(&crate::cards::CardDef) -> bool,
    ) -> bool {
        let mut matches: Inline<u16, { crate::state::MAX_DECK }> = Inline::new();
        for (i, c) in self.player(side).deck.iter().enumerate() {
            if pred(c.def()) {
                matches.push(i as u16);
            }
        }
        if matches.is_empty() {
            return false;
        }
        // Same simplification as `discover`: three offered, the engine takes
        // the most expensive it could plausibly cast.
        let mut offered = [0u32; 3];
        let n = self
            .rngs
            .effects
            .sample_indices(matches.len(), &mut offered);
        let crystals = self.player(side).crystals;
        let best = offered[..n]
            .iter()
            .map(|&k| matches[k as usize] as usize)
            .max_by_key(|&at| {
                let d = self.player(side).deck[at].def();
                (d.cost <= crystals + 1, d.cost)
            });
        let Some(at) = best else { return false };
        let card = self.player(side).deck[at];
        self.player_mut(side).deck.remove(at);
        self.give_hand_card(side, card.to_hand())
    }

    /// Discover among the opponent's remaining deck, moving the pick to
    /// `side`'s hand. The opponent keeps their copy — this only reads what
    /// they still have left to draw, the same way `discover_from_deck` reads
    /// the caster's own.
    pub fn discover_from_opponent_deck(
        &mut self,
        side: Side,
        pred: fn(&crate::cards::CardDef) -> bool,
    ) -> bool {
        let foe = side.other();
        let mut matches: Inline<u16, { crate::state::MAX_DECK }> = Inline::new();
        for (i, c) in self.player(foe).deck.iter().enumerate() {
            if pred(c.def()) {
                matches.push(i as u16);
            }
        }
        if matches.is_empty() {
            return false;
        }
        let pick = self.rngs.effects.index(matches.len());
        let card = self.player(foe).deck[matches[pick] as usize].card;
        self.give_card(side, card)
    }

    /// Discover a copy of a card in the opponent's hand, moving the copy to
    /// `side`'s hand. The opponent keeps the original.
    pub fn discover_from_opponent_hand(&mut self, side: Side) -> bool {
        let foe = side.other();
        let n = self.player(foe).hand.len();
        if n == 0 {
            return false;
        }
        let pick = self.rngs.effects.index(n);
        let card = self.player(foe).hand[pick].card;
        self.give_card(side, card)
    }

    /// Add a random card matching `pred` to hand, straight from the pool.
    pub fn add_random_to_hand(
        &mut self,
        side: Side,
        pred: fn(&crate::cards::CardDef) -> bool,
    ) -> bool {
        let pool = crate::cards::discover_pool(pred);
        if pool.is_empty() {
            return false;
        }
        let pick = self.rngs.effects.index(pool.len());
        self.give_card(side, pool[pick])
    }

    /// [`add_random_to_hand`](Self::add_random_to_hand) with a closure, for
    /// pools that depend on the position ("from another class").
    pub fn add_random_to_hand_where(
        &mut self,
        side: Side,
        pred: impl Fn(&crate::cards::CardDef) -> bool,
    ) -> bool {
        let pool = crate::cards::discover_pool(pred);
        if pool.is_empty() {
            return false;
        }
        let pick = self.rngs.effects.index(pool.len());
        self.give_card(side, pool[pick])
    }

    /// Summon a copy of an existing minion, as printed.
    pub fn summon_copy_of(&mut self, side: Side, source: Target) -> bool {
        let Target::Minion(s, i) = source else {
            return false;
        };
        let Some(m) = self.player(s).board.get(i as usize).copied() else {
            return false;
        };
        self.summon(side, m.card)
    }

    /// Move a minion to `new_side`'s board, marking it so `Game::end_turn`
    /// returns it once that side's own turn ends (Cursed Chains). Fresh to
    /// this board the same way a summon is: it cannot attack this turn.
    ///
    /// Checked before anything moves, not after: leaving the permanent to
    /// vanish because the new board turned out to be full would be worse
    /// than the spell simply failing.
    pub fn take_control(&mut self, target: Target, new_side: Side) -> bool {
        let Target::Minion(old_side, slot) = target else {
            return false;
        };
        if old_side == new_side || self.player(new_side).board.is_full() {
            return false;
        }
        let Some(mut m) = self.player(old_side).board.get(slot as usize).copied() else {
            return false;
        };
        self.player_mut(old_side).board.remove(slot as usize);
        m.stolen_from = Some(old_side);
        m.flags.insert(Flags::JUST_SUMMONED);
        m.attacks_done = 0;
        self.player_mut(new_side).board.push(m);
        self.board_dirty = true;
        self.recompute_auras();
        true
    }

    /// The scale a Soldier summoned right now would carry: 1, 2 or 4.
    pub fn herald_scale(&self, side: Side) -> i16 {
        match self.player(side).herald {
            n if n >= 4 => 4,
            n if n >= 2 => 2,
            _ => 1,
        }
    }

    /// The scale of the Soldier that was summoned by the Herald in progress.
    ///
    /// Read from the count *before* it was incremented, because a Soldier's
    /// own effect resolves immediately after the increment.
    pub fn heralded_scale(&self, side: Side) -> i16 {
        match self.player(side).herald - 1 {
            n if n >= 4 => 4,
            n if n >= 2 => 2,
            _ => 1,
        }
    }

    /// Herald: summon your class's Soldier, scaled by how often you have
    /// Heralded before, and resolve its arrival effect.
    ///
    /// Classes with no Soldier still advance the counter — Deathwing's cost
    /// reduction keys off it regardless.
    pub fn herald(&mut self, side: Side) {
        use crate::cards::token;
        const SOLDIERS: [(crate::cards::Class, CardId); 6] = [
            (crate::cards::Class::Rogue, token("CATA_158t")),
            (crate::cards::Class::Warlock, token("CATA_725t")),
            (crate::cards::Class::Shaman, token("CATA_565t")),
            (crate::cards::Class::DemonHunter, token("CATA_525t")),
            (crate::cards::Class::Warrior, token("CATA_580t")),
            (crate::cards::Class::DeathKnight, token("CATA_780t")),
        ];
        self.player_mut(side).herald += 1;
        let class = self.player(side).class;
        let Some((_, card)) = SOLDIERS.iter().copied().find(|(c, _)| *c == class) else {
            return;
        };
        if !self.summon(side, card) {
            return;
        }
        // A Soldier's arrival effect is a battlecry in the table, but it is
        // summoned rather than played, so it is invoked here.
        let slot = self.player(side).board.len() as u8 - 1;
        if let Some(f) = crate::cards::behaviour_of(card).and_then(|b| b.battlecry) {
            f(
                self,
                &crate::cards::Ctx {
                    card,
                    side,
                    target: None,
                    source: Some(slot),
                    outcast: false,
                    dying: None,
                    marks: crate::state::Marks::NONE,
                    mana_spent: 0,
                },
            );
        }
    }

    /// Summon `n` random implemented minions costing exactly `cost`.
    pub fn summon_random_of_cost(&mut self, side: Side, cost: i16, n: usize) -> usize {
        let pool = crate::cards::discover_pool(|d| {
            d.kind() == crate::cards::Kind::Minion && d.cost == cost
        });
        if pool.is_empty() {
            return 0;
        }
        let mut made = 0;
        for _ in 0..n {
            let pick = self.rngs.effects.index(pool.len());
            if !self.summon(side, pool[pick]) {
                break;
            }
            made += 1;
        }
        made
    }

    /// Summon a random implemented minion matching `pred` — for cards that
    /// narrow by more than cost, such as Guard Dog's "a random 1-Cost
    /// Deathrattle minion".
    pub fn summon_random_where(
        &mut self,
        side: Side,
        pred: fn(&crate::cards::CardDef) -> bool,
    ) -> bool {
        let pool = crate::cards::discover_pool(pred);
        if pool.is_empty() {
            return false;
        }
        let pick = self.rngs.effects.index(pool.len());
        self.summon(side, pool[pick])
    }

    /// Equip a random implemented weapon matching `pred`.
    pub fn equip_random(&mut self, side: Side, pred: fn(&crate::cards::CardDef) -> bool) -> bool {
        let pool = crate::cards::discover_pool(pred);
        if pool.is_empty() {
            return false;
        }
        let pick = self.rngs.effects.index(pool.len());
        self.equip(side, pool[pick]);
        true
    }

    /// Put a card at the bottom of `side`'s deck -- the last card they will
    /// draw. The deck's "top" is the end of the array (drawing pops), so the
    /// bottom is index 0.
    pub fn put_on_bottom(&mut self, side: Side, card: CardId) -> bool {
        self.player_mut(side).deck.insert(0, DeckCard::new(card))
    }

    /// Put an already-built deck card at the bottom -- for a card that
    /// arrives there with stats or a cost already on it.
    pub fn put_deck_card_on_bottom(&mut self, side: Side, dc: DeckCard) -> bool {
        self.player_mut(side).deck.insert(0, dc)
    }

    /// Shuffle `side`'s deck in place -- for a card that adds copies to it
    /// and must not leave them all sitting on top, drawn before anything
    /// already there.
    pub fn shuffle_deck(&mut self, side: Side) {
        let mut deck = self.player(side).deck;
        self.rngs.library[side.index()].shuffle(deck.as_mut_slice());
        self.player_mut(side).deck = deck;
    }

    /// Destroy the top `n` cards of a deck.
    ///
    /// The top is the end of the array, the same end [`Game::draw`] takes
    /// from, so milling and drawing agree about what "top" means.
    pub fn mill(&mut self, side: Side, n: usize) -> usize {
        let mut gone = 0;
        for _ in 0..n {
            if self.player_mut(side).deck.pop().is_none() {
                break;
            }
            gone += 1;
        }
        gone
    }

    /// Shuffle a card into `side`'s deck at a random position. Distinct from
    /// [`Game::put_on_bottom`]: "shuffle into your deck" can land anywhere,
    /// not always last.
    pub fn shuffle_into_deck(&mut self, side: Side, card: CardId) -> bool {
        self.shuffle_deck_card(side, DeckCard::new(card))
    }

    /// Shuffle an already-built deck card in, keeping whatever is written on
    /// it -- stats, a cost, where it came from.
    pub fn shuffle_deck_card(&mut self, side: Side, dc: DeckCard) -> bool {
        let len = self.player(side).deck.len();
        let at = self.rngs.effects.index(len + 1);
        self.player_mut(side).deck.insert(at, dc)
    }

    /// Shuffle a random implemented card matching `pred` into `side`'s deck.
    pub fn shuffle_random_into_deck(
        &mut self,
        side: Side,
        pred: fn(&crate::cards::CardDef) -> bool,
    ) -> bool {
        let pool = crate::cards::discover_pool(pred);
        if pool.is_empty() {
            return false;
        }
        let pick = self.rngs.effects.index(pool.len());
        self.shuffle_into_deck(side, pool[pick])
    }

    /// Give every minion in an area attack for this turn — negative to debuff.
    pub fn temp_atk_area(&mut self, side: Side, area: Area, n: i16) {
        let mut hits: Inline<Target, { MAX_BOARD * 2 + 2 }> = Inline::new();
        self.collect_area(side, area, &mut hits);
        for t in hits.iter() {
            self.buff_temp_atk(*t, n);
        }
    }

    /// Re-fire the deathrattles of minions that died earlier this game.
    ///
    /// Not tracked as a graveyard: the engine keeps no history, so this
    /// re-fires the deathrattles of the friendly minions currently on the
    /// board instead. A documented approximation — Endbringer Umbra reads
    /// "died this game", and modelling that needs a per-player graveyard.
    pub fn retrigger_friendly_deathrattles(&mut self, side: Side, n: usize) {
        let mut fired = 0;
        for slot in 0..self.player(side).board.len() {
            if fired >= n {
                break;
            }
            let m = self.player(side).board[slot];
            if !m.active() {
                continue;
            }
            if let Some(f) = crate::cards::behaviour_of(m.card).and_then(|b| b.deathrattle) {
                f(
                    self,
                    &crate::cards::Ctx {
                        card: m.card,
                        side,
                        target: None,
                        source: Some(slot as u8),
                        outcast: false,
                        dying: Some(m),
                        marks: crate::state::Marks::NONE,
                        mana_spent: 0,
                    },
                );
                fired += 1;
            }
        }
    }

    /// Whether a Combo condition is met for the card currently resolving.
    ///
    /// `cards_played_turn` has already counted the combo card itself by the
    /// time its effect runs, so the test is "more than one", not "more than
    /// zero" — the off-by-one every implementation of this makes once.
    pub fn combo_active(&self, side: Side) -> bool {
        self.player(side).cards_played_turn > 1
    }

    /// How many *other* cards were played this turn, for cards that scale.
    pub fn other_cards_played(&self, side: Side) -> i16 {
        self.player(side).cards_played_turn.saturating_sub(1) as i16
    }

    /// Whether the side controls a live minion of a given tribe.
    ///
    /// `Races::ALL` on a minion matches every tribe, which is why this is not
    /// a plain bit test against the minion's own races.
    pub fn controls_race(&self, side: Side, race: crate::cards::Races) -> bool {
        self.player(side)
            .minions()
            .any(|m| m.races().any(race) || m.races().has(crate::cards::Races::ALL))
    }

    /// Healing split one point at a time among friendly characters, the way
    /// [`damage_split`](Self::damage_split) splits damage.
    ///
    /// Re-picked after every point, and a character already at full health is
    /// not a candidate — twelve points into a board that can only take four
    /// stop at four rather than being thrown away one at a time.
    pub fn heal_split(&mut self, side: Side, total: i16) {
        for _ in 0..total.max(0) {
            let mut pool: Inline<Target, { MAX_BOARD + 1 }> = Inline::new();
            if self.player(side).hero_hp < crate::state::START_HP {
                pool.push(Target::Hero(side));
            }
            for (i, m) in self.player(side).board.iter().enumerate() {
                if m.active() && m.is_minion() && m.damage > 0 {
                    pool.push(Target::Minion(side, i as u8));
                }
            }
            if pool.is_empty() {
                return;
            }
            let pick = self.rngs.effects.index(pool.len());
            self.heal(pool[pick], 1);
        }
    }

    // --------------------------------------------------------- graveyard

    /// The friendly minions that have died this game and match `pred`, as
    /// card ids in the order they died.
    ///
    /// A pool, not a set: two copies of the same minion that both died are
    /// two entries, and a resurrect that picks at random is twice as likely
    /// to find one. That is what the real pool does.
    pub fn dead_where(
        &self,
        side: Side,
        pred: impl Fn(&crate::cards::CardDef) -> bool,
    ) -> Inline<CardId, { crate::state::GRAVEYARD }> {
        let mut out = Inline::new();
        for card in self.player(side).graveyard.iter().copied() {
            if pred(card.def()) {
                out.push(card);
            }
        }
        out
    }

    /// Summon one friendly minion that died this game and matches `pred`,
    /// picked at random from the pool. Returns whether one was found.
    pub fn resurrect(&mut self, side: Side, pred: impl Fn(&crate::cards::CardDef) -> bool) -> bool {
        let pool = self.dead_where(side, pred);
        if pool.is_empty() {
            return false;
        }
        let pick = self.rngs.effects.index(pool.len());
        self.summon(side, pool[pick])
    }

    /// Summon the friendly minion with the highest printed Cost that died
    /// this game and matches `pred`. Ties go to the one that died first,
    /// which is the order the pool is kept in.
    pub fn resurrect_costliest(
        &mut self,
        side: Side,
        pred: impl Fn(&crate::cards::CardDef) -> bool,
    ) -> bool {
        let pool = self.dead_where(side, pred);
        // `max_by_key` would return the *last* maximum; the tie-break here is
        // the one that died first, so this folds by hand.
        let Some(best) = pool
            .iter()
            .copied()
            .reduce(|a, b| if b.def().cost > a.def().cost { b } else { a })
        else {
            return false;
        };
        self.summon(side, best)
    }

    /// Fire the deathrattle a card carries, with no body on the board.
    ///
    /// The Ctx gets a freshly printed copy of the minion as its `dying` body:
    /// the real one is long gone, and a deathrattle that scales with what it
    /// was carrying has nothing left to read. Returns whether the card had a
    /// deathrattle at all.
    pub fn fire_deathrattle_of(&mut self, side: Side, card: CardId) -> bool {
        let Some(f) = crate::cards::behaviour_of(card).and_then(|b| b.deathrattle) else {
            return false;
        };
        f(
            self,
            &crate::cards::Ctx {
                card,
                side,
                target: None,
                source: None,
                outcast: false,
                dying: Some(crate::state::Permanent::summon(card)),
                marks: crate::state::Marks::NONE,
                mana_spent: 0,
            },
        );
        true
    }

    /// A random enemy character, hero included.
    pub fn random_enemy(&mut self, side: Side) -> Option<Target> {
        let foe = side.other();
        let mut pool: Inline<Target, { MAX_BOARD + 1 }> = Inline::new();
        for (i, m) in self.player(foe).board.iter().enumerate() {
            if m.active() && m.is_minion() {
                pool.push(Target::Minion(foe, i as u8));
            }
        }
        pool.push(Target::Hero(foe));
        let pick = self.rngs.effects.index(pool.len());
        Some(pool[pick])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cards::{Class, by_name};
    use crate::state::Permanent;

    fn fixture(mine: &[&str], theirs: &[&str]) -> Game {
        let mut g = Game::new((Class::Mage, &[]), (Class::Mage, &[]), 5).unwrap();
        for n in mine {
            let mut m = Permanent::summon(by_name(n).unwrap());
            m.flags.remove(Flags::JUST_SUMMONED);
            g.players[0].board.push(m);
        }
        for n in theirs {
            let mut m = Permanent::summon(by_name(n).unwrap());
            m.flags.remove(Flags::JUST_SUMMONED);
            g.players[1].board.push(m);
        }
        g
    }

    #[test]
    fn spell_damage_adds_the_board_bonus() {
        let mut g = fixture(&["Bloodmage Thalnos"], &["Boulderfist Ogre"]); // +1, 6/7
        assert_eq!(g.players[0].spell_power(), 1);
        g.spell_damage(Side::Player0, Some(Target::Minion(Side::Player1, 0)), 3);
        assert_eq!(g.players[1].board[0].damage, 4, "3 + 1 Spell Damage");
    }

    #[test]
    fn combat_damage_ignores_spell_damage() {
        let mut g = fixture(&["Bloodmage Thalnos"], &["Boulderfist Ogre"]);
        g.deal_damage(Target::Minion(Side::Player1, 0), 3);
        assert_eq!(g.players[1].board[0].damage, 3);
    }

    #[test]
    fn an_area_effect_hits_the_right_side() {
        let mut g = fixture(
            &["Bloodfen Raptor"],
            &["Bloodfen Raptor", "Bloodfen Raptor"],
        );
        g.damage_area(Side::Player0, Area::EnemyMinions, 1);
        assert_eq!(g.players[0].board[0].damage, 0, "friendly board untouched");
        assert_eq!(g.players[1].board.len(), 2);
        assert!(g.players[1].board.iter().all(|m| m.damage == 1));
    }

    #[test]
    fn an_area_effect_kills_but_leaves_removal_to_the_engine() {
        // Verbs never sweep. Removing a body mid-effect would shift the slot
        // indices the effect had already collected — which is how Flamestrike
        // came to hit the wrong minions. The engine sweeps once the card that
        // called this has finished resolving.
        let mut g = fixture(&[], &["Bloodfen Raptor", "Bloodfen Raptor"]); // 3/2 each
        g.damage_area(Side::Player0, Area::EnemyMinions, 2);
        assert_eq!(g.players[1].board.len(), 2, "still present, at zero health");
        assert!(g.players[1].board.iter().all(|m| m.is_dead()));
        g.sweep_deaths();
        assert!(g.players[1].board.is_empty());
    }

    #[test]
    fn everything_includes_both_heroes() {
        let mut g = fixture(&["Bloodfen Raptor"], &["Bloodfen Raptor"]);
        g.damage_area(Side::Player0, Area::Everything, 1);
        assert_eq!(g.players[0].hero_hp, 29);
        assert_eq!(g.players[1].hero_hp, 29);
    }

    #[test]
    fn divine_shield_absorbs_one_instance_of_area_damage() {
        let mut g = fixture(&[], &["Argent Squire"]); // 1/1 Divine Shield
        g.damage_area(Side::Player0, Area::EnemyMinions, 5);
        g.sweep_deaths();
        assert_eq!(
            g.players[1].board.len(),
            1,
            "the shield should have eaten it"
        );
        assert!(!g.players[1].board[0].has(Keywords::DIVINE_SHIELD));
        g.damage_area(Side::Player0, Area::EnemyMinions, 1);
        g.sweep_deaths();
        assert!(g.players[1].board.is_empty());
    }

    #[test]
    fn split_damage_stops_when_the_board_empties() {
        // Ten points into a lone 1-health minion must not loop forever or
        // panic; the excess goes to the hero, who is in AllEnemies.
        let mut g = fixture(&[], &["Argent Squire"]);
        g.damage_split(Side::Player0, Area::AllEnemies, 10);
        assert!(g.players[1].hero_hp < 30);
    }

    #[test]
    fn destroy_ignores_health() {
        let mut g = fixture(&[], &["Boulderfist Ogre"]); // 6/7
        g.destroy(Target::Minion(Side::Player1, 0));
        assert!(g.players[1].board[0].is_dead(), "marked, not yet removed");
        g.sweep_deaths();
        assert!(g.players[1].board.is_empty());
    }

    #[test]
    fn buffs_add_to_current_stats() {
        let mut g = fixture(&["Bloodfen Raptor"], &[]);
        g.buff(Target::Minion(Side::Player0, 0), 2, 3);
        let m = &g.players[0].board[0];
        assert_eq!((m.atk, m.health()), (5, 5));
    }

    #[test]
    fn bounce_returns_the_card_to_hand() {
        let mut g = fixture(&["Bloodfen Raptor"], &[]);
        g.bounce(Target::Minion(Side::Player0, 0));
        assert!(g.players[0].board.is_empty());
        assert_eq!(g.players[0].hand.len(), 1);
        assert_eq!(g.players[0].hand[0].card.name(), "Bloodfen Raptor");
    }

    #[test]
    fn bounce_burns_the_card_when_the_hand_is_full() {
        let mut g = fixture(&["Bloodfen Raptor"], &[]);
        let filler = by_name("Bloodfen Raptor").unwrap();
        for _ in 0..10 {
            g.players[0].hand.push(crate::state::HandCard::new(filler));
        }
        g.bounce(Target::Minion(Side::Player0, 0));
        assert!(g.players[0].board.is_empty());
        assert_eq!(g.players[0].hand.len(), 10, "the eleventh card burns");
    }

    #[test]
    fn summoning_stops_at_a_full_board() {
        let mut g = fixture(&[], &[]);
        let made = g.summon_token(Side::Player0, crate::cards::token("CS2_boar"), 10);
        assert!(made <= MAX_BOARD, "summoned {made} onto a seven-slot board");
        assert_eq!(g.players[0].board.len(), made);
    }

    #[test]
    fn silence_through_the_verb_strips_keywords() {
        let mut g = fixture(&[], &["Goldshire Footman"]);
        assert!(g.players[1].board[0].has(Keywords::TAUNT));
        g.silence(Target::Minion(Side::Player1, 0));
        assert!(!g.players[1].board[0].has(Keywords::TAUNT));
    }

    #[test]
    fn crystals_stop_at_the_cap() {
        let mut g = fixture(&[], &[]);
        g.gain_crystal(Side::Player0, 20);
        assert_eq!(g.players[0].crystals, crate::state::MAX_MANA);
    }

    #[test]
    fn random_pickers_handle_an_empty_board() {
        let mut g = fixture(&[], &[]);
        assert_eq!(g.random_minion(Side::Player0), None);
        // The hero is always a valid enemy, so this one never returns None.
        assert_eq!(
            g.random_enemy(Side::Player0),
            Some(Target::Hero(Side::Player1))
        );
    }
}
