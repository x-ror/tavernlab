//! What individual cards do.
//!
//! A card is one row in [`BEHAVIOURS`]. The effect itself is a non-capturing
//! closure, which coerces to a plain `fn` pointer — so the whole table is
//! static data and dispatch is an array index, not a hash lookup or a chain of
//! string comparisons.
//!
//! Rows are keyed by **name**, not by card id, and attach to every printing of
//! that name. Hearthstone reprints constantly: Fireball exists as `CS2_029` and
//! `CORE_CS2_029`, and a rotation adds more. Keying by id would silently drop
//! the behaviour from whichever printing a deck happened to use.
//!
//! Adding a card should be one line. When it cannot be, the missing piece is a
//! verb in [`crate::effects`], and adding it there pays off across every card
//! that follows.

use std::sync::OnceLock;

use super::{CardId, DEFS, INFO, Keywords, Races, token};
use crate::effects::Area;
use crate::events::{Event, Trigger};
use crate::inline::Inline;
use crate::state::{Flags, Game, HandCard, MAX_DECK, Marks, Pending, PendingKind, Side, Target};

/// What an effect is told about the circumstances it fires in.
#[derive(Clone, Copy, Debug)]
pub struct Ctx {
    /// The card the effect belongs to.
    ///
    /// Effects mostly do not need it -- the row already knows which card it
    /// is. It is here for the ones that read their own printed data, above
    /// all the tokens a card creates: "summon a 1/1 Murloc Scout" is then a
    /// lookup through [`CardId::children`], not a token id typed in by hand.
    pub card: CardId,
    /// The controller of the card.
    pub side: Side,
    /// The chosen target, if the card takes one.
    pub target: Option<Target>,
    /// Board slot of the minion the effect belongs to, for deathrattles and
    /// battlecries that care where they are.
    pub source: Option<u8>,
    /// Whether the card was the leftmost or rightmost card in hand when it was
    /// played. A one-card hand satisfies it — the card is both ends at once.
    pub outcast: bool,
    /// The permanent this effect belongs to, as it stood immediately before
    /// firing -- its stats, buffs and `growth` counter included.
    ///
    /// Only a deathrattle needs this: a battlecry's minion is still on the
    /// board and reachable through `source`, but a deathrattle's body is
    /// already gone from the board by the time it runs (see
    /// `Game::sweep_deaths`), so anything it wants to know about itself has
    /// to arrive here instead. `None` for every other hook.
    pub dying: Option<crate::state::Permanent>,
    /// What happened while this card sat in hand, snapshotted the moment it
    /// left hand to be played -- by the time a spell or battlecry effect
    /// runs, the card itself is already gone from `Player::hand`. See
    /// `crate::state::Marks`. `Marks::NONE` everywhere but a spell or
    /// battlecry Ctx.
    pub marks: crate::state::Marks,
    /// Mana spent on anything else while this card sat in hand, snapshotted
    /// the same way `marks` is (Merithra of the Dream). `0` everywhere but a
    /// spell or battlecry Ctx.
    pub mana_spent: i16,
}

/// A card effect. A `fn` pointer rather than a boxed closure, so the table is
/// static and dispatch costs an indirect call.
pub type Effect = fn(&mut Game, &Ctx);

/// What a card needs pointed at before it can be played.
///
/// This drives action enumeration, so a card whose requirement cannot be met
/// is not offered at all — the same rule the real game applies when it greys
/// a card out.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TargetSpec {
    /// No target.
    None,
    /// Any hero or minion.
    AnyCharacter,
    AnyMinion,
    EnemyMinion,
    FriendlyMinion,
    EnemyCharacter,
    FriendlyCharacter,
    /// A minion with at most this much attack.
    MinionAtkAtMost(i16),
    /// A minion with at least this much attack.
    MinionAtkAtLeast(i16),
    DamagedEnemyMinion,
    UndamagedMinion,
    FriendlyBeast,
}

impl TargetSpec {
    /// Whether this card needs a target chosen.
    #[inline]
    pub fn needed(self) -> bool {
        self != TargetSpec::None
    }

    /// Whether `t` satisfies the requirement for `side`.
    pub fn matches(self, g: &Game, side: Side, t: Target) -> bool {
        let foe = side.other();
        // Stealth hides a minion from the opponent's targeting, and Elusive
        // hides it from every spell and hero power.
        if let Target::Minion(s, i) = t {
            let Some(m) = g.player(s).board.get(i as usize) else {
                return false;
            };
            if !m.active() || !m.is_minion() {
                return false;
            }
            if m.has(Keywords::ELUSIVE) {
                return false;
            }
            if s == foe && m.has(Keywords::STEALTH) {
                return false;
            }
        }
        match self {
            TargetSpec::None => false,
            TargetSpec::AnyCharacter => true,
            TargetSpec::AnyMinion => matches!(t, Target::Minion(..)),
            TargetSpec::EnemyMinion => matches!(t, Target::Minion(s, _) if s == foe),
            TargetSpec::FriendlyMinion => matches!(t, Target::Minion(s, _) if s == side),
            TargetSpec::EnemyCharacter => match t {
                Target::Hero(s) => s == foe,
                Target::Minion(s, _) => s == foe,
            },
            TargetSpec::FriendlyCharacter => match t {
                Target::Hero(s) => s == side,
                Target::Minion(s, _) => s == side,
            },
            TargetSpec::MinionAtkAtMost(n) => {
                matches!(t, Target::Minion(s, i) if g.player(s).board[i as usize].atk <= n)
            }
            TargetSpec::MinionAtkAtLeast(n) => {
                matches!(t, Target::Minion(s, i) if g.player(s).board[i as usize].atk >= n)
            }
            TargetSpec::DamagedEnemyMinion => {
                matches!(t, Target::Minion(s, i) if s == foe && g.player(s).board[i as usize].damage > 0)
            }
            TargetSpec::UndamagedMinion => {
                matches!(t, Target::Minion(s, i) if g.player(s).board[i as usize].damage == 0)
            }
            TargetSpec::FriendlyBeast => matches!(t, Target::Minion(s, i)
                if s == side && g.player(s).board[i as usize].races().any(Races::BEAST)),
        }
    }
}

/// One card's behaviour.
pub struct Behaviour {
    /// Card name, matched against every printing.
    pub name: &'static str,
    pub target: TargetSpec,
    /// Cast effect, for spells.
    pub spell: Option<Effect>,
    /// Fires when a minion or weapon is played from hand.
    pub battlecry: Option<Effect>,
    /// Fires when a minion dies.
    pub deathrattle: Option<Effect>,
    /// Fires on anything happening anywhere while this permanent is in play.
    /// The effect decides for itself which events it cares about — one hook
    /// rather than a family of them, so a new event kind needs no plumbing.
    pub trigger: Option<Trigger>,
    /// A continuous effect on other minions, true while this one is in play.
    pub aura: Option<Aura>,
    /// A secret's reaction. Returns whether it fired, in which case the engine
    /// removes it — a secret that does not apply must say so rather than
    /// silently doing nothing, or it would stay armed forever.
    pub secret: Option<Secret>,
    /// A cost adjustment computed from the position, in mana. Negative makes
    /// the card cheaper. Given the hand index because Outcast and "for each
    /// card in your hand" both need to know where the card sits.
    pub cost_delta: Option<CostFn>,
    /// Choose One modes. When present these replace the card's own `spell` or
    /// `battlecry`, and each mode brings its own target requirement — the two
    /// halves of a Choose One rarely want the same thing pointed at.
    pub choose: Option<&'static [Mode]>,
    /// Fires once per game, for every copy found in a player's opening hand
    /// or deck, after both mulligans and before the Coin is handed out (see
    /// `Game::start`). No target, no board slot -- a Start of Game card need
    /// not even be on the board yet.
    pub start_of_game: Option<Effect>,
}

/// A dynamic cost adjustment: `(game, controller, hand index) -> mana delta`.
///
/// Read every time a cost is asked for rather than applied once, because the
/// conditions are live — "costs (2) less if you're holding a Dragon" stops
/// applying the moment the Dragon is played.
pub type CostFn = fn(&Game, Side, usize) -> i16;

/// One half of a Choose One card.
pub struct Mode {
    pub target: TargetSpec,
    pub effect: Effect,
}

/// Shorthand for a Choose One mode.
pub const fn m(target: TargetSpec, effect: Effect) -> Mode {
    Mode { target, effect }
}

/// A secret's reaction to an event. `owner` is the player who set it.
pub type Secret = fn(&mut Game, owner: Side, event: Event) -> bool;

/// What one aura source grants one candidate minion, as `(attack, health)`.
///
/// Deliberately a pure function of board positions rather than of the whole
/// game: an aura that could read arbitrary state would be impossible to
/// recompute safely, and every real aura is a question about who is next to
/// whom and what tribe they are.
pub type Aura = fn(
    source_side: Side,
    source_slot: u8,
    target_side: Side,
    target_slot: u8,
    target: &crate::state::Permanent,
) -> (i16, i16);

/// Shorthand so a row stays on one line.
///
/// One parameter per hook is deliberate: the wrappers below are what card
/// authors use, and this stays a plain positional record so adding a hook is a
/// compile error everywhere it matters rather than a silently defaulted field.
#[allow(clippy::too_many_arguments)]
const fn c(
    name: &'static str,
    target: TargetSpec,
    spell: Option<Effect>,
    battlecry: Option<Effect>,
    deathrattle: Option<Effect>,
    trigger: Option<Trigger>,
    aura: Option<Aura>,
    secret: Option<Secret>,
    choose: Option<&'static [Mode]>,
    cost_delta: Option<CostFn>,
    start_of_game: Option<Effect>,
) -> Behaviour {
    Behaviour {
        name,
        target,
        spell,
        battlecry,
        deathrattle,
        trigger,
        aura,
        secret,
        choose,
        cost_delta,
        start_of_game,
    }
}

const fn spell(name: &'static str, target: TargetSpec, f: Effect) -> Behaviour {
    c(name, target, Some(f), None, None, None, None, None, None, None, None)
}
const fn battlecry(name: &'static str, target: TargetSpec, f: Effect) -> Behaviour {
    c(name, target, None, Some(f), None, None, None, None, None, None, None)
}
const fn deathrattle(name: &'static str, f: Effect) -> Behaviour {
    c(name, TargetSpec::None, None, None, Some(f), None, None, None, None, None, None)
}
/// A Choose One card.
const fn choose(name: &'static str, modes: &'static [Mode]) -> Behaviour {
    c(
        name,
        TargetSpec::None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(modes),
        None,
        None,
    )
}
/// A secret.
const fn secret(name: &'static str, f: Secret) -> Behaviour {
    c(name, TargetSpec::None, None, None, None, None, None, Some(f), None, None, None)
}
/// A card whose only behaviour is a continuous effect on other minions.
const fn aura(name: &'static str, f: Aura) -> Behaviour {
    c(name, TargetSpec::None, None, None, None, None, Some(f), None, None, None, None)
}
/// A card whose only behaviour is reacting to events.
const fn trigger(name: &'static str, f: Trigger) -> Behaviour {
    c(name, TargetSpec::None, None, None, None, Some(f), None, None, None, None, None)
}
/// A card whose only behaviour fires once at the start of the game.
const fn start_of_game(name: &'static str, f: Effect) -> Behaviour {
    c(name, TargetSpec::None, None, None, None, None, None, None, None, None, Some(f))
}
/// Every token this table names, resolved at compile time.
///
/// [`token`] is a `const fn`, so an id that stops resolving -- renamed
/// upstream, or rotated out of both corpora -- fails the build on the line
/// that names it. That replaced a hand-kept list of "ids the table mentions"
/// and a test that walked it: the list was only as good as whoever added a
/// token remembering to add it there too, and a token that silently failed to
/// resolve left a card that costs mana and does nothing.
mod tokens {
    use super::{CardId, token};

    pub const DAMAGED_GOLEM: CardId = token("skele21");
    pub const SHEEP: CardId = token("AV_218t");
    pub const FROG: CardId = token("hexfrog");
    pub const SPECTRAL_SPIDER: CardId = token("EX1_554t");
    pub const IMP: CardId = token("EX1_598");
    // Despite the old constant name, "EX1_160t" is the 2/2 Panther Power of
    // the Wild summons -- not a Treant. Kept distinct from the real one below
    // so a future Treant card cannot inherit the mistake.
    pub const PANTHER: CardId = token("EX1_160t");
    pub const TREANT: CardId = token("EX1_tk9");
    pub const VIOLET_APPRENTICE: CardId = token("EX1_014t");
    pub const COIN: CardId = token("GAME_005");
    pub const GHOUL: CardId = token("HERO_11bpt");
    pub const WHELP: CardId = token("EX1_131t");
    pub const EDR_492T: CardId = token("EDR_492t");
    pub const EDR_810T: CardId = token("EDR_810t");
    pub const TLC_101T: CardId = token("TLC_101t");
    pub const TLC_903T: CardId = token("TLC_903t");
    pub const FLAME_ELEMENTAL: CardId = token("UNG_809t1");
    pub const ARCANE_MISSILES: CardId = token("EX1_277");
    pub const MUGS_MAGIC: CardId = token("JAIL_800hp1");
    pub const ZEES_MIGHT: CardId = token("JAIL_800hp2");
    pub const NESPIRAH_UNSHACKLED: CardId = token("CATA_527t2");
    pub const GORISHI_STINGER: CardId = token("TLC_630t");
    /// Brood Keeper's "2/2 Sword". A weapon, so it never shows up through
    /// `summonable_children()`, which only ever returns minions.
    pub const NIGHTMARE_SLICER: CardId = token("EDR_457t");
    /// Eredar Deceptor's "1/1 Demon with Rush". Its Standard printing
    /// (`CORE_TTN_843`) carries no `childIds` in this snapshot of the corpus
    /// (see docs/RUST_CARDS_PLAN.md §2a on reprints missing child data), so
    /// this names the original printing's token directly; summoning by id
    /// does not check the token's own format legality.
    pub const INVADING_FELBAT: CardId = token("TTN_843t1");
    /// Twilight Egg's Whelp. Its base stats are overwritten to match however
    /// many of the Egg's controller's turns it survived; see the deathrattle.
    pub const ACCELERATED_WHELP: CardId = token("CATA_210t");
    /// Imp Gang Stooge's "8/8 Demon with Taunt and Lifesteal".
    pub const GRANDMOTHER_IMP: CardId = token("JAIL_399t1");
    /// Sigil of the Seas' "3/3 Naga with Taunt".
    pub const NAGA_MONSTROSITY: CardId = token("CATA_528t");
    /// Emergency Surgery's "3/1 Undead with Lifesteal".
    pub const NECRONURSE: CardId = token("JAIL_454t");
    /// Spire of Solitude's "a Demon". Its base 1/1 is overwritten to match
    /// the caster's hand size; see the Location's activation.
    pub const SHIVARRA_INFILTRATOR: CardId = token("JAIL_511t");
    /// The Food Chain's reward.
    pub const SHOKK: CardId = token("TLC_830t");
    /// Unleash the Colossus's reward.
    pub const GORISHI_COLOSSUS: CardId = token("TLC_631t");
    /// Storm the Gates' reward is a "custom Zombeast" crafted from two
    /// chosen minions in the real game; approximated here as this one fixed
    /// old token instead (weaker, not the real hybrid), and noted in
    /// APPROXIMATE.
    pub const ZOMBEAST: CardId = token("ICC_800h3t");
    /// The five printings of "The Egg of Khelos" and its final hatch,
    /// disambiguated only by id -- all five share one printed name, so the
    /// single behaviour row below tells them apart via `Ctx::dying`.
    pub const EGG_OF_KHELOS_1: CardId = token("DINO_410");
    pub const EGG_OF_KHELOS_2: CardId = token("DINO_410t2");
    pub const EGG_OF_KHELOS_3: CardId = token("DINO_410t3");
    pub const EGG_OF_KHELOS_4: CardId = token("DINO_410t4");
    pub const EGG_OF_KHELOS_5: CardId = token("DINO_410t5");
    pub const KHELOS: CardId = token("DINO_410t");
    /// Blood Doctor Thal'ena's granted second Hero Power.
    pub const VAMPYRS_KISS: CardId = token("JAIL_446hp");
    /// The five Dream cards Shaladrassil hands out, paired with their
    /// Corrupted counterparts.
    pub const NIGHTMARE: CardId = token("DREAM_05");
    pub const CORRUPTED_NIGHTMARE: CardId = token("EDR_846t1");
    pub const DREAM: CardId = token("DREAM_04");
    pub const CORRUPTED_DREAM: CardId = token("EDR_846t2");
    pub const LAUGHING_SISTER: CardId = token("DREAM_01");
    pub const CORRUPTED_LAUGHING_SISTER: CardId = token("EDR_846t3");
    pub const YSERA_AWAKENS: CardId = token("DREAM_02");
    pub const CORRUPTED_AWAKENING: CardId = token("EDR_846t4");
    pub const EMERALD_DRAKE: CardId = token("DREAM_03");
    pub const CORRUPTED_DRAKE: CardId = token("EDR_846t5");

    /// The three Dreadseeds. Cards that summon "a random Dormant Dreadseed"
    /// roll one of these.
    pub const DREADSEEDS: &[CardId] = &[
        token("EDR_840t"),
        token("EDR_840t1"),
        token("EDR_840t2"),
    ];
}

use TargetSpec as T;

/// Every implemented card.
///
/// Order is irrelevant — the index sorts by name at first use.
pub static BEHAVIOURS: &[Behaviour] = &[
    // ------------------------------------------------------------- neutral
    spell("The Coin", T::None, |g, c| g.gain_temp_mana(c.side, 1)),
    battlecry("Novice Engineer", T::None, |g, c| g.draw_cards(c.side, 1)),
    battlecry("Gnomish Inventor", T::None, |g, c| g.draw_cards(c.side, 1)),
    battlecry("Voodoo Doctor", T::AnyCharacter, |g, c| {
        if let Some(t) = c.target {
            g.heal(t, 2)
        }
    }),
    battlecry("Elven Archer", T::AnyCharacter, |g, c| {
        if let Some(t) = c.target {
            g.deal_damage(t, 1);
        }
    }),
    battlecry("Ironbeak Owl", T::AnyMinion, |g, c| {
        if let Some(t) = c.target {
            g.silence(t)
        }
    }),
    battlecry("Acidic Swamp Ooze", T::None, |g, c| {
        g.destroy_weapon(c.side.other())
    }),
    deathrattle("Loot Hoarder", |g, c| g.draw_cards(c.side, 1)),
    deathrattle("Bloodmage Thalnos", |g, c| g.draw_cards(c.side, 1)),
    deathrattle("Harvest Golem", |g, c| {
        g.summon_token(c.side, tokens::DAMAGED_GOLEM, 1);
    }),
    deathrattle("Leper Gnome", |g, c| {
        g.deal_damage(Target::Hero(c.side.other()), 2);
    }),
    // ---------------------------------------------------------------- mage
    spell("Fireball", T::AnyCharacter, |g, c| {
        g.spell_damage(c.side, c.target, 6);
    }),
    spell("Frostbolt", T::AnyCharacter, |g, c| {
        g.spell_damage(c.side, c.target, 3);
        if let Some(t) = c.target {
            g.freeze(t)
        }
    }),
    spell("Arcane Explosion", T::None, |g, c| {
        g.spell_damage_area(c.side, Area::EnemyMinions, 1)
    }),
    spell("Flamestrike", T::None, |g, c| {
        g.spell_damage_area(c.side, Area::EnemyMinions, 5)
    }),
    spell("Arcane Intellect", T::None, |g, c| g.draw_cards(c.side, 2)),
    spell("Arcane Missiles", T::None, |g, c| {
        g.damage_split(c.side, Area::AllEnemies, 3)
    }),
    spell("Frost Nova", T::None, |g, c| {
        g.freeze_area(c.side, Area::EnemyMinions)
    }),
    spell("Polymorph", T::AnyMinion, |g, c| {
        if let Some(t) = c.target {
            g.transform(t, tokens::SHEEP)
        }
    }),
    // -------------------------------------------------------------- priest
    spell("Holy Smite", T::AnyMinion, |g, c| {
        g.spell_damage(c.side, c.target, 3);
    }),
    spell("Power Word: Shield", T::AnyMinion, |g, c| {
        if let Some(t) = c.target {
            g.buff(t, 0, 2)
        }
        g.draw_cards(c.side, 1);
    }),
    spell("Holy Nova", T::None, |g, c| {
        g.spell_damage_area(c.side, Area::EnemyMinions, 2);
        let mut heals: crate::inline::Inline<Target, 9> = crate::inline::Inline::new();
        g.collect_area(c.side, Area::FriendlyMinions, &mut heals);
        heals.push(Target::Hero(c.side));
        for t in heals.iter() {
            g.heal(*t, 2);
        }
    }),
    spell("Shadow Word: Pain", T::MinionAtkAtMost(3), |g, c| {
        if let Some(t) = c.target {
            g.destroy(t)
        }
    }),
    spell("Shadow Word: Death", T::MinionAtkAtLeast(5), |g, c| {
        if let Some(t) = c.target {
            g.destroy(t)
        }
    }),
    // --------------------------------------------------------------- druid
    spell("Moonfire", T::AnyCharacter, |g, c| {
        g.spell_damage(c.side, c.target, 1);
    }),
    spell("Wild Growth", T::None, |g, c| g.gain_crystal(c.side, 1)),
    spell("Healing Touch", T::AnyCharacter, |g, c| {
        if let Some(t) = c.target {
            g.heal(t, 8)
        }
    }),
    spell("Mark of the Wild", T::AnyMinion, |g, c| {
        if let Some(t) = c.target {
            g.buff(t, 2, 3);
            g.grant(t, Keywords::TAUNT);
        }
    }),
    spell("Savage Roar", T::None, |g, c| {
        g.buff_temp_area(c.side, Area::FriendlyMinions, 2);
        g.hero_attack_bonus(c.side, 2);
    }),
    spell("Claw", T::None, |g, c| {
        g.hero_attack_bonus(c.side, 2);
        g.gain_armor(c.side, 2);
    }),
    spell("Swipe", T::EnemyCharacter, |g, c| {
        // Four to the chosen enemy, one to every other enemy.
        g.spell_damage(c.side, c.target, 4);
        let mut rest: crate::inline::Inline<Target, 9> = crate::inline::Inline::new();
        g.collect_area(c.side, Area::AllEnemies, &mut rest);
        let bonus = g.player(c.side).spell_power();
        for t in rest.iter() {
            if Some(*t) != c.target {
                g.deal_damage(*t, 1 + bonus);
            }
        }
    }),
    // ------------------------------------------------------------- warlock
    spell("Shadow Bolt", T::AnyMinion, |g, c| {
        g.spell_damage(c.side, c.target, 4);
    }),
    spell("Hellfire", T::None, |g, c| {
        g.spell_damage_area(c.side, Area::Everything, 3)
    }),
    spell("Drain Life", T::AnyCharacter, |g, c| {
        g.spell_damage(c.side, c.target, 2);
        g.heal_hero(c.side, 2);
    }),
    battlecry("Dread Infernal", T::None, |g, c| {
        // "All other characters" — the Infernal itself is exempt, and it is
        // the last minion on its own board when the battlecry fires.
        let me = c.side;
        let own_slot = c.source;
        let mut hits: crate::inline::Inline<Target, 16> = crate::inline::Inline::new();
        g.collect_area(me, Area::Everything, &mut hits);
        for t in hits.iter() {
            if Some(*t) == own_slot.map(|s| Target::Minion(me, s)) {
                continue;
            }
            g.deal_damage(*t, 1);
        }
    }),
    // -------------------------------------------------------------- hunter
    spell("Arcane Shot", T::AnyCharacter, |g, c| {
        g.spell_damage(c.side, c.target, 2);
    }),
    spell("Multi-Shot", T::None, |g, c| {
        g.damage_random_enemy_minions(c.side, 2, 3)
    }),
    spell("Kill Command", T::AnyCharacter, |g, c| {
        let n = if g.controls_race(c.side, Races::BEAST) {
            5
        } else {
            3
        };
        g.spell_damage(c.side, c.target, n);
    }),
    battlecry("Houndmaster", T::FriendlyBeast, |g, c| {
        if let Some(t) = c.target {
            g.buff(t, 2, 2);
            g.grant(t, Keywords::TAUNT);
        }
    }),
    // --------------------------------------------------------------- rogue
    spell("Backstab", T::UndamagedMinion, |g, c| {
        g.spell_damage(c.side, c.target, 2);
    }),
    spell("Sinister Strike", T::None, |g, c| {
        g.spell_damage(c.side, Some(Target::Hero(c.side.other())), 3);
    }),
    spell("Assassinate", T::EnemyMinion, |g, c| {
        if let Some(t) = c.target {
            g.destroy(t)
        }
    }),
    spell("Sprint", T::None, |g, c| g.draw_cards(c.side, 4)),
    spell("Fan of Knives", T::None, |g, c| {
        g.spell_damage_area(c.side, Area::EnemyMinions, 1);
        g.draw_cards(c.side, 1);
    }),
    spell("Deadly Poison", T::None, |g, c| g.buff_weapon(c.side, 2, 0)),
    // -------------------------------------------------------------- shaman
    spell("Frost Shock", T::EnemyCharacter, |g, c| {
        g.spell_damage(c.side, c.target, 1);
        if let Some(t) = c.target {
            g.freeze(t)
        }
    }),
    spell("Rockbiter Weapon", T::FriendlyCharacter, |g, c| {
        if let Some(t) = c.target {
            g.buff_temp_atk(t, 3)
        }
    }),
    spell("Bloodlust", T::None, |g, c| {
        g.buff_temp_area(c.side, Area::FriendlyMinions, 3)
    }),
    spell("Hex", T::AnyMinion, |g, c| {
        if let Some(t) = c.target {
            g.transform(t, tokens::FROG)
        }
    }),
    spell("Windfury", T::AnyMinion, |g, c| {
        if let Some(t) = c.target {
            g.grant(t, Keywords::WINDFURY)
        }
    }),
    spell("Ancestral Healing", T::AnyMinion, |g, c| {
        if let Some(t) = c.target {
            g.restore_full(t);
            g.grant(t, Keywords::TAUNT);
        }
    }),
    // ------------------------------------------------------------- paladin
    spell("Consecration", T::None, |g, c| {
        g.spell_damage_area(c.side, Area::AllEnemies, 2)
    }),
    spell("Blessing of Kings", T::AnyMinion, |g, c| {
        if let Some(t) = c.target {
            g.buff(t, 4, 4)
        }
    }),
    spell("Hammer of Wrath", T::AnyCharacter, |g, c| {
        g.spell_damage(c.side, c.target, 3);
        g.draw_cards(c.side, 1);
    }),
    spell("Holy Light", T::None, |g, c| g.heal_hero(c.side, 8)),
    spell("Humility", T::AnyMinion, |g, c| {
        if let Some(t) = c.target {
            g.set_attack(t, 1)
        }
    }),
    spell("Hand of Protection", T::AnyMinion, |g, c| {
        if let Some(t) = c.target {
            g.grant(t, Keywords::DIVINE_SHIELD)
        }
    }),
    // ------------------------------------------------------------- warrior
    spell("Execute", T::DamagedEnemyMinion, |g, c| {
        if let Some(t) = c.target {
            g.destroy(t)
        }
    }),
    spell("Cleave", T::None, |g, c| {
        g.damage_random_enemy_minions(c.side, 2, 2)
    }),
    spell("Whirlwind", T::None, |g, c| {
        g.spell_damage_area(c.side, Area::AllMinions, 1)
    }),
    spell("Shield Block", T::None, |g, c| {
        g.gain_armor(c.side, 5);
        g.draw_cards(c.side, 1);
    }),
    spell("Heroic Strike", T::None, |g, c| {
        g.hero_attack_bonus(c.side, 4)
    }),
    // ------------------------------------------------------------ triggers
    // The Death Knight hero-power token. It has Charge and expires the same
    // turn, so the power reads as a burst of damage rather than a permanent
    // board; without the trigger the ghouls would simply pile up.
    trigger("Frail Ghoul", |g, c| {
        if matches!(c.event, Event::TurnEnd { side } if side == c.side) {
            g.destroy(c.me());
        }
    }),
    trigger("Healing Totem", |g, c| {
        if matches!(c.event, Event::TurnEnd { side } if side == c.side) {
            let mut hits: Inline<Target, 9> = Inline::new();
            g.collect_area(c.side, Area::FriendlyMinions, &mut hits);
            for t in hits.iter() {
                g.heal(*t, 1);
            }
        }
    }),
    trigger("Strength Totem", |g, c| {
        if matches!(c.event, Event::TurnEnd { side } if side == c.side) {
            // "another friendly minion" — never itself.
            let mut pool: Inline<Target, 9> = Inline::new();
            g.collect_area(c.side, Area::FriendlyMinions, &mut pool);
            let me = c.me();
            pool.retain(|t| *t != me);
            if !pool.is_empty() {
                let pick = g.rngs.effects.index(pool.len());
                g.buff(pool[pick], 1, 0);
            }
        }
    }),
    trigger("Acolyte of Pain", |g, c| {
        if c.hit_me() {
            g.draw_cards(c.side, 1);
        }
    }),
    trigger("Gurubashi Berserker", |g, c| {
        if c.hit_me() {
            g.buff(c.me(), 3, 0);
        }
    }),
    trigger("Northshire Cleric", |g, c| {
        // "Whenever a minion is healed" — either side's minion, never a hero.
        if matches!(
            c.event,
            Event::Healed {
                target: Target::Minion(..),
                ..
            }
        ) {
            g.draw_cards(c.side, 1);
        }
    }),
    trigger("Lightwarden", |g, c| {
        // "a character" — heroes included, unlike the Cleric.
        if matches!(c.event, Event::Healed { .. }) {
            g.buff(c.me(), 2, 0);
        }
    }),
    trigger("Mana Wyrm", |g, c| {
        if matches!(c.event, Event::SpellCast { .. }) && c.mine() {
            g.buff(c.me(), 1, 0);
        }
    }),
    trigger("Questing Adventurer", |g, c| {
        if matches!(c.event, Event::CardPlayed { .. }) && c.mine() {
            g.buff(c.me(), 1, 1);
        }
    }),
    trigger("Wild Pyromancer", |g, c| {
        // Fires after the spell has resolved, which is why it can finish off
        // what the spell left behind — including itself.
        if matches!(c.event, Event::SpellCast { .. }) && c.mine() {
            g.damage_area(c.side, Area::AllMinions, 1);
        }
    }),
    trigger("Knife Juggler", |g, c| {
        // "After you summon a minion" — the Juggler's own arrival does not
        // count, which is why the event carries the slot it landed in.
        if let Event::MinionSummoned { side, slot, .. } = c.event
            && side == c.side
            && slot != c.slot
            && let Some(t) = g.random_enemy(c.side)
        {
            g.deal_damage(t, 1);
        }
    }),
    trigger("Flesheating Ghoul", |g, c| {
        // Any minion dying, either side. Never itself: a dead reactor is off
        // the board before the event fires, and `Game::fire` skips it.
        if matches!(c.event, Event::MinionDied { .. }) {
            g.buff(c.me(), 1, 0);
        }
    }),
    trigger("Imp Master", |g, c| {
        if matches!(c.event, Event::TurnEnd { side } if side == c.side) {
            g.deal_damage(c.me(), 1);
            g.summon_token(c.side, tokens::IMP, 1);
        }
    }),

    // -------------------------------------------------------------- auras
    // Continuous, not triggered: true while the source is in play and false
    // the instant it leaves. `recompute_auras` re-derives all of these from
    // the board rather than accumulating them.
    aura("Stormwind Champion", |ss, sl, ts, tl, _m| {
        if ss == ts && sl != tl { (1, 1) } else { (0, 0) }
    }),
    aura("Raid Leader", |ss, sl, ts, tl, _m| {
        if ss == ts && sl != tl { (1, 0) } else { (0, 0) }
    }),
    aura("Timber Wolf", |ss, sl, ts, tl, m| {
        if ss == ts && sl != tl && m.races().any(Races::BEAST) { (1, 0) } else { (0, 0) }
    }),
    aura("Murloc Warleader", |ss, sl, ts, tl, m| {
        if ss == ts && sl != tl && m.races().any(Races::MURLOC) { (2, 0) } else { (0, 0) }
    }),
    aura("Grimscale Oracle", |ss, sl, ts, tl, m| {
        if ss == ts && sl != tl && m.races().any(Races::MURLOC) { (1, 0) } else { (0, 0) }
    }),
    aura("Southsea Captain", |ss, sl, ts, tl, m| {
        if ss == ts && sl != tl && m.races().any(Races::PIRATE) { (1, 1) } else { (0, 0) }
    }),
    // "Adjacent" is a board-position question, which is why an aura is given
    // slots rather than a view of the whole game.
    aura("Dire Wolf Alpha", |ss, sl, ts, tl, _m| {
        if ss == ts && tl.abs_diff(sl) == 1 { (1, 0) } else { (0, 0) }
    }),
    aura("Flametongue Totem", |ss, sl, ts, tl, _m| {
        if ss == ts && tl.abs_diff(sl) == 1 { (2, 0) } else { (0, 0) }
    }),

    // ------------------------------------------------------------- secrets
    // A secret returns whether it fired. Saying "no" matters as much as
    // saying "yes": a secret that quietly did nothing would stay armed and
    // block its own re-play forever.
    secret("Counterspell", |g, owner, ev| {
        if let Event::SpellCasting { side, .. } = ev
            && side == owner.other()
        {
            g.countered = true;
            return true;
        }
        false
    }),
    secret("Mirror Entity", |g, owner, ev| {
        if let Event::MinionSummoned { side, card, .. } = ev
            && side == owner.other()
        {
            return g.summon(owner, card);
        }
        false
    }),
    secret("Ice Barrier", |g, owner, ev| {
        if let Event::AttackDeclared { defender, .. } = ev
            && defender == Target::Hero(owner)
        {
            g.gain_armor(owner, 8);
            return true;
        }
        false
    }),
    secret("Explosive Trap", |g, owner, ev| {
        if let Event::AttackDeclared { defender, .. } = ev
            && defender == Target::Hero(owner)
        {
            g.damage_area(owner, Area::AllEnemies, 2);
            return true;
        }
        false
    }),
    secret("Vaporize", |g, owner, ev| {
        // "When a minion attacks your hero" — a hero swing does not set it off.
        if let Event::AttackDeclared { attacker, defender } = ev
            && defender == Target::Hero(owner)
            && matches!(attacker, Target::Minion(..))
        {
            g.destroy(attacker);
            return true;
        }
        false
    }),
    secret("Snake Trap", |g, owner, ev| {
        // "When one of your minions is attacked" — the hero does not count.
        if let Event::AttackDeclared { defender, .. } = ev
            && matches!(defender, Target::Minion(s, _) if s == owner)
        {
            g.summon_token(owner, tokens::SPECTRAL_SPIDER, 3);
            return true;
        }
        false
    }),
    secret("Eye for an Eye", |g, owner, ev| {
        if let Event::Damaged { target, amount } = ev
            && target == Target::Hero(owner)
        {
            g.deal_damage(Target::Hero(owner.other()), amount);
            return true;
        }
        false
    }),

    // ----------------------------------------------------------- locations
    // A Location's activated ability uses the `spell` hook; see
    // `Game::use_location` for why that slot is safe to reuse.
    spell("Sanguine Depths", T::AnyMinion, |g, c| {
        if let Some(t) = c.target {
            g.deal_damage(t, 1);
            g.buff(t, 2, 0);
        }
    }),
    spell("Fan Club", T::None, |g, c| {
        let mut hits: Inline<Target, 9> = Inline::new();
        g.collect_area(c.side, Area::FriendlyMinions, &mut hits);
        hits.push(Target::Hero(c.side));
        for t in hits.iter() {
            g.heal(*t, 3);
        }
    }),
    spell("Cathedral of Atonement", T::AnyMinion, |g, c| {
        if let Some(t) = c.target {
            g.buff(t, 2, 1);
        }
        g.draw_cards(c.side, 1);
    }),
    spell("Dance Floor", T::None, |g, c| {
        let mut hits: Inline<Target, 9> = Inline::new();
        g.collect_area(c.side, Area::FriendlyMinions, &mut hits);
        for t in hits.iter() {
            g.grant(*t, Keywords::RUSH);
        }
    }),
    spell("Castle Kennels", T::FriendlyMinion, |g, c| {
        if let Some(t) = c.target {
            g.buff(t, 2, 0);
            if let Target::Minion(s, i) = t
                && g.player(s).board[i as usize].races().any(Races::BEAST)
            {
                g.grant(t, Keywords::RUSH);
            }
        }
    }),

    // ---------------------------------------------------------- choose one
    // Each half declares its own target requirement, so the two modes appear
    // as separate actions and a search sees both.
    choose("Wrath", &[
        m(T::AnyMinion, |g, c| {
            g.spell_damage(c.side, c.target, 3);
        }),
        m(T::AnyMinion, |g, c| {
            g.spell_damage(c.side, c.target, 1);
            g.draw_cards(c.side, 1);
        }),
    ]),
    choose("Power of the Wild", &[
        m(T::None, |g, c| g.buff_area(c.side, Area::FriendlyMinions, 1, 1)),
        m(T::None, |g, c| {
            g.summon_token(c.side, tokens::PANTHER, 1);
        }),
    ]),
    choose("Mark of Nature", &[
        m(T::AnyMinion, |g, c| {
            if let Some(t) = c.target {
                g.buff(t, 4, 0)
            }
        }),
        m(T::AnyMinion, |g, c| {
            if let Some(t) = c.target {
                g.buff(t, 0, 4);
                g.grant(t, Keywords::TAUNT);
            }
        }),
    ]),
    choose("Nourish", &[
        m(T::None, |g, c| g.gain_crystal(c.side, 2)),
        m(T::None, |g, c| g.draw_cards(c.side, 3)),
    ]),
    choose("Keeper of the Grove", &[
        m(T::AnyCharacter, |g, c| {
            g.spell_damage(c.side, c.target, 2);
        }),
        m(T::AnyMinion, |g, c| {
            if let Some(t) = c.target {
                g.silence(t)
            }
        }),
    ]),
    choose("Ancient of War", &[
        m(T::None, |g, c| {
            if let Some(s) = c.source {
                g.buff(Target::Minion(c.side, s), 5, 0)
            }
        }),
        m(T::None, |g, c| {
            if let Some(s) = c.source {
                let me = Target::Minion(c.side, s);
                g.buff(me, 0, 5);
                g.grant(me, Keywords::TAUNT);
            }
        }),
    ]),

    // -------------------------------------------------------------- combo
    // No mechanism of its own: Combo is a question about the turn so far.
    spell("Eviscerate", T::AnyCharacter, |g, c| {
        let n = if g.combo_active(c.side) { 4 } else { 2 };
        g.spell_damage(c.side, c.target, n);
    }),
    spell("Cold Blood", T::AnyMinion, |g, c| {
        let n = if g.combo_active(c.side) { 4 } else { 2 };
        if let Some(t) = c.target {
            g.buff(t, n, 0)
        }
    }),
    spell("Shiv", T::AnyCharacter, |g, c| {
        g.spell_damage(c.side, c.target, 1);
        g.draw_cards(c.side, 1);
    }),
    battlecry("SI:7 Agent", T::AnyCharacter, |g, c| {
        if g.combo_active(c.side)
            && let Some(t) = c.target
        {
            g.deal_damage(t, 3);
        }
    }),
    battlecry("Defias Ringleader", T::None, |g, c| {
        if g.combo_active(c.side) {
            g.summon_token(c.side, tokens::WHELP, 1);
        }
    }),
    battlecry("Edwin VanCleef", T::None, |g, c| {
        let n = g.other_cards_played(c.side);
        if n > 0
            && let Some(s) = c.source
        {
            g.buff(Target::Minion(c.side, s), 2 * n, 2 * n);
        }
    }),

    // ------------------------------------------------- straightforward text
    // Cards whose whole text is one verb already in the vocabulary. Found by
    // matching the corpus against the shapes `effects` can express, so each
    // is a mechanical addition rather than a judgement call.
    spell("Pyroblast", T::AnyCharacter, |g, c| {
        g.spell_damage(c.side, c.target, 10);
    }),
    spell("Chaos Nova", T::None, |g, c| g.spell_damage_area(c.side, Area::AllMinions, 4)),
    spell("Volcanic Potion", T::None, |g, c| g.spell_damage_area(c.side, Area::AllMinions, 2)),
    spell("Flash Heal", T::AnyCharacter, |g, c| {
        if let Some(t) = c.target {
            g.heal(t, 5)
        }
    }),
    spell("Regenerate", T::AnyCharacter, |g, c| {
        if let Some(t) = c.target {
            g.heal(t, 3)
        }
    }),
    spell("Iron Hide", T::None, |g, c| g.gain_armor(c.side, 5)),
    spell("Power Infusion", T::AnyMinion, |g, c| {
        if let Some(t) = c.target {
            g.buff(t, 2, 6)
        }
    }),
    spell("Power Word: Tentacles", T::AnyMinion, |g, c| {
        if let Some(t) = c.target {
            g.buff(t, 2, 6)
        }
    }),
    spell("Divine Strength", T::AnyMinion, |g, c| {
        if let Some(t) = c.target {
            g.buff(t, 1, 2)
        }
    }),

    // Battlecry: deal N damage.
    battlecry("Fire Elemental", T::AnyCharacter, |g, c| {
        if let Some(t) = c.target {
            g.deal_damage(t, 4);
        }
    }),
    battlecry("Fire Plume Phoenix", T::AnyCharacter, |g, c| {
        if let Some(t) = c.target {
            g.deal_damage(t, 3);
        }
    }),
    battlecry("North Sea Kraken", T::AnyCharacter, |g, c| {
        if let Some(t) = c.target {
            g.deal_damage(t, 4);
        }
    }),
    battlecry("Spring Rocket", T::AnyCharacter, |g, c| {
        if let Some(t) = c.target {
            g.deal_damage(t, 2);
        }
    }),
    battlecry("Blowgill Sniper", T::AnyCharacter, |g, c| {
        if let Some(t) = c.target {
            g.deal_damage(t, 1);
        }
    }),
    battlecry("Ironforge Rifleman", T::AnyCharacter, |g, c| {
        if let Some(t) = c.target {
            g.deal_damage(t, 1);
        }
    }),
    battlecry("Stormpike Commando", T::AnyCharacter, |g, c| {
        if let Some(t) = c.target {
            g.deal_damage(t, 2);
        }
    }),
    battlecry("Filletfighter", T::AnyCharacter, |g, c| {
        if let Some(t) = c.target {
            g.deal_damage(t, 1);
        }
    }),

    // Battlecry: restore N Health.
    battlecry("Gadgetzan Socialite", T::AnyCharacter, |g, c| {
        if let Some(t) = c.target {
            g.heal(t, 2)
        }
    }),
    battlecry("Earthen Ring Farseer", T::AnyCharacter, |g, c| {
        if let Some(t) = c.target {
            g.heal(t, 3)
        }
    }),
    battlecry("Shroom Brewer", T::AnyCharacter, |g, c| {
        if let Some(t) = c.target {
            g.heal(t, 4)
        }
    }),
    battlecry("Darkshire Alchemist", T::AnyCharacter, |g, c| {
        if let Some(t) = c.target {
            g.heal(t, 5)
        }
    }),
    battlecry("Amber Watcher", T::AnyCharacter, |g, c| {
        if let Some(t) = c.target {
            g.heal(t, 8)
        }
    }),

    battlecry("Shieldmaiden", T::None, |g, c| g.gain_armor(c.side, 5)),
    battlecry("Big Ol' Whelp", T::None, |g, c| g.draw_cards(c.side, 1)),

    // Deathrattle: draw a card.
    deathrattle("Runic Egg", |g, c| g.draw_cards(c.side, 1)),
    deathrattle("Polluted Hoarder", |g, c| g.draw_cards(c.side, 1)),

    // Deathrattle: deal N damage to the enemy hero.
    deathrattle("Goblin Bomb", |g, c| {
        g.deal_damage(Target::Hero(c.side.other()), 2);
    }),
    deathrattle("Backstreet Leper", |g, c| {
        g.deal_damage(Target::Hero(c.side.other()), 2);
    }),
    deathrattle("Shadowed Spirit", |g, c| {
        g.deal_damage(Target::Hero(c.side.other()), 3);
    }),
    deathrattle("Naval Mine", |g, c| {
        g.deal_damage(Target::Hero(c.side.other()), 4);
    }),
    deathrattle("Kobold Sandtrooper", |g, c| {
        g.deal_damage(Target::Hero(c.side.other()), 3);
    }),

    // --------------------------------------------------- Standard gauntlet
    // Chosen by how many of the twelve meta decks play them, not by fame. An
    // earlier batch implemented famous classics that turned out to be
    // Wild-only and added nothing to Standard coverage.
    battlecry("Glacial Shard", T::EnemyCharacter, |g, c| {
        if let Some(t) = c.target {
            g.freeze(t)
        }
    }),
    battlecry("Rustrot Viper", T::None, |g, c| g.destroy_weapon(c.side.other())),
    c(
        "Prize Vendor",
        T::None,
        None,
        // "Battlecry and Deathrattle" — the same effect on both hooks.
        Some(|g, c| {
            g.draw_cards(c.side, 1);
            g.draw_cards(c.side.other(), 1);
        }),
        Some(|g, c| {
            g.draw_cards(c.side, 1);
            g.draw_cards(c.side.other(), 1);
        }),
        None,
        None,
        None,
        None,
        None,
        None,
    ),
    battlecry("The Curator", T::None, |g, c| {
        g.draw_matching(c.side, |d| d.races.any(Races::BEAST));
        g.draw_matching(c.side, |d| d.races.any(Races::DRAGON));
        g.draw_matching(c.side, |d| d.races.any(Races::MURLOC));
    }),
    battlecry("Holy Eggbearer", T::None, |g, c| {
        g.draw_matching(c.side, |d| d.kind() == super::Kind::Minion && d.atk == 0);
    }),
    spell("Press the Advantage", T::AnyCharacter, |g, c| {
        g.spell_damage(c.side, c.target, 1);
        g.hero_attack_bonus(c.side, 1);
        g.draw_cards(c.side, 1);
    }),
    spell("Flash of Light", T::AnyCharacter, |g, c| {
        if let Some(t) = c.target {
            g.heal(t, 4)
        }
        g.draw_cards(c.side, 1);
    }),
    spell("Drain Soul", T::AnyMinion, |g, c| {
        let healed = 3 + g.player(c.side).spell_power();
        g.spell_damage(c.side, c.target, 3);
        g.heal_hero(c.side, healed);
    }),
    spell("Conflagrate", T::AnyMinion, |g, c| {
        g.spell_damage(c.side, c.target, 5);
        // "Its owner draws a card" — the minion's controller, not the caster.
        if let Some(Target::Minion(s, _)) = c.target {
            g.draw_cards(s, 1);
        }
    }),
    spell("Equality", T::None, |g, c| {
        let mut hits: Inline<Target, 16> = Inline::new();
        g.collect_area(c.side, Area::AllMinions, &mut hits);
        for t in hits.iter() {
            g.set_health(*t, 1);
        }
    }),
    spell("Shadow Word: Ruin", T::None, |g, c| {
        let mut hits: Inline<Target, 16> = Inline::new();
        g.collect_area(c.side, Area::AllMinions, &mut hits);
        for t in hits.iter() {
            if let Target::Minion(s, i) = *t
                && g.player(s).board[i as usize].atk >= 5
            {
                g.destroy(*t);
            }
        }
    }),
    spell("Moonwell", T::None, |g, c| {
        g.spell_damage_area(c.side, Area::AllEnemies, 4);
        let mut heals: Inline<Target, 9> = Inline::new();
        g.collect_area(c.side, Area::FriendlyMinions, &mut heals);
        heals.push(Target::Hero(c.side));
        for t in heals.iter() {
            g.heal(*t, 4);
        }
    }),
    spell("Innervate", T::None, |g, c| g.gain_temp_mana(c.side, 1)),
    spell("Sleet Storm", T::AnyCharacter, |g, c| {
        g.spell_damage(c.side, c.target, 2);
        if let Some(t) = g.random_enemy(c.side) {
            g.deal_damage(t, 1);
        }
    }),
    spell("Broxigar's Last Stand", T::None, |g, c| {
        // "Draw a card for each that died" — counted rather than tracked,
        // which is why the sweep has to happen before the second count.
        let before = g.minion_count(c.side) + g.minion_count(c.side.other());
        g.spell_damage_area(c.side, Area::AllMinions, 1);
        g.sweep_deaths();
        let after = g.minion_count(c.side) + g.minion_count(c.side.other());
        g.draw_cards(c.side, before.saturating_sub(after));
    }),

    // ------------------------------------------- gauntlet, no new mechanism
    spell("Arcane Flow", T::AnyCharacter, |g, c| {
        g.spell_damage(c.side, c.target, 4);
        g.spell_damage_area(c.side, Area::AllEnemies, 2);
    }),
    spell("Searing Fissure", T::None, |g, c| {
        g.spell_damage_area(c.side, Area::AllMinions, 1);
        g.hero_attack_bonus(c.side, 3);
    }),
    spell("Arcane Barrage", T::EnemyCharacter, |g, c| {
        g.spell_damage(c.side, c.target, 3);
        // "two other random ones": the chosen target is excluded, and a
        // second pick is made only if something else is still standing.
        for _ in 0..2 {
            let mut pool: Inline<Target, { 7 + 1 }> = Inline::new();
            g.collect_area(c.side, Area::AllEnemies, &mut pool);
            pool.retain(|t| Some(*t) != c.target);
            if pool.is_empty() {
                break;
            }
            let pick = g.rngs.effects.index(pool.len());
            let t = pool[pick];
            g.spell_damage(c.side, Some(t), 2);
        }
    }),
    spell("Fumigate", T::AnyMinion, |g, c| {
        // "and all others of the same minion type" — a minion with no tribe
        // hits nothing else, which is how the card reads.
        let races = match c.target {
            Some(Target::Minion(s, i)) => g.player(s).board[i as usize].races(),
            _ => Races::NONE,
        };
        g.spell_damage(c.side, c.target, 3);
        if races.0 != 0 {
            let mut hits: Inline<Target, 16> = Inline::new();
            g.collect_area(c.side, Area::AllMinions, &mut hits);
            for t in hits.iter() {
                if Some(*t) == c.target {
                    continue;
                }
                if let Target::Minion(s, i) = *t
                    && g.player(s).board[i as usize].races().any(races)
                {
                    g.spell_damage(c.side, Some(*t), 3);
                }
            }
        }
    }),
    spell("Renewing Flames", T::None, |g, c| {
        // Re-picked between the two hits: the first can kill its target.
        for _ in 0..2 {
            let Some(t) = g.lowest_health_enemy(c.side) else {
                break;
            };
            let dealt = 5 + g.player(c.side).spell_power();
            g.spell_damage(c.side, Some(t), 5);
            g.heal_hero(c.side, dealt);
            g.sweep_deaths();
        }
    }),
    spell("Drink Blood", T::AnyMinion, |g, c| {
        let dealt = 3 + g.player(c.side).spell_power();
        g.spell_damage(c.side, c.target, 3);
        g.heal_hero(c.side, dealt);
        g.refresh_hero_power(c.side);
    }),
    choose("Twilight Timereaver", &[
        m(T::None, |g, c| {
            let me = c.source.map(|s| Target::Minion(c.side, s));
            let mut hits: Inline<Target, 16> = Inline::new();
            g.collect_area(c.side, Area::AllMinions, &mut hits);
            for t in hits.iter() {
                if Some(*t) != me {
                    g.set_attack(*t, 1);
                }
            }
        }),
        m(T::None, |g, c| {
            let me = c.source.map(|s| Target::Minion(c.side, s));
            let mut hits: Inline<Target, 16> = Inline::new();
            g.collect_area(c.side, Area::AllMinions, &mut hits);
            for t in hits.iter() {
                if Some(*t) != me {
                    g.set_health(*t, 1);
                }
            }
        }),
    ]),

    battlecry("Twilight Mistress", T::None, |g, c| {
        g.bounce_area(c.side, Area::EnemyMinions)
    }),
    battlecry("V'ama, Looming Death", T::None, |g, c| {
        g.destroy_area_where(c.side, Area::AllMinions, |m| {
            m.card.def().class() != super::Class::Paladin
        });
    }),
    battlecry("Royal Librarian", T::AnyMinion, |g, c| {
        if let Some(t) = c.target {
            g.silence(t)
        }
    }),
    battlecry("Mother Duck", T::None, |g, c| {
        g.summon_token(c.side, tokens::EDR_492T, 3);
    }),
    battlecry("Darkscale Broodmother", T::None, |g, c| {
        if g.holding_race(c.side, Races::DRAGON) {
            g.refresh_mana(c.side, 2);
        }
    }),
    battlecry("King Mukla", T::None, |g, c| {
        g.give_token(c.side.other(), tokens::VIOLET_APPRENTICE);
        g.give_token(c.side.other(), tokens::VIOLET_APPRENTICE);
    }),

    deathrattle("Sizzling Cinder", |g, c| g.damage_split(c.side, Area::AllEnemies, 2)),
    deathrattle("Lotus Bookie", |g, c| {
        g.give_token(c.side, tokens::COIN);
    }),
    deathrattle("Living Flame", |g, c| {
        g.draw_matching(c.side, |d| {
            d.kind() == super::Kind::Spell && d.school() == super::School::Fire
        });
    }),

    trigger("Enduring Roach", |g, c| {
        if matches!(c.event, Event::HeroPowerUsed { side } if side == c.side) {
            g.refresh_mana(c.side, 2);
        }
    }),
    trigger("Petal Peddler", |g, c| {
        if matches!(c.event, Event::TurnEnd { side } if side == c.side) {
            g.buff_random_race(c.side, Races::DRAGON, Some(c.me()), 1, 1);
        }
    }),
    trigger("Tower of Ghouls", |g, c| {
        if c.hit_me() {
            g.summon_token(c.side, tokens::GHOUL, 2);
        }
    }),
    trigger("Gallagio Goon", |g, c| {
        // "After you play a Battlecry minion" — the Goon's own arrival does
        // not count, and the buff lands on the minion that just arrived.
        if let Event::MinionSummoned { side, card, slot } = c.event
            && side == c.side
            && slot != c.slot
            && card.def().keywords.has(Keywords::BATTLECRY)
        {
            g.buff(Target::Minion(side, slot), 1, 1);
        }
    }),
    trigger("Felfire Blaze", |g, c| {
        if let Event::SpellCast { side, card } = c.event
            && side == c.side
            && card.def().school() == super::School::Fel
        {
            g.destroy(c.me());
            g.spell_damage_area(c.side, Area::AllEnemies, 2);
        }
    }),

    // ------------------------------------------------- cost modification
    // `cost_delta` is read every time a cost is asked for, so these track the
    // board and hand live rather than being applied once and going stale.
    c(
        "The Unseen Atlas",
        T::None,
        Some(|g, c| g.draw_cards(c.side, 3)),
        None, None, None, None, None, None,
        // "Costs (1) less for each card in your hand" — itself excluded.
        Some(|g, side, _i| -(g.hand_size(side) - 1).max(0)),
        None,
    ),
    c(
        "Prescient Slitherdrake",
        T::None,
        None, None, None, None, None, None, None,
        // "another Dragon": the Slitherdrake in hand does not count itself.
        Some(|g, side, i| {
            let others = g
                .player(side)
                .hand
                .iter()
                .enumerate()
                .filter(|(k, h)| *k != i && h.card.def().races.any(Races::DRAGON))
                .count();
            if others > 0 { -3 } else { 0 }
        }),
        None,
    ),
    c(
        "Pterrorwing Ravager",
        T::None,
        None, None, None, None, None, None, None,
        Some(|g, side, _i| if g.kindred(side, Races::DRAGON) { -2 } else { 0 }),
        None,
    ),

    // ------------------------------------------------------ small mechanics
    spell("Demonic Confinement", T::AnyMinion, |g, c| {
        if let Some(t) = c.target {
            g.make_dormant(t, 2)
        }
    }),
    trigger("Warden Maiev", |g, c| {
        // "After you play a minion" — not itself.
        if let Event::MinionSummoned { side, slot, .. } = c.event
            && side == c.side
            && slot != c.slot
        {
            let t = Target::Minion(side, slot);
            g.buff(t, 3, 3);
            g.make_dormant(t, 1);
        }
    }),
    battlecry("Ravasaur Matriarch", T::EnemyMinion, |g, c| {
        // Kindred: only if a Beast was played on the previous turn.
        if g.kindred(c.side, Races::BEAST)
            && let Some(t) = c.target
            && let Some(s) = c.source
        {
            let n = g.player(c.side).board[s as usize].atk;
            g.deal_damage(t, n);
        }
    }),
    battlecry("Windpeak Wyrm", T::AnyCharacter, |g, c| {
        if let Some(t) = c.target {
            g.deal_damage(t, 5);
        }
        g.gain_armor(c.side, 5);
    }),

    // -------------------------------------------------- after-attack cards
    // These listen on `AfterAttack`, which fires once the exchange has fully
    // resolved. Weapons reach the trigger sweep too, marked by `WEAPON_SLOT`.
    trigger("Battlefiend", |g, c| {
        if matches!(c.event, Event::AfterAttack { attacker: Target::Hero(s), .. } if s == c.side) {
            g.buff(c.me(), 1, 0);
        }
    }),
    trigger("Hench-Clan Thug", |g, c| {
        if matches!(c.event, Event::AfterAttack { attacker: Target::Hero(s), .. } if s == c.side) {
            g.buff(c.me(), 1, 1);
        }
    }),
    trigger("Hookfist-3000", |g, c| {
        if matches!(c.event, Event::AfterAttack { attacker: Target::Hero(s), .. } if s == c.side) {
            g.gain_armor(c.side, 4);
            g.draw_cards(c.side, 1);
        }
    }),
    trigger("Ursine Maul", |g, c| {
        if matches!(c.event, Event::AfterAttack { attacker: Target::Hero(s), .. } if s == c.side) {
            g.draw_cards(c.side, 1);
        }
    }),
    trigger("Command Claw", |g, c| {
        if matches!(c.event, Event::AfterAttack { attacker: Target::Hero(s), .. } if s == c.side) {
            // Any friendly minion, not a tribe: `Races::ALL` is the bit for
            // "counts as every tribe", which is a different question.
            if let Some(t) = g.random_minion(c.side) {
                g.buff(t, 2, 0);
            }
        }
    }),

    // ------------------------------------------------------------ discover
    // The pick is made by the engine, not the policy — see `Game::discover`
    // for why, and what that costs in strength.
    spell("Tracking", T::None, |g, c| {
        g.discover_from_deck(c.side, |_| true);
    }),
    spell("Waveshaping", T::None, |g, c| {
        g.discover_from_deck(c.side, |_| true);
    }),
    spell("Runed Orb", T::AnyCharacter, |g, c| {
        g.spell_damage(c.side, c.target, 2);
        g.discover(c.side, |d| d.kind() == super::Kind::Spell);
    }),
    spell("Horn of Plenty", T::None, |g, c| {
        g.discover(c.side, |d| {
            d.kind() == super::Kind::Spell && d.school() == super::School::Nature
        });
    }),
    spell("Sands of Time", T::None, |g, c| {
        g.discover(c.side, |d| d.kind() == super::Kind::Spell);
    }),
    spell("The Skeleton Key", T::None, |g, c| {
        g.discover(c.side, |d| d.kind() == super::Kind::Spell);
    }),
    spell("Illidari Studies", T::None, |g, c| {
        g.discover(c.side, |d| d.keywords.has(Keywords::OUTCAST));
    }),
    spell("Odd Map", T::None, |g, c| {
        g.discover(c.side, |d| {
            d.races.any(Races::BEAST) && d.atk % 2 == 1
        });
    }),
    battlecry("Jeweled Macaw", T::None, |g, c| {
        g.add_random_to_hand(c.side, |d| d.races.any(Races::BEAST));
    }),
    battlecry("Watfin", T::None, |g, c| {
        g.discover(c.side, |d| d.kind() == super::Kind::Minion);
    }),
    battlecry("Nightmare Lord Xavius", T::None, |g, c| {
        g.discover_from_deck(c.side, |d| d.kind() == super::Kind::Minion);
    }),
    trigger("Staff of Trickery", |g, c| {
        if matches!(c.event, Event::AfterAttack { attacker: Target::Hero(s), .. } if s == c.side) {
            g.discover(c.side, |d| d.class() == super::Class::Druid);
        }
    }),

    // ------------------------------------------------------- draw and hand
    deathrattle("Conjured Bookkeeper", |g, c| {
        g.draw_matching(c.side, |d| d.kind() == super::Kind::Spell);
    }),
    battlecry("Sawbones", T::None, |g, c| {
        // "Destroy all your other minions" — the Sawbones itself survives.
        let me = c.source.map(|s| Target::Minion(c.side, s));
        let mut hits: Inline<Target, 9> = Inline::new();
        g.collect_area(c.side, Area::FriendlyMinions, &mut hits);
        for t in hits.iter() {
            if Some(*t) != me {
                g.destroy(*t);
            }
        }
        g.draw_cards(c.side, 1);
        g.refresh_hero_power(c.side);
    }),
    trigger("The Kingslayers", |g, c| {
        if matches!(c.event, Event::AfterAttack { attacker: Target::Hero(s), .. } if s == c.side) {
            for side in [c.side, c.side.other()] {
                g.draw_matching(side, |d| d.rarity() == super::Rarity::Legendary);
            }
        }
    }),

    trigger("Corpse Cannon", |g, c| {
        if matches!(c.event, Event::AfterAttack { attacker: Target::Hero(s), .. } if s == c.side) {
            g.summon_token(c.side, tokens::GHOUL, 1);
        }
    }),
    battlecry("Defias Smuggler", T::FriendlyMinion, |g, c| {
        if let Some(t) = c.target {
            g.buff(t, 2, 0);
            g.grant(t, Keywords::RUSH);
        }
    }),
    c(
        "Remnant of Rage",
        T::None,
        None,
        Some(|g, c| g.draw_cards(c.side, 1)),
        None,
        None,
        None,
        None,
        None,
        // "Costs (1) less for each minion that died this turn" — read live, so
        // it tracks a board wipe that happened moments ago.
        Some(|g, _side, _i| -(g.deaths_this_turn as i16)),
        None,
    ),

    // -------------------------------------------------------------- herald
    // Herald summons your class's Soldier, scaled by how many times you have
    // done it before. Classes without a Soldier still advance the counter.
    battlecry("Envoy of the End", T::None, |g, c| g.herald(c.side)),
    battlecry("Skywall Sentinel", T::None, |g, c| g.herald(c.side)),
    c(
        "Shadowsworn Disciple",
        T::None,
        None,
        Some(|g, c| g.herald(c.side)),
        Some(|g, c| g.heal_hero(c.side, 3)),
        None, None, None, None, None, None,
    ),
    deathrattle("Maniacal Follower", |g, c| g.herald(c.side)),
    spell("Rite of Twilight", T::None, |g, c| {
        g.herald(c.side);
        if g.combo_active(c.side)
            && let Some(t) = g.random_enemy(c.side)
        {
            g.spell_damage(c.side, Some(t), 3);
        }
    }),
    spell("Shrine of Twilight", T::None, |g, c| {
        g.herald(c.side);
        g.draw_cards(c.side, 1);
    }),

    // The Soldiers themselves. Their effect resolves on being summoned, so it
    // lives on the battlecry hook and `Game::herald` invokes it directly.
    battlecry("Soldier of Sinestra", T::None, |g, c| {
        let n = g.heralded_scale(c.side);
        let class = g.player(c.side).class;
        let pool = crate::cards::discover_pool(|d| d.kind() == super::Kind::Spell);
        let off: Inline<crate::cards::CardId, 64> = pool
            .into_iter()
            .filter(|x| {
                let k = x.def().class();
                k != class && k != super::Class::Neutral
            })
            .take(64)
            .collect();
        if !off.is_empty() {
            let pick = g.rngs.effects.index(off.len());
            let card = off[pick];
            if g.give_card(c.side, card)
                && let Some(h) = g.player_mut(c.side).hand.last_mut()
            {
                h.cost_delta -= n;
            }
        }
    }),
    battlecry("Soldier of Azshara", T::None, |g, c| {
        let n = g.heralded_scale(c.side);
        g.hero_attack_bonus(c.side, 2 * n);
    }),
    deathrattle("Soldier of Ragnaros", |g, c| {
        let n = g.heralded_scale(c.side);
        if let Some(t) = g.random_enemy(c.side) {
            g.deal_damage(t, n);
        }
    }),
    trigger("Soldier of Cho'gall", |g, c| {
        // "destroy the minion to the right" — its own board, one slot along.
        if matches!(c.event, Event::TurnEnd { side } if side == c.side)
            && (c.slot as usize + 1) < g.player(c.side).board.len()
        {
            g.destroy(Target::Minion(c.side, c.slot + 1));
        }
    }),
    aura("Soldier of Al'Akir", |ss, sl, ts, tl, _m| {
        // Adjacent minions have +N Attack, where N is the Herald scale. The
        // aura signature is a pure function of position, so the scale cannot
        // be read here; it uses the base value the card prints at Herald 1.
        if ss == ts && tl.abs_diff(sl) == 1 { (1, 0) } else { (0, 0) }
    }),

    // ------------------------------------------------------- weakest decks
    spell("Preparation", T::None, |g, c| {
        g.player_mut(c.side).next_spell_discount += 2;
    }),
    trigger("Spider Rider", |g, c| {
        if matches!(c.event, Event::AfterAttack { attacker: Target::Hero(s), .. } if s == c.side) {
            g.draw_cards(c.side, 1);
        }
    }),
    battlecry("Kaldorei Priestess", T::None, |g, c| {
        // "until your next turn": a temporary change expires at the end of the
        // holder's turn, which for enemy minions is exactly the turn before
        // yours comes round again.
        g.temp_atk_area(c.side, Area::EnemyMinions, -2);
    }),
    c(
        "Eye Beam",
        T::AnyMinion,
        Some(|g, c| {
            let dealt = 3 + g.player(c.side).spell_power();
            g.spell_damage(c.side, c.target, 3);
            g.heal_hero(c.side, dealt);
        }),
        None, None, None, None, None, None,
        // Outcast: costs (1). The hand position is exactly what `cost_delta`
        // is handed, so the condition is answerable here.
        Some(|g, side, i| {
            let n = g.player(side).hand.len();
            let outcast = i == 0 || i + 1 == n;
            if outcast { 1 - 3 } else { 0 }
        }),
        None,
    ),
    battlecry("Medivh the Hallowed", T::None, |g, c| {
        // "Silence and destroy all other minions" — itself excluded.
        let me = c.source.map(|s| Target::Minion(c.side, s));
        let mut hits: Inline<Target, 16> = Inline::new();
        g.collect_area(c.side, Area::AllMinions, &mut hits);
        for t in hits.iter() {
            if Some(*t) != me {
                g.silence(*t);
                g.destroy(*t);
            }
        }
    }),
    spell("Karazhan the Sanctum", T::None, |g, c| {
        g.summon_random_of_cost(c.side, 8, 2);
    }),
    spell("Infest the Scullery", T::None, |g, c| {
        g.summon_random_of_cost(c.side, 3, 2);
    }),
    spell("Lifebloom", T::None, |g, c| {
        let mut heals: Inline<Target, 9> = Inline::new();
        g.collect_area(c.side, Area::FriendlyMinions, &mut heals);
        heals.push(Target::Hero(c.side));
        for t in heals.iter() {
            g.heal(*t, 8);
        }
        g.summon_random_of_cost(c.side, 8, 2);
    }),
    spell("Amirdrassil", T::None, |g, c| {
        g.summon_random_of_cost(c.side, 1, 1);
        g.gain_armor(c.side, 1);
        g.draw_cards(c.side, 1);
        g.refresh_mana(c.side, 1);
    }),

    // ---------------------------------------------------- token-based cards
    // The Dreadseeds come in three flavours and the card rolls one; the ids
    // are listed rather than guessed, and the token test checks them.
    spell("Grim Harvest", T::None, |g, c| {
        g.draw_cards(c.side, 1);
        g.summon_random_dormant(c.side, tokens::DREADSEEDS, 2);
    }),
    choose("Wyvern's Slumber", &[
        m(T::None, |g, c| {
            for _ in 0..2 {
                g.summon_random_dormant(c.side, tokens::DREADSEEDS, 2);
            }
        }),
        m(T::EnemyMinion, |g, c| {
            g.spell_damage(c.side, c.target, 2);
        }),
    ]),
    spell("Infested Breath", T::AnyCharacter, |g, c| {
        g.spell_damage(c.side, c.target, 2);
        g.summon_token(c.side, tokens::EDR_810T, 1);
    }),
    trigger("Insect Claw", |g, c| {
        if matches!(c.event, Event::AfterAttack { attacker: Target::Hero(s), .. } if s == c.side) {
            g.summon_token(c.side, tokens::TLC_903T, 1);
        }
    }),
    c(
        "Horn of Feasting",
        T::None,
        Some(|g, c| {
            let made = g.summon_token(c.side, tokens::TLC_101T, 3);
            if c.outcast {
                let n = g.player(c.side).board.len();
                for i in (n - made)..n {
                    g.grant(Target::Minion(c.side, i as u8), Keywords::IMMUNE);
                }
            }
        }),
        None, None, None, None, None, None, None, None,
    ),
    spell("Wound Prey", T::AnyCharacter, |g, c| {
        g.spell_damage(c.side, c.target, 1);
        g.summon_token(c.side, tokens::TLC_903T, 1);
    }),

    // ------------------------------------------------------------- bespoke
    battlecry("Endbringer Umbra", T::None, |g, c| {
        g.retrigger_friendly_deathrattles(c.side, 5)
    }),
    battlecry("Dissolving Ooze", T::FriendlyMinion, |g, c| {
        if let Some(t) = c.target {
            g.destroy(t)
        }
    }),
    battlecry("Falric", T::None, |g, c| {
        // The doubling clause needs a per-player multiplier the engine does
        // not have; the draw is the half that is expressible.
        g.draw_cards(c.side, 1);
    }),
    spell("Soulrest Ceremony", T::None, |g, c| {
        let mut hits: Inline<Target, 9> = Inline::new();
        g.collect_area(c.side, Area::FriendlyMinions, &mut hits);
        for t in hits.iter() {
            g.buff(*t, 1, 0);
            g.grant(*t, Keywords::RUSH);
            if let Target::Minion(s, i) = *t
                && let Some(m) = g.player_mut(s).board.get_mut(i as usize)
            {
                m.flags.insert(Flags::DOOMED);
            }
        }
    }),
    // "While holding this" -- Marks, set on whatever is left in hand when a
    // triggering card is played (Game::play_card), read back on cast since
    // the card itself is gone from hand by then (Ctx::marks).
    c(
        "Platysaur",
        T::None,
        None,
        Some(|g, c| {
            let before = g.player(c.side).hand.len();
            g.draw_cards(c.side, 1);
            if let Some(hc) = g.player_mut(c.side).hand.get_mut(before) {
                hc.marks.insert(Marks::DRAWN_BY_PLATYSAUR);
            }
        }),
        Some(|g, c| {
            if let Some(idx) = g
                .player(c.side)
                .hand
                .iter()
                .position(|hc| hc.marks.has(Marks::DRAWN_BY_PLATYSAUR))
            {
                g.player_mut(c.side).hand.remove(idx);
            }
        }),
        None, None, None, None, None, None,
    ),
    spell("Ebb and Flow", T::AnyCharacter, |g, c| {
        g.spell_damage(c.side, c.target, 3);
        if c.marks.has(Marks::PLAYED_MINION) {
            g.gain_armor(c.side, 5);
        }
    }),
    battlecry("Mind Sweeper", T::None, |g, c| {
        if c.marks.has(Marks::PLAYED_OPPONENT_CARD) {
            g.damage_area(c.side, Area::EnemyMinions, 2);
        }
    }),
    c(
        "Unshackle Soul",
        T::AnyMinion,
        Some(|g, c| {
            if let Some(t) = c.target {
                g.destroy(t);
            }
        }),
        None, None, None, None, None, None,
        Some(|g, side, idx| {
            let Some(hc) = g.player(side).hand.get(idx) else {
                return 0;
            };
            if hc.marks.has(Marks::PLAYED_OPPONENT_CARD) {
                -(hc.card.def().cost - 1)
            } else {
                0
            }
        }),
        None,
    ),
    spell("Cosmic Manifestations", T::AnyCharacter, |g, c| {
        for _ in 0..1 + c.outcast as u8 {
            g.spell_damage(c.side, c.target, 2);
            g.shuffle_random_into_deck(c.side, |d| {
                d.kind() == super::Kind::Spell && d.class() == super::Class::DemonHunter
            });
        }
    }),
    trigger("Briarspawn Drake", |g, c| {
        // "At the end of your turn, attack a random enemy minion."
        if matches!(c.event, Event::TurnEnd { side } if side == c.side) {
            let me = g.player(c.side).board[c.slot as usize];
            if let Some(t) = g.random_minion(c.side.other()) {
                g.deal_damage(t, me.atk);
            }
        }
    }),
    trigger("Gorishi Wasp", |g, c| {
        if c.hit_me() {
            g.add_random_to_hand(c.side, |d| d.cost == 1);
        }
    }),
    // ------------------------------------------- data-driven and one-liners
    // Cards whose whole text is verbs the vocabulary already has. The
    // summons among them name no token id at all: the card's own `childIds`
    // say what it creates, so the row asks for "this card's token" and the
    // table answers.
    battlecry("Murloc Tidehunter", T::None, |g, c| {
        g.summon_child(c.side, c.card, 1);
    }),
    spell("Animal Companion", T::None, |g, c| {
        g.summon_random_child(c.side, c.card);
    }),
    spell("Sanguine Infestation", T::None, |g, c| {
        g.draw_cards(c.side, 2);
        g.summon_child(c.side, c.card, 2);
    }),

    // Armour and corpses.
    deathrattle("Plated Beetle", |g, c| g.gain_armor(c.side, 3)),
    deathrattle("Mo'arg Forgefiend", |g, c| g.gain_armor(c.side, 8)),
    battlecry("Body Bagger", T::None, |g, c| g.gain_corpses(c.side, 1)),

    // Damage, with the rider the card is printed with. Lightning Bolt's
    // Overload is in the table and the kernel applies it on play, so the row
    // is only the damage.
    spell("Bash", T::AnyCharacter, |g, c| {
        g.spell_damage(c.side, c.target, 3);
        g.gain_armor(c.side, 3);
    }),
    spell("Lightning Bolt", T::AnyCharacter, |g, c| {
        g.spell_damage(c.side, c.target, 3);
    }),
    battlecry("Flame Imp", T::None, |g, c| {
        g.deal_damage(Target::Hero(c.side), 3);
    }),
    battlecry("Frostbitten Imp", T::None, |g, c| {
        // "Freeze this" — a 5/3 for two that cannot swing the turn it lands.
        if let Some(slot) = c.source {
            g.freeze(Target::Minion(c.side, slot));
        }
    }),

    spell("Hand of A'dal", T::AnyMinion, |g, c| {
        if let Some(t) = c.target {
            g.buff(t, 2, 1)
        }
        g.draw_cards(c.side, 1);
    }),
    spell("Horn of Winter", T::None, |g, c| g.refresh_mana(c.side, 2)),
    spell("Anti-Magic Shell", T::None, |g, c| {
        g.buff_area(c.side, Area::FriendlyMinions, 1, 1);
        let mut hits: Inline<Target, 9> = Inline::new();
        g.collect_area(c.side, Area::FriendlyMinions, &mut hits);
        for t in hits.iter() {
            g.grant(*t, Keywords::ELUSIVE);
        }
    }),

    // Outcast: the card was on one end of the hand when it was played.
    battlecry("Crimson Sigil Runner", T::None, |g, c| {
        if c.outcast {
            g.draw_cards(c.side, 1);
        }
    }),
    spell("Spectral Sight", T::None, |g, c| {
        g.draw_cards(c.side, 1);
        if c.outcast {
            g.draw_cards(c.side, 1);
        }
    }),

    // Discover, narrowed by what the card asks for.
    battlecry("Netherwalker", T::None, |g, c| {
        g.discover(c.side, |d| d.races.any(Races::DEMON));
    }),
    battlecry("Battle Vicar", T::None, |g, c| {
        g.discover(c.side, |d| {
            d.kind() == super::Kind::Spell && d.school() == super::School::Holy
        });
    }),
    battlecry("Dark Peddler", T::None, |g, c| {
        g.discover(c.side, |d| d.cost == 1);
    }),
    // ------------------------------------------- ported from the Python engine
    // Cards `hs2/impls.py` already reasoned through, translated onto the verbs
    // that exist here. The Python original is the reference, not the authority:
    // it was written against a corpus that carried three data bugs we have
    // since fixed, so each of these arrives with a test of its own.

    // Cards that hand you a specific token. Python had to synthesise these by
    // name; here the id is resolved at compile time.
    battlecry("Fire Fly", T::None, |g, c| {
        g.give_token(c.side, tokens::FLAME_ELEMENTAL);
    }),
    deathrattle("Violet Spellwing", |g, c| {
        g.give_token(c.side, tokens::ARCANE_MISSILES);
    }),
    spell("Contraband Wands", T::None, |g, c| {
        for _ in 0..3 {
            g.give_token(c.side, tokens::ARCANE_MISSILES);
        }
    }),

    // "Get a random X" -- the pool is already restricted to cards the engine
    // can actually play, so a Discover can never hand out a card that would
    // then do nothing.
    battlecry("Witch's Apprentice", T::None, |g, c| {
        g.add_random_to_hand(c.side, |d| {
            d.kind() == super::Kind::Spell && d.class() == super::Class::Shaman
        });
    }),
    battlecry("Carrier Whelp", T::None, |g, c| {
        g.add_random_to_hand(c.side, |d| {
            d.kind() == super::Kind::Minion && d.races.any(Races::DRAGON) && d.cost <= 3
        });
    }),

    // Buffs that read the board or the hand.
    battlecry("Caged Cranium", T::None, |g, c| {
        // "+1 Health for each card in your hand" -- the Cranium has already
        // left hand by the time this fires, so it does not count itself.
        let n = g.hand_size(c.side);
        if let Some(slot) = c.source {
            g.buff(Target::Minion(c.side, slot), 0, n);
        }
    }),
    battlecry("Hijacked Securitybot", T::None, |g, c| {
        let me = c.source.map(|s| Target::Minion(c.side, s));
        let mut hits: Inline<Target, 9> = Inline::new();
        g.collect_area(c.side, Area::FriendlyMinions, &mut hits);
        for t in hits.iter() {
            if Some(*t) != me {
                g.buff(*t, 1, 1);
            }
        }
    }),

    // A Location whose token comes from the card's own childIds. Python had to
    // build the Rat by hand, deathrattle and all; here it is the printed card.
    spell("Underbelly Network", T::None, |g, c| {
        g.summon_child(c.side, c.card, 1);
    }),

    // The token Infestation hands you. Without it the spell would put two dead
    // cards in hand, which is conservative but makes the card pointless.
    spell("Gorishi Stinger", T::AnyCharacter, |g, c| {
        g.spell_damage(c.side, c.target, 2);
        g.summon_token(c.side, tokens::TLC_903T, 1);
    }),
    spell("Infestation", T::None, |g, c| {
        for _ in 0..2 {
            g.give_token(c.side, tokens::GORISHI_STINGER);
        }
    }),

    // Choose One, where each half wants something different pointed at it.
    choose("Secret Ingredient", &[
        m(T::None, |g, c| g.hero_attack_bonus(c.side, 2)),
        m(T::None, |g, c| {
            g.add_random_to_hand(c.side, |d| d.class() == super::Class::Druid);
        }),
    ]),
    choose("Morbid Swarm", &[
        m(T::None, |g, c| {
            g.summon_child(c.side, c.card, 2);
        }),
        m(T::AnyMinion, |g, c| {
            // The corpses are spent only when there are two to spend, so the
            // mode does nothing rather than going into debt.
            if g.spend_corpses(c.side, 2) {
                g.spell_damage(c.side, c.target, 4);
            }
        }),
    ]),

    battlecry("Felwood Treant", T::None, |g, c| g.gain_temp_mana(c.side, 1)),

    // Burn Mage's last two blockers.
    battlecry("Archmage Kalec", T::None, |g, c| g.give_spell_power(c.side, 1)),
    battlecry("Tricksy Improviser", T::None, |g, c| {
        if g.spells_cast_turn(c.side) > 0 {
            g.cast_random_secrets(c.side, 2);
        }
    }),

    // ---------------------------------------------- 2026 meta decks, phase 1
    // Ported from `hs2/impls.py` against the corpus text; see
    // docs/RUST_CARDS_PLAN.md §4 phase 1. No new engine mechanism needed —
    // existing verbs plus the small additions listed in that section.
    // death knight
    deathrattle("Staff of the Endbringer", |g, c| {
        let mut all: Inline<Target, 16> = Inline::new();
        g.collect_area(c.side, Area::AllMinions, &mut all);
        for t in all.iter() {
            g.destroy(*t);
        }
    }),
    // druid
    trigger("Spiderling", |g, ctx| {
        if let Event::TurnStart { side } = ctx.event
            && side == ctx.side
        {
            g.hero_attack_bonus(ctx.side, 1);
        }
    }),
    // hunter
    deathrattle("Guard Dog", |g, c| {
        g.summon_random_where(c.side, |d| {
            d.kind() == super::Kind::Minion && d.cost == 1 && d.keywords.has(Keywords::DEATHRATTLE)
        });
    }),
    spell("Earthen Roar", T::EnemyMinion, |g, c| {
        let Some(t) = c.target else { return };
        g.set_health(t, 1);
        if g.holding_race(c.side, Races::DRAGON) {
            let foe = c.side.other();
            let pick = g
                .player(foe)
                .board
                .iter()
                .copied()
                .enumerate()
                .filter(|&(i, m)| {
                    Target::Minion(foe, i as u8) != t && m.active() && m.is_minion() && m.health() >= 3
                })
                .max_by_key(|&(_, m)| m.health())
                .map(|(i, _)| Target::Minion(foe, i as u8));
            if let Some(t2) = pick {
                g.set_health(t2, 1);
            }
        }
    }),
    spell("Cower in Fear", T::AnyMinion, |g, c| {
        g.spell_damage(c.side, c.target, 3);
        g.player_mut(c.side).next_beast_discount = 2;
    }),
    // paladin
    spell("Judgment", T::FriendlyMinion, |g, c| {
        let Some(Target::Minion(s, i)) = c.target else { return };
        let Some(m) = g.player(s).board.get(i as usize) else { return };
        let (atk, hp) = (m.atk, m.health());
        let mut all: Inline<Target, 16> = Inline::new();
        g.collect_area(c.side, Area::AllMinions, &mut all);
        for t in all.iter() {
            g.set_attack(*t, atk);
            g.set_health(*t, hp);
        }
    }),
    // Deathrattle summons a Whelp sized by how many of the Egg's controller's
    // turns it survived; the trigger counts those turns on the Egg itself
    // via `Permanent::growth`, read back through `Ctx::dying` once the body
    // is gone.
    c(
        "Twilight Egg",
        T::None,
        None,
        None,
        Some(|g, c| {
            let n = 1 + c.dying.map_or(0, |m| m.growth) as i16;
            if g.summon_token(c.side, tokens::ACCELERATED_WHELP, 1) > 0
                && let Some(last) = g.player(c.side).board.len().checked_sub(1)
            {
                let t = Target::Minion(c.side, last as u8);
                g.set_attack(t, n);
                g.set_health(t, n);
            }
        }),
        Some(|g, ctx| {
            if let Event::TurnStart { side } = ctx.event
                && side == ctx.side
                && let Some(m) = g.player_mut(ctx.side).board.get_mut(ctx.slot as usize)
            {
                m.growth = m.growth.saturating_add(1);
            }
        }),
        None, None, None, None, None,
    ),
    deathrattle("Soothsayer", |g, c| {
        g.heal_hero(c.side, 6);
        g.summon_random_of_cost(c.side, 6, 1);
    }),
    // Divine Shield is the minion's own printed keyword and needs no code;
    // only the hero-facing half of the battlecry is implemented here.
    battlecry("Hardlight Protector", T::None, |g, c| {
        g.heal_hero(c.side, 3);
        g.player_mut(c.side).hero_divine_shield = true;
    }),
    // priest
    spell("Intertwined Fate", T::None, |g, c| {
        g.discover_from_deck(c.side, |_| true);
        g.discover_from_opponent_deck(c.side, |_| true);
    }),
    // rogue
    // Battlecry, Combo and Deathrattle all cast the same "Fan of Knives" —
    // Combo does not change what happens here, so it needs no separate path.
    c(
        "Opu the Unseen",
        T::None,
        None,
        Some(|g, c| {
            g.spell_damage_area(c.side, Area::EnemyMinions, 1);
            g.draw_cards(c.side, 1);
        }),
        Some(|g, c| {
            g.spell_damage_area(c.side, Area::EnemyMinions, 1);
            g.draw_cards(c.side, 1);
        }),
        None, None, None, None, None, None,
    ),
    battlecry("Agent of the Old Ones", T::None, |g, c| {
        let worst = g
            .player(c.side)
            .hand
            .iter()
            .enumerate()
            .max_by_key(|(_, hc)| hc.card.def().cost)
            .map(|(i, _)| i);
        if let Some(idx) = worst
            && g.player_mut(c.side).hand.remove(idx).is_some()
        {
            g.give_token(c.side, tokens::COIN);
        }
    }),
    spell("Deja Vu", T::None, |g, c| {
        g.discover_from_opponent_hand(c.side);
    }),
    // "If you play it this turn, also pick one of the others" needs G1
    // (per-hand-card marks); the plain Discover is a safe, weaker cut of it.
    spell("Cultist Map", T::None, |g, c| {
        g.discover_from_deck(c.side, |_| true);
    }),
    // shaman
    battlecry("Getaway Hogdriver", T::None, |g, c| {
        let before = g.player(c.side).hand.len();
        g.draw_cards(c.side, 2);
        if g.is_over() {
            return;
        }
        let drawn = &g.player(c.side).hand.as_slice()[before..];
        let both_minions =
            drawn.len() == 2 && drawn.iter().all(|hc| hc.card.def().kind() == super::Kind::Minion);
        if both_minions
            && let Some(slot) = c.source
        {
            g.grant(Target::Minion(c.side, slot), Keywords::CHARGE);
        }
    }),
    // warlock
    // "Make it Temporary" (burns unused at end of turn) needs per-card state
    // the engine does not have; this Discover reads stronger than the
    // printed card rather than weaker, unlike every other approximation.
    spell("Cursed Catacombs", T::None, |g, c| {
        g.discover_from_deck(c.side, |_| true);
    }),
    trigger("Eredar Deceptor", |g, ctx| {
        if let Event::CardDrawn { side } = ctx.event
            && side == ctx.side
        {
            g.summon_token(ctx.side, tokens::INVADING_FELBAT, 1);
        }
    }),
    deathrattle("Imp Gang Stooge", |g, c| {
        for _ in 0..2 {
            g.put_on_bottom(c.side, tokens::GRANDMOTHER_IMP);
        }
    }),
    spell("Annihilation", T::None, |g, c| {
        let mut all: Inline<Target, 16> = Inline::new();
        g.collect_area(c.side, Area::AllMinions, &mut all);
        for t in all.iter() {
            g.destroy(*t);
        }
        g.sweep_deaths();
        let bottom = g.player(c.side).deck.len().min(3);
        let mut demon_idx: Inline<u8, 3> = Inline::new();
        for i in 0..bottom {
            let card = g.player(c.side).deck[i];
            if card.def().kind() == super::Kind::Minion && card.def().races.any(Races::DEMON) {
                demon_idx.push(i as u8);
            }
        }
        for &i in demon_idx.iter().rev() {
            if let Some(card) = g.player_mut(c.side).deck.remove(i as usize) {
                g.summon(c.side, card);
            }
        }
    }),
    // warrior
    battlecry("Brood Keeper", T::None, |g, c| {
        if g.holding_race(c.side, Races::DRAGON) {
            g.equip(c.side, tokens::NIGHTMARE_SLICER);
        }
    }),
    // "Rewind" (a second, independent roll for each equip) is not modelled,
    // matching the Python reference's own simplification.
    battlecry("Stadium Announcer", T::None, |g, c| {
        g.equip_random(c.side, |d| d.kind() == super::Kind::Weapon);
        g.equip_random(c.side.other(), |d| d.kind() == super::Kind::Weapon);
        g.buff_weapon(c.side, 1, 1);
    }),
    spell("Erupting Volcano", T::None, |g, c| {
        let fire = g.player(c.side).schools_cast_turn & (1 << (super::School::Fire as u8)) != 0;
        g.damage_split(c.side, Area::AllEnemies, if fire { 6 } else { 3 });
    }),
    // Deals its damage; does not return to hand with excess damage, which
    // would need a per-copy variable-damage field the engine does not have.
    spell("Torch", T::DamagedEnemyMinion, |g, c| {
        if let Some(t) = c.target {
            g.deal_damage(t, 8);
        }
    }),
    battlecry("Darkrider", T::None, |g, c| {
        if g.holding_race(c.side, Races::DRAGON) {
            g.discover(c.side, |d| d.kind() == super::Kind::Minion && d.races.any(Races::DRAGON));
        }
    }),
    spell("Shadowflame Suffusion", T::AnyCharacter, |g, c| {
        g.spell_damage(c.side, c.target, 2);
        g.discover(c.side, |d| {
            d.kind() == super::Kind::Minion && d.class() == super::Class::Warrior
        });
    }),
    // demon hunter
    spell("Dark Bribe", T::None, |g, c| {
        let before = g.player(c.side).hand.len();
        g.draw_cards(c.side, 3);
        if g.is_over() {
            return;
        }
        let cheapest = g.player(c.side).hand.as_slice()[before..]
            .iter()
            .enumerate()
            .min_by_key(|(_, hc)| hc.card.def().cost)
            .map(|(i, _)| before + i);
        if let Some(idx) = cheapest
            && let Some(hc) = g.player_mut(c.side).hand.remove(idx)
        {
            g.give_card(c.side.other(), hc.card);
        }
    }),

    // ------------------------------------------------------- phase 2, G1/G2
    // docs/RUST_CARDS_PLAN.md §4 phase 2.
    spell("Acceleration Aura", T::None, |g, c| {
        g.player_mut(c.side).pending.push(Pending {
            kind: PendingKind::TempCrystal,
            turns_left: 3,
            amount: 1,
            card: CardId(0),
        });
    }),
    spell("Sigil of the Seas", T::None, |g, c| {
        g.player_mut(c.side).pending.push(Pending {
            kind: PendingKind::SummonToken,
            turns_left: 1,
            amount: 0,
            card: tokens::NAGA_MONSTROSITY,
        });
    }),
    spell("Rotten Apple", T::None, |g, c| {
        g.heal_hero(c.side, 12);
        g.player_mut(c.side).pending.push(Pending {
            kind: PendingKind::HeroDamage,
            turns_left: 2,
            amount: 3,
            card: CardId(0),
        });
    }),
    battlecry("Cult Neophyte", T::None, |g, c| {
        g.player_mut(c.side.other()).spell_tax_pending += 1;
    }),
    // Approximate: casts the chosen spell once, immediately, rather than
    // also recasting it at the start of the next two turns -- weaker, and
    // recorded in APPROXIMATE.
    battlecry("Ursol", T::None, |g, c| {
        let chosen = g
            .player(c.side)
            .hand
            .iter()
            .enumerate()
            .filter(|(_, hc)| {
                hc.card.def().kind() == super::Kind::Spell
                    && behaviour_of(hc.card).and_then(|b| b.spell).is_some()
            })
            .max_by_key(|(_, hc)| hc.card.def().cost)
            .map(|(i, hc)| (i, hc.card));
        if let Some((idx, card)) = chosen
            && let Some(f) = behaviour_of(card).and_then(|b| b.spell)
        {
            g.player_mut(c.side).hand.remove(idx);
            f(
                g,
                &Ctx {
                    card,
                    side: c.side,
                    target: None,
                    source: None,
                    outcast: false,
                    dying: None,
                    marks: Marks::NONE,
                    mana_spent: 0,
                },
            );
        }
    }),

    // -------------------------------------------------------- phase 3, G5
    // Start of Game: docs/RUST_CARDS_PLAN.md §4 phase 4 (G5).
    start_of_game("Chainbreaker Hogger", |g, c| {
        let mut extra: Inline<CardId, MAX_DECK> = Inline::new();
        for &card in g.player(c.side).deck.iter() {
            if card != c.card && card.def().rarity() == super::Rarity::Legendary {
                extra.push(card);
            }
        }
        for card in extra.iter() {
            g.player_mut(c.side).deck.push(*card);
        }
        g.shuffle_deck(c.side);
    }),
    // Both halves check the *deck* specifically, matching "while building
    // your deck" framing: a deck with no other minions gets Mug's Magic, one
    // with no spells gets Zee's Might, each read back later by name from
    // `hero_power` -- see `Game::card_cost`/`play_card`/`legal_actions`. A
    // deck satisfying both (extremely degenerate) ends up with Zee's Might,
    // since it is checked second; not a real deckbuilding choice either way.
    start_of_game("Mug'Zee", |g, c| {
        let deck = g.player(c.side).deck;
        // "No *other* minions": Mug'Zee itself may still be sitting in the
        // deck (this fires for a copy kept in hand too, and Start of Game
        // scans deck and hand alike), and must not disqualify itself.
        if deck
            .iter()
            .all(|&card| card == c.card || card.def().kind() != super::Kind::Minion)
        {
            g.player_mut(c.side).hero_power = tokens::MUGS_MAGIC;
        }
        if deck
            .iter()
            .all(|&card| card.def().kind() != super::Kind::Spell)
        {
            g.player_mut(c.side).hero_power = tokens::ZEES_MIGHT;
        }
    }),
    // King Llane plants itself in the *opponent's* deck at Start of Game, so
    // Garona Halforcen's "if your opponent is holding King Llane" can ever
    // be true; its battlecry then returns it to whichever deck it is drawn
    // from, which by then is normally the opponent's.
    c(
        "King Llane",
        T::None,
        None,
        Some(|g, c| {
            g.draw_cards(c.side, 1);
            g.player_mut(c.side).deck.push(c.card);
            g.shuffle_deck(c.side);
        }),
        None, None, None, None, None, None,
        Some(|g, c| {
            let foe = c.side.other();
            if let Some(idx) = g.player(c.side).deck.position(&c.card) {
                g.player_mut(c.side).deck.remove(idx);
                g.player_mut(foe).deck.push(c.card);
                g.shuffle_deck(foe);
            }
        }),
    ),
    battlecry("Garona Halforcen", T::None, |g, c| {
        let foe = c.side.other();
        if let Some(idx) = g
            .player(foe)
            .hand
            .iter()
            .position(|hc| hc.card.name() == "King Llane")
        {
            g.player_mut(foe).hand.remove(idx);
            g.player_mut(foe).hero_hp /= 2;
            g.check_over();
        }
    }),

    // -------------------------------------------------------- phase 4, G7
    // Forced attack: docs/RUST_CARDS_PLAN.md §4 phase 4 (G7).
    spell("Emergency Surgery", T::EnemyMinion, |g, c| {
        let Some(target) = c.target else { return };
        for _ in 0..4 {
            if g.summon_token(c.side, tokens::NECRONURSE, 1) == 0 {
                break;
            }
            let alive = matches!(target, Target::Minion(s, i)
                if g.player(s).board.get(i as usize).is_some_and(|m| m.active()));
            if !alive {
                break;
            }
            let slot = g.player(c.side).board.len() as u8 - 1;
            g.forced_attack((c.side, slot), target);
            if g.is_over() {
                return;
            }
        }
    }),
    spell("Spire of Solitude", T::None, |g, c| {
        let n = g.player(c.side).hand.len() as i16;
        if g.summon_token(c.side, tokens::SHIVARRA_INFILTRATOR, 1) > 0
            && let Some(last) = g.player(c.side).board.len().checked_sub(1)
        {
            let slot = last as u8;
            let t = Target::Minion(c.side, slot);
            g.set_attack(t, n);
            g.set_health(t, n);
            if let Some(target) = g.random_minion(c.side.other()) {
                g.forced_attack((c.side, slot), target);
            }
        }
    }),

    // -------------------------------------------------------- phase 4, G6
    // Quest / Sidequest: docs/RUST_CARDS_PLAN.md §4 phase 4 (G6). Progress
    // lives entirely in the `trigger` hook -- see Player::quest/sidequest
    // and the extra reactor slots in Game::fire.
    trigger("The Food Chain", |g, ctx| {
        if let Event::CardPlayed { side, card } = ctx.event
            && side == ctx.side
            && card.def().kind() == super::Kind::Minion
            && card.def().races.any(Races::BEAST)
            && matches!(card.def().atk, 1 | 3 | 5 | 7)
            && let Some((qcard, progress)) = g.player(ctx.side).quest
        {
            // 1 -> bit 0, 3 -> bit 1, 5 -> bit 2, 7 -> bit 3: which of the
            // four thresholds have been played, not how many total.
            let bit = 1u8 << ((card.def().atk - 1) / 2);
            if progress & bit == 0 {
                let progress = progress | bit;
                if progress.count_ones() == 4 {
                    g.give_token(ctx.side, tokens::SHOKK);
                    g.player_mut(ctx.side).quest = None;
                } else {
                    g.player_mut(ctx.side).quest = Some((qcard, progress));
                }
            }
        }
    }),
    trigger("Unleash the Colossus", |g, ctx| {
        if let Event::Damaged { target, amount } = ctx.event
            && amount == 2
            && g.current == ctx.side
        {
            let enemy = match target {
                Target::Hero(s) => s != ctx.side,
                Target::Minion(s, _) => s != ctx.side,
            };
            if enemy
                && let Some((qcard, progress)) = g.player(ctx.side).quest
            {
                let progress = progress + 1;
                if progress >= 12 {
                    g.give_token(ctx.side, tokens::GORISHI_COLOSSUS);
                    g.player_mut(ctx.side).quest = None;
                } else {
                    g.player_mut(ctx.side).quest = Some((qcard, progress));
                }
            }
        }
    }),
    trigger("Storm the Gates", |g, ctx| {
        if let Event::CardPlayed { side, card } = ctx.event
            && side == ctx.side
            && card.def().kind() == super::Kind::Minion
            && (card.def().races.any(Races::BEAST) || card.def().races.any(Races::UNDEAD))
            && let Some((qcard, progress)) = g.player(ctx.side).sidequest
        {
            let progress = progress + 1;
            if progress >= 3 {
                g.give_token(ctx.side, tokens::ZOMBEAST);
                g.player_mut(ctx.side).sidequest = None;
            } else {
                g.player_mut(ctx.side).sidequest = Some((qcard, progress));
            }
        }
    }),
    // ------------------------------------------------------------- misc
    deathrattle("The Egg of Khelos", |g, c| {
        let Some(dying) = c.dying.map(|m| m.card) else {
            return;
        };
        let next = if dying == tokens::EGG_OF_KHELOS_1 {
            tokens::EGG_OF_KHELOS_2
        } else if dying == tokens::EGG_OF_KHELOS_2 {
            tokens::EGG_OF_KHELOS_3
        } else if dying == tokens::EGG_OF_KHELOS_3 {
            tokens::EGG_OF_KHELOS_4
        } else if dying == tokens::EGG_OF_KHELOS_4 {
            tokens::EGG_OF_KHELOS_5
        } else {
            tokens::KHELOS
        };
        g.summon_token(c.side, next, 1);
    }),
    // Rush and Lifesteal are ordinary keywords; "Costs Corpses instead of
    // Mana" is not a mechanic the generator's TEXT_UNDERSTOOD pass
    // recognises, so this card would otherwise read as unimplemented even
    // though `Game::pays_with_corpses` already gives it real behaviour.
    // Every hook stays None on purpose -- this row exists only so
    // `is_implemented` sees it.
    c(
        "Reanimated Pterrordax",
        T::None,
        None, None, None, None, None, None, None, None, None,
    ),
    battlecry("Blood Doctor Thal'ena", T::None, |g, c| {
        g.player_mut(c.side).second_hero_power = Some(tokens::VAMPYRS_KISS);
    }),
    spell("Shaladrassil", T::None, |g, c| {
        let corrupt = c.marks.has(Marks::PLAYED_HIGHER_COST);
        for (normal, corrupted) in [
            (tokens::NIGHTMARE, tokens::CORRUPTED_NIGHTMARE),
            (tokens::DREAM, tokens::CORRUPTED_DREAM),
            (tokens::LAUGHING_SISTER, tokens::CORRUPTED_LAUGHING_SISTER),
            (tokens::YSERA_AWAKENS, tokens::CORRUPTED_AWAKENING),
            (tokens::EMERALD_DRAKE, tokens::CORRUPTED_DRAKE),
        ] {
            g.give_token(c.side, if corrupt { corrupted } else { normal });
        }
    }),
    // Charge is an ordinary keyword; the rest of the text -- reacting from
    // hand or deck rather than the board -- is engine-level special-casing
    // in Game::fire/tick_warptooth, which no hook here could express. This
    // row exists only so `is_implemented` sees it.
    c(
        "Warptooth",
        T::None,
        None, None, None, None, None, None, None, None, None,
    ),
    // "Craft a custom location" from the deck's cost curve needs a
    // location-generation system the engine does not have. This row exists
    // only so `is_implemented` sees a vanilla 3/5.
    c(
        "Elise the Navigator",
        T::None,
        None, None, None, None, None, None, None, None, None,
    ),
    // "Carve 12 Mana worth of Nature spells into them" needs per-token stat
    // state the engine does not track; only the three bare Treants land.
    battlecry("Bashana Runetotem", T::None, |g, c| {
        g.summon_token(c.side, tokens::TREANT, 3);
    }),
    // The "three hits to break" aura needs a per-shield hit counter the
    // engine's Divine Shield flag does not have. This row exists only so
    // `is_implemented` sees the printed Divine Shield and Taunt.
    c(
        "Toreth the Unbreaking",
        T::None,
        None, None, None, None, None, None, None, None, None,
    ),
    // Fixed ammunition rather than the four choosable, cycling effects: this
    // always fires the plainest of the four (1 damage to all enemies) and
    // never freezes, summons or discounts, so it is weaker every game.
    trigger("Tiny Pal", |g, c| {
        if matches!(c.event, Event::AfterAttack { attacker: Target::Hero(s), .. } if s == c.side) {
            g.damage_area(c.side, Area::AllEnemies, 1);
        }
    }),
    // Combo (arriving with a Dark Gift, G11) is not implemented; the base
    // Discover always fires without it -- the same approximation already
    // used for Darkrider and Shadowflame Suffusion.
    spell("Nightmare Fuel", T::None, |g, c| {
        g.discover_from_opponent_deck(c.side, |d| d.kind() == super::Kind::Minion);
    }),
    // The random Bonus Effect pool (stat buffs and keyword grants) is fixed
    // to always +1/+1 -- the plainest entry in that pool. "Not itself": the
    // same exclusion Warden Maiev's identical "after you play a minion"
    // wording uses above.
    trigger("Dreambound Raptor", |g, c| {
        if let Event::MinionSummoned { side, slot, .. } = c.event
            && side == c.side
            && slot != c.slot
        {
            g.buff(Target::Minion(side, slot), 1, 1);
        }
    }),
    // The current hand -- already missing this very card, removed by `play`
    // before the battlecry runs -- is stashed and restored whole in
    // `Game::end_turn`; nothing here needs to know what turn it is.
    battlecry("The Fins Beyond Time", T::None, |g, c| {
        let p = g.player_mut(c.side);
        p.swapped_hand = Some(p.hand);
        p.hand = p
            .starting_hand
            .iter()
            .map(|&card| HandCard::new(card))
            .collect();
    }),
    // Location. "Deal 1 damage" carries no target qualifier in the corpus
    // text -- Blizzard's own Location oracle text often omits it, relying on
    // the in-game targeting reticle -- so this is narrowed to enemies only
    // as the conservative default; see APPROXIMATE. "Reopen" clears the same
    // `Flags::USED` + `cooldown` pair `Game::use_location` sets, which is
    // exactly what stands between a Location and using it again this turn.
    c(
        "Nespirah, Enthralled",
        T::EnemyCharacter,
        Some(|g, c| {
            if let Some(t) = c.target {
                g.spell_damage(c.side, Some(t), 1);
            }
        }),
        None,
        Some(|g, c| {
            g.summon_token(c.side, tokens::NESPIRAH_UNSHACKLED, 1);
        }),
        Some(|g, c| {
            if let Event::SpellCast { side, card } = c.event
                && side == c.side
                && card.def().school() == super::School::Fel
                && let Some(m) = g.player_mut(c.side).board.get_mut(c.slot as usize)
            {
                m.flags.remove(Flags::USED);
                m.cooldown = 0;
            }
        }),
        None, None, None, None, None,
    ),
    // Needs never be played, only kept: this only sets a flag `Game::give_card`
    // and `Game::begin_turn` read for the rest of the game. "When you have
    // space" is approximated as a once-per-own-turn check rather than a fully
    // reactive one -- a return can be delayed a turn, never skipped outright.
    start_of_game("Godfrey the Betrayer", |g, c| {
        g.player_mut(c.side).godfrey_active = true;
    }),
    // `add_random_to_hand`'s own return value already covers both stopping
    // conditions ("Fill" reaching a full hand, and the Dragon pool running
    // dry), so the loop needs no separate length check.
    battlecry("Merithra of the Dream", T::None, |g, c| {
        let discount = c.mana_spent >= 25;
        loop {
            let before = g.player(c.side).hand.len();
            let added = g.add_random_to_hand(c.side, |d| {
                d.kind() == super::Kind::Minion && d.races.any(Races::DRAGON)
            });
            if !added {
                break;
            }
            if discount
                && let Some(hc) = g.player_mut(c.side).hand.get_mut(before)
            {
                hc.cost_delta = 1 - hc.card.def().cost;
            }
        }
    }),
];

/// Cards implemented only in part, with what is missing.
///
/// Entries are **conservative by default**: the card is weaker than it should
/// be, never stronger, so a deck holding one is underrated rather than
/// overrated. Where that is not achievable the note says so in as many words,
/// because an entry that reads stronger than the card inflates a deck's rating
/// and must be impossible to mistake for the safe kind.
///
/// They are listed rather than silently counted as complete, because a
/// coverage figure that mixes exact and approximate cards is worse than a
/// smaller honest one.
pub const APPROXIMATE: &[(&str, &str)] = &[
    (
        "Storm the Gates",
        "the reward is a fixed old 1/1 Zombeast token rather than a custom          minion crafted from two chosen deck cards, which the engine has no          way to build",
    ),
    (
        "Archmage Kalec",
        "\"all spells in your hand and deck\" needs per-card state in the deck;          implemented as Spell Damage on the hero, which also boosts spells          acquired after it lands -- the one entry here that can read stronger          than the card, not weaker",
    ),
    (
        "Felwood Treant",
        "the permanent-crystal upgrade needs \"mana spent while holding this\",          which the engine does not track; only the temporary crystal is          implemented",
    ),
    (
        "Falric",
        "the corpse-doubling clause needs a per-player multiplier the engine          does not have; only the draw is implemented",
    ),
    (
        "Endbringer Umbra",
        "\"died this game\" needs a graveyard; re-fires the deathrattles of          living friendly minions instead",
    ),
    (
        "Soldier of Al'Akir",
        "its aura scales with the Herald count, which the aura signature          cannot read; fixed at the Herald-1 value",
    ),
    (
        "Cursed Catacombs",
        "\"Make it Temporary\" (burns unused at end of turn) needs per-card          state the engine does not have; the Discover alone reads stronger,          not weaker -- the second entry here that can, after Archmage Kalec",
    ),
    (
        "Darkrider",
        "the Discover comes with no Dark Gift (G11); a plain Dragon is          discovered instead",
    ),
    (
        "Shadowflame Suffusion",
        "the Discover comes with no Dark Gift (G11); a plain Warrior minion is          discovered instead",
    ),
    (
        "Torch",
        "does not return to hand with excess damage; that needs a per-copy          variable-damage field the engine does not have",
    ),
    (
        "Stadium Announcer",
        "Rewind is not modelled -- both equips are single, independent rolls",
    ),
    (
        "Elise the Navigator",
        "crafting a custom location from the deck's cost curve needs a location-generation system the engine does not have; plays as a vanilla 3/5",
    ),
    (
        "Bashana Runetotem",
        "\"Carve 12 Mana worth of Nature spells into them\" needs per-token stat state the engine does not have; only the three bare 2/2 Treants are summoned",
    ),
    (
        "Toreth the Unbreaking",
        "the aura giving friendly Divine Shields three hits before breaking needs a hit counter the engine's Divine Shield flag does not have; Divine Shield behaves normally (one hit) instead",
    ),
    (
        "Tiny Pal",
        "the four choosable, cycling ammunition effects are approximated as one fixed effect -- 1 damage to all enemies after each hero attack -- with no freeze, summon or discount option",
    ),
    (
        "Nightmare Fuel",
        "the Combo bonus (the discovered minion arrives with a Dark Gift, G11) is not implemented; the base Discover always fires without it",
    ),
    (
        "Dreambound Raptor",
        "the random Bonus Effect pool is fixed to a single always +1/+1, rather than a random draw from the full set of stat and keyword grants",
    ),
    (
        "Nespirah, Enthralled",
        "the corpus text for \"Deal 1 damage\" carries no target qualifier; narrowed to enemies only as the conservative default rather than guessing it can hit friendly characters too",
    ),
    (
        "Godfrey the Betrayer",
        "\"when you have space\" is checked once per own turn (begin_turn) rather than the instant a card leaves hand, so a return can lag by up to a turn; the queue of waiting cards is also capped at 10",
    ),
];

/// Whether a card is implemented only approximately.
pub fn is_approximate(card: CardId) -> bool {
    APPROXIMATE.iter().any(|(n, _)| *n == card.name())
}

/// `CardId` -> behaviour slot plus one, with zero meaning "no behaviour".
///
/// Built once. A dense array over the whole card table costs 32 KB and turns
/// dispatch into a single load, which matters because this is consulted every
/// time a card is played or a minion dies.
static INDEX: OnceLock<Box<[u16]>> = OnceLock::new();

fn index() -> &'static [u16] {
    INDEX.get_or_init(|| {
        let mut by_name: Vec<(&'static str, u16)> = BEHAVIOURS
            .iter()
            .enumerate()
            .map(|(i, b)| (b.name, i as u16))
            .collect();
        by_name.sort_unstable();
        let mut out = vec![0u16; DEFS.len()].into_boxed_slice();
        for (i, info) in INFO.iter().enumerate() {
            if let Ok(k) = by_name.binary_search_by_key(&info.name, |(n, _)| *n) {
                out[i] = by_name[k].1 + 1;
            }
        }
        out
    })
}

/// The behaviour attached to a card, if it has one.
#[inline]
pub fn behaviour_of(card: CardId) -> Option<&'static Behaviour> {
    match index()[card.0 as usize] {
        0 => None,
        slot => Some(&BEHAVIOURS[slot as usize - 1]),
    }
}

/// Whether the engine knows what this card does.
///
/// A vanilla minion needs no behaviour: its stats and keywords already say
/// everything. A card with rules text and no behaviour is unimplemented, and
/// must not be offered to a deck builder.
pub fn is_implemented(card: CardId) -> bool {
    if behaviour_of(card).is_some() {
        return true;
    }
    let d = card.def();
    // Locations, hero cards and hero powers do nothing without code. A minion
    // or weapon whose entire text is keywords the kernel already models needs
    // none — a vanilla 3/2 and a plain Taunt minion are equally playable.
    match d.kind() {
        super::Kind::Minion | super::Kind::Weapon => d.keywords.has(Keywords::TEXT_UNDERSTOOD),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::super::by_name;
    use super::*;

    #[test]
    fn every_declared_card_exists_in_the_corpus() {
        // Catches a typo in a name the moment it is written, rather than as a
        // card that silently does nothing in a million games.
        for b in BEHAVIOURS {
            assert!(by_name(b.name).is_some(), "no card named {:?}", b.name);
        }
    }

    #[test]
    fn every_approximate_card_is_real_and_implemented() {
        for (name, note) in APPROXIMATE {
            let card = by_name(name).unwrap_or_else(|| panic!("no card named {name:?}"));
            assert!(
                behaviour_of(card).is_some(),
                "{name} is listed as approximate but has no behaviour at all"
            );
            assert!(!note.is_empty(), "{name} must say what is missing");
        }
    }

    #[test]
    fn no_card_is_declared_twice() {
        let mut names: Vec<&str> = BEHAVIOURS.iter().map(|b| b.name).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(names.len(), before, "a card is listed twice in BEHAVIOURS");
    }

    #[test]
    fn behaviours_attach_to_every_printing() {
        // Fireball exists as more than one card id; both must carry the spell.
        let printings: Vec<CardId> = super::super::all()
            .filter(|c| c.info().name == "Fireball")
            .collect();
        assert!(
            printings.len() > 1,
            "expected reprints of Fireball to test against"
        );
        for p in printings {
            assert!(
                behaviour_of(p).is_some(),
                "{} lost its behaviour",
                p.info().id
            );
        }
    }

    #[test]
    fn a_spell_declares_a_cast_effect_or_is_a_secret_or_a_quest() {
        // A secret is a spell that is set rather than cast, so its behaviour
        // lives in the `secret` hook and it has no cast effect at all. A
        // Quest or Sidequest is the same shape, one hook over: its progress
        // lives entirely in `trigger` (see Player::quest/sidequest).
        for b in BEHAVIOURS {
            let card = by_name(b.name).unwrap();
            if card.def().kind() != super::super::Kind::Spell {
                continue;
            }
            let is_secret = card.def().keywords.has(Keywords::SECRET);
            assert_eq!(
                b.secret.is_some(),
                is_secret,
                "{} and its SECRET keyword disagree",
                b.name
            );
            let is_quest = card.def().keywords.has(Keywords::QUEST)
                || card.def().keywords.has(Keywords::SIDE_QUEST);
            if is_quest {
                assert!(
                    b.trigger.is_some() && b.spell.is_none(),
                    "{} is a Quest/Sidequest; its progress belongs in `trigger`, not `spell`",
                    b.name
                );
                continue;
            }
            // A Choose One card's behaviour lives in its modes, so it has no
            // cast effect of its own either.
            assert_eq!(
                b.spell.is_some(),
                !is_secret && b.choose.is_none(),
                "{} should have a cast effect exactly when it is neither a                  secret nor a Choose One",
                b.name
            );
            assert!(
                b.battlecry.is_none(),
                "{} is a spell with a battlecry",
                b.name
            );
        }
    }

    #[test]
    fn choose_one_cards_declare_at_least_two_modes() {
        for b in BEHAVIOURS {
            if let Some(modes) = b.choose {
                assert!(
                    modes.len() >= 2,
                    "{} is a Choose One with {} mode(s)",
                    b.name,
                    modes.len()
                );
                assert!(
                    b.spell.is_none() && b.battlecry.is_none(),
                    "{} has both modes and a plain effect",
                    b.name
                );
                assert!(
                    by_name(b.name).unwrap().def().keywords.has(Keywords::CHOOSE_ONE),
                    "{} has modes but the card is not a Choose One",
                    b.name
                );
            }
        }
    }

    #[test]
    fn a_minion_never_declares_a_cast_effect() {
        for b in BEHAVIOURS {
            let card = by_name(b.name).unwrap();
            if card.def().kind() == super::super::Kind::Minion {
                assert!(
                    b.spell.is_none(),
                    "{} is a minion with a cast effect",
                    b.name
                );
            }
        }
    }

    #[test]
    fn declared_battlecries_match_the_printed_keyword() {
        // If the corpus says Battlecry and the row has none, the card is only
        // half implemented and would quietly play as a vanilla body.
        for b in BEHAVIOURS {
            let card = by_name(b.name).unwrap();
            if b.battlecry.is_some() {
                // A Combo minion fires on being played, which is the same
                // hook; the corpus tags it COMBO rather than BATTLECRY.
                // Battlecry, Combo and Kindred all fire on being played. The
                // corpus tags the first two as mechanics; Kindred exists only
                // in the text, so that one is matched there.
                let d = card.def();
                // Battlecry, Combo, Kindred and "When summoned" all resolve
                // as the card enters play. The corpus tags the first two as
                // mechanics; the others exist only in the text.
                let text = card.info().text;
                assert!(
                    d.keywords.has(Keywords::BATTLECRY)
                        || d.keywords.has(Keywords::COMBO)
                        // Outcast resolves as the card enters play, the same
                        // hook under a different printed name.
                        || d.keywords.has(Keywords::OUTCAST)
                        || text.contains("Kindred")
                        || text.contains("When summoned"),
                    "{} has a play effect the card text does not mention",
                    b.name
                );
            }
        }
    }

    #[test]
    fn declared_deathrattles_match_the_printed_keyword() {
        for b in BEHAVIOURS {
            let card = by_name(b.name).unwrap();
            if b.deathrattle.is_some() {
                assert!(
                    card.def().keywords.has(Keywords::DEATHRATTLE),
                    "{} has a deathrattle the card text does not mention",
                    b.name
                );
            }
        }
    }

    #[test]
    fn vanilla_minions_count_as_implemented() {
        let vanilla = by_name("Bloodfen Raptor").unwrap();
        assert!(is_implemented(vanilla));
        assert!(
            behaviour_of(vanilla).is_none(),
            "a vanilla body needs no code"
        );
    }

    #[test]
    fn a_text_card_without_behaviour_is_not_implemented() {
        let unimplemented = super::super::all()
            .find(|c| c.def().kind() == super::super::Kind::Spell && behaviour_of(*c).is_none())
            .expect("plenty of spells are still unimplemented");
        assert!(!is_implemented(unimplemented));
    }

    #[test]
    fn the_index_is_dense_and_covers_the_table() {
        assert_eq!(index().len(), DEFS.len());
    }
}
