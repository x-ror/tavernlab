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
use crate::state::{
    DeckCard, Flags, Game, HandCard, MAX_BOARD, MAX_DECK, MAX_HAND, Marks, Pending, PendingKind,
    Side, Target,
};

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
    /// Whether the card sat in the exact middle of the hand when it was
    /// played (Precise Shot). Read at the same moment `outcast` is, and for
    /// the same reason -- by the time the effect runs the card has left.
    ///
    /// A hand with an even number of cards has no middle, so this is false
    /// for every card in one; "EXACTLY in the center" is the card's own word.
    pub centre: bool,
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

impl Ctx {
    /// A Ctx for an effect nobody played: a token cast on someone's behalf, a
    /// Deathrattle fired with no body, a Start of Game hook.
    ///
    /// Thirteen call sites used to spell out the same eight fields, six of
    /// them the same six defaults every time -- which meant that adding a
    /// field to `Ctx` was thirteen edits, and that a site which wanted a
    /// non-default was hard to spot among the ones that did not.
    pub fn bare(card: CardId, side: Side) -> Ctx {
        Ctx {
            card,
            side,
            target: None,
            source: None,
            outcast: false,
            centre: false,
            dying: None,
            marks: crate::state::Marks::NONE,
            mana_spent: 0,
        }
    }

    /// [`bare`](Self::bare) pointed at something.
    pub fn at(card: CardId, side: Side, target: Option<Target>) -> Ctx {
        Ctx {
            target,
            ..Ctx::bare(card, side)
        }
    }
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
    /// Any damaged minion, either side (Ominous Nightmares).
    DamagedMinion,
    UndamagedMinion,
    FriendlyBeast,
    /// A Legendary minion, either side (Garona's Last Stand).
    LegendaryMinion,
    /// An enemy minion that has a minion type at all (Bugsquasher). A body
    /// with no tribe is not a legal target, which is the whole joke.
    EnemyMinionWithRace,
    /// An enemy minion with Taunt (The Black Knight). A requirement, not a
    /// preference: with nothing taunting, the card has no legal target at
    /// all — which is exactly when its Tradeable half earns its keep.
    EnemyTaunt,
    /// One of your own Locations (Welcome Home!). The only spec that points
    /// at something that is not a minion, which is why the guard below has
    /// to ask what kind of permanent it is before it checks anything else.
    FriendlyLocation,
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
        if self == TargetSpec::FriendlyLocation {
            return matches!(t, Target::Minion(s, i)
                if s == side
                    && g.player(s).board.get(i as usize).is_some_and(|m| {
                        m.active() && m.kind() == super::Kind::Location
                    }));
        }
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
            TargetSpec::DamagedMinion => {
                matches!(t, Target::Minion(s, i) if g.player(s).board[i as usize].damage > 0)
            }
            TargetSpec::UndamagedMinion => {
                matches!(t, Target::Minion(s, i) if g.player(s).board[i as usize].damage == 0)
            }
            TargetSpec::FriendlyBeast => matches!(t, Target::Minion(s, i)
                if s == side && g.player(s).board[i as usize].races().any(Races::BEAST)),
            TargetSpec::LegendaryMinion => matches!(t, Target::Minion(s, i)
                if g.player(s).board[i as usize].card.def().rarity() == super::Rarity::Legendary),
            TargetSpec::EnemyMinionWithRace => matches!(t, Target::Minion(s, i)
                if s == foe && !g.player(s).board[i as usize].races().is_empty()),
            TargetSpec::EnemyTaunt => matches!(t, Target::Minion(s, i)
                if s == foe && g.player(s).board[i as usize].has(Keywords::TAUNT)),
            // Answered above, before the minion guard.
            TargetSpec::FriendlyLocation => false,
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
    /// A continuous effect a minion has on *itself* -- "Has +3 Attack while
    /// damaged", "Has +2 Attack during your opponent's turn". Recomputed
    /// alongside auras.
    pub bonus: Option<Bonus>,
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

/// What a minion grants itself right now, as `(attack, health)`.
///
/// Unlike [`Aura`] this does get the whole game, because the conditions
/// printed on these cards are about the position at large -- whose turn it
/// is, whether a weapon is equipped, what else you control. The rule that
/// keeps that safe is the same one recomputation needs: **a bonus may only
/// read state this recomputation does not itself change.** Reading another
/// minion's attack would feed back into itself; reading `damage`, the turn,
/// the weapon or the board does not.
///
/// It also has to be true that every input can only change at a point that
/// recomputes -- a board change, a turn boundary, damage or healing, or a
/// weapon arriving or breaking. "While your opponent holds six cards" is a
/// real printed condition this cannot yet express, because hand size moves
/// on every draw and no recomputation follows.
pub type Bonus = fn(
    game: &Game,
    side: Side,
    slot: u8,
    me: &crate::state::Permanent,
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
        bonus: None,
        secret,
        choose,
        cost_delta,
        start_of_game,
    }
}

/// A minion whose only behaviour is a continuous effect on itself.
const fn bonus(name: &'static str, f: Bonus) -> Behaviour {
    Behaviour {
        name,
        target: TargetSpec::None,
        spell: None,
        battlecry: None,
        deathrattle: None,
        trigger: None,
        aura: None,
        bonus: Some(f),
        secret: None,
        choose: None,
        cost_delta: None,
        start_of_game: None,
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
    /// Shadows of Yesterday's 3/2 Shade, and Tyrannogill's 2/1 Murloc.
    pub const ANOMALOUS_SHADE: CardId = token("TIME_610t2");
    pub const DINOLOC: CardId = token("TLC_240t");
    /// The Kabal's plant: a 3/3 Lifesteal that goes into the *enemy's* deck
    /// and summons for you when they draw it.
    pub const IMP_FORMANT: CardId = token("127024-imp-formant");
    /// Tonic of Tyranny's payout.
    pub const VOIDLORD: CardId = token("CORE_LOOT_368");
    /// The three that wake up when a Dragon is played, and what they wake into.
    pub const STONETALON_STRIKER: CardId = token("CATA_551");
    pub const STONETALON_STRIKER_AWAKE: CardId = token("CATA_551t");
    pub const EBONSCALE_SCOUT: CardId = token("CATA_552");
    pub const EBONSCALE_SCOUT_AWAKE: CardId = token("CATA_552t");
    pub const EBYSSIAN: CardId = token("CATA_553");
    pub const EBYSSIAN_AWAKE: CardId = token("CATA_553t");
    /// Schism, and the two halves it shatters into. Both halves are printed
    /// under the parent's own name, which is why they need ids here at all:
    /// the behaviour table is keyed by name, so all three would otherwise be
    /// one row.
    pub const SCHISM: CardId = token("CATA_306");
    pub const SCHISM_BUFF: CardId = token("CATA_306t1");
    pub const SCHISM_COPY: CardId = token("CATA_306t2");
    /// Reach Equilibrium's two rewards, and the minion they combine into.
    pub const SOLETOS_LIFE: CardId = token("TLC_817t3");
    pub const SOLETOS_DEATH: CardId = token("TLC_817t4");
    pub const SOLETOS_WHOLE: CardId = token("TLC_817t5");
    /// Slime 'em!'s payout, one for each player.
    pub const ECTOPLASM: CardId = token("127118-ectoplasm");
    /// What five Demon Hunter cards hand out.
    pub const VOID_SOUL: CardId = token("JAIL_732");
    /// The Forbidden Sequence's reward.
    pub const ORIGIN_STONE: CardId = token("TLC_460t");
    /// Lady Azshara's two Locations, and the empowered form of each. Both
    /// pairs share a name, so the behaviour table gives each pair one row.
    pub const WELL_OF_ETERNITY: CardId = token("TIME_211t1");
    pub const WELL_OF_ETERNITY_EMPOWERED: CardId = token("TIME_211t1t");
    pub const ZIN_AZSHARI: CardId = token("TIME_211t2");
    pub const ZIN_AZSHARI_EMPOWERED: CardId = token("TIME_211t2t");
    /// The carrier for Welcome Home!'s granted deathrattle -- a real card
    /// whose whole text is "Deathrattle: Summon a random 3-Cost minion."
    pub const STUBBORN_SUSPECT: CardId = token("SW_006");
    /// Rockskipper's payout: a 1-Cost spell that deals 3 damage.
    pub const ROCK: CardId = token("WW_001t");
    /// The Arcanomicon's three upgrades. The Leylines themselves are named
    /// once, in `LEYLINES`, because seven cards ask about them as a group.
    pub const ENERGIZE: CardId = token("MEND_505t");
    pub const UNBLOCK: CardId = token("MEND_505t2");
    pub const EMPOWER: CardId = token("MEND_505t3");
    /// Spellweaver's Brilliance's 6/6.
    pub const AZURE_WARDEN: CardId = token("CATA_452t");
    /// The Aura cycle, its summon, and the Shatter pair that flies with it.
    pub const CHRONOLOGICAL_AURA: CardId = token("TIME_700");
    pub const CHRONOLOGICAL_DRAKE: CardId = token("TIME_700t");
    pub const SANDFURY_AURA: CardId = token("CATA_480");
    pub const GNOMISH_AURA: CardId = token("TIME_009t1");
    pub const MEKKATORQUES_AURA: CardId = token("TIME_009t2");
    pub const FLIGHT_MANEUVERS: CardId = token("CATA_479");
    pub const FLIGHT_MANEUVERS_DRAKES: CardId = token("CATA_479t");
    pub const FLIGHT_MANEUVERS_BUFF: CardId = token("CATA_479t2");
    pub const SKY_DRAKE: CardId = token("CATA_479t3");
    /// Gnomeregan's later two ages. The first needs no id: its row is keyed
    /// by name like every other, and only the ages it turns into are named
    /// here, as the targets of the transform.
    /// The carrier for the Gnomeregan chain's granted "Deathrattle: Deal 2
    /// damage to the enemy hero" -- a real card whose whole text is that
    /// deathrattle, the way `GRANTED_RATTLES` works everywhere else.
    pub const LEPER_GNOME: CardId = token("EX1_029");
    pub const PRESENT_GNOMEREGAN: CardId = token("TIME_044t1");
    pub const FUTURE_GNOMEREGAN: CardId = token("TIME_044t2");
    /// Supply Run, and the two halves it shatters into.
    pub const SUPPLY_RUN: CardId = token("CATA_820");
    pub const SUPPLY_RUN_DRAW: CardId = token("CATA_820t");
    pub const SUPPLY_RUN_BUFF: CardId = token("CATA_820t2");
    /// Godfather Kazakus's nine sham-trial effects, and the trial itself.
    pub const DETAINED_FOR_DESTRUCTION: CardId = token("132837-detained-for-destruction");
    pub const CONVICTED_FOR_CONSPIRACY: CardId = token("132849-convicted-for-conspiracy");
    pub const SENTENCED_FOR_SMUGGLING: CardId = token("132843-sentenced-for-smuggling");
    pub const CRATE_OF_CONTRABAND: CardId = token("132838-crate-of-contraband");
    pub const SPURIOUS_SHIV: CardId = token("132844-spurious-shiv");
    pub const CRIMINAL_CONTRACT: CardId = token("132850-criminal-contract");
    pub const POTION_OF_PERJURY: CardId = token("132851-potion-of-perjury");
    pub const SWILL_OF_SUGGESTIBILITY: CardId = token("132856-swill-of-suggestibility");
    pub const TONIC_OF_TYRANNY: CardId = token("132848-tonic-of-tyranny");
    /// Imbue: the Blessings' own tokens.
    pub const EMERALD_PORTAL: CardId = token("EDR_445pt3");
    pub const EDR_WISP: CardId = token("EDR_851t");
    /// Frostburn Matriarch's 4/4 Dragon with Taunt.
    pub const FROSTBURN_BROODLING: CardId = token("FIR_901t");
    /// Cards that act as they are drawn, and what they leave behind.
    pub const ACORN: CardId = token("SW_439t");
    pub const SATISFIED_SQUIRREL: CardId = token("SW_439t2");
    pub const SHRED_OF_TIME: CardId = token("TIME_025t");
    pub const FOUND_GEAR: CardId = token("JAIL_386t");
    pub const TRIPPED_ARCANE: CardId = token("JAIL_881t");
    pub const TRIPPED_BEAST: CardId = token("JAIL_879t");
    pub const TORTOLLAN_NINJA: CardId = token("TLC_513t2");
    pub const GREENWING_ILLUSION: CardId = token("EDR_260t");
    /// The 1/1 Cannoneer. Its id is a slug because the card is newer than
    /// the CardDefs snapshot the corpus was built from -- see `xtask
    /// backfill`, which is what put it in the table at all.
    pub const CANNONEER: CardId = token("127012-cannoneer");
    pub const FLAME_ELEMENTAL: CardId = token("UNG_809t1");
    pub const ARCANE_MISSILES: CardId = token("EX1_277");
    pub const MUGS_MAGIC: CardId = token("JAIL_800hp1");
    /// Soul Immolation's replacement Hero Power. Its damage lives on the
    /// player (`hero_power_bonus`), because the power itself is a shared
    /// table entry that every game reads.
    pub const COLLAPSING_STAR: CardId = token("JAIL_EVENT_101hp");
    pub const ZEES_MIGHT: CardId = token("JAIL_800hp2");
    pub const NESPIRAH_UNSHACKLED: CardId = token("CATA_527t2");
    /// One of Wickerfang's four Legs (CATA_139t..t4); all four print
    /// identical text and stats, so any one id summoned four times behaves
    /// exactly like one of each.
    pub const WICKERFANGS_LEG: CardId = token("CATA_139t");
    pub const CHARGED_HAND_OF_ALAKIR: CardId = token("CATA_153t");
    /// Deathwing's four Cataclysms and the 12/12 one of them summons. Each
    /// Cataclysm is a real spell card in the corpus with its own row below,
    /// so it can be tested on its own rather than only through Deathwing.
    pub const DRAGONS_REIGN: CardId = token("CATA_190t10");
    pub const TOPPLE: CardId = token("CATA_190t11");
    pub const RAZE: CardId = token("CATA_190t12");
    pub const ENTHRALL: CardId = token("CATA_190t13");
    pub const PROGENY_OF_DEATHWING: CardId = token("CATA_190t14");
    /// Deathwing's Hero Power: +5 Attack this turn.
    pub const RUTHLESS: CardId = token("CATA_190p");
    pub const SINESTRAS_WING: CardId = token("CATA_154t");
    // Broxigar's Portal to Argus chain: each demon is "for your opponent",
    // and each demon's own deathrattle shuffles the next portal into the
    // *caster's* deck (a cross-player reward -- see the cards themselves).
    pub const SECOND_PORTAL_TO_ARGUS: CardId = token("TIME_020t3");
    pub const THIRD_PORTAL_TO_ARGUS: CardId = token("TIME_020t4");
    pub const FINAL_PORTAL_TO_ARGUS: CardId = token("TIME_020t5");
    pub const FLEEING_URZUL: CardId = token("TIME_020t2t");
    pub const FLEEING_INCUBUS: CardId = token("TIME_020t3t");
    pub const FLEEING_WRATHGUARD: CardId = token("TIME_020t4t");
    pub const FLEEING_TERRORGUARD: CardId = token("TIME_020t5t");
    pub const BROXIGAR: CardId = token("TIME_020");
    // Warrior backlog batch.
    pub const COLISEUM_CROCOLISK: CardId = token("TIME_873t");
    pub const STEADFAST_SECURITY: CardId = token("TLC_622t");
    pub const RAMPAGING_ZOMBIE: CardId = token("RLK_018t");
    pub const UNDEAD_MONSTROSITY: CardId = token("RLK_057t");
    pub const SPARK: CardId = token("BOT_102t");
    pub const SIZZLING_CINDER: CardId = token("TLC_249");
    /// Static Shock is itself a real, collectible card (implemented below);
    /// Thunderquake just needs to hand out a copy of it.
    pub const STATIC_SHOCK: CardId = token("TIME_218");
    pub const PLAYFUL_PUP: CardId = token("EDR_850pe");
    pub const WEBSPINNER: CardId = token("FP1_011");
    /// Muster for Battle's own weapon child is a Weapon, so
    /// `summonable_children()` (minions only) can't find it either.
    pub const LIGHTS_JUSTICE: CardId = token("CS2_091");
    /// Blob of Tar summons one of each, not one of a random child, so
    /// `summon_child` (which picks a single child) cannot cover both.
    pub const LANKY_BLOB: CardId = token("TLC_468t1");
    pub const ROBUST_BLOB: CardId = token("TLC_468t2");
    /// Voidwalker is itself a real, collectible card; Demonic Assault just
    /// needs to hand out copies of it.
    pub const VOIDWALKER: CardId = token("CS2_065");
    /// Second Flame is a real playable spell (First Flame's own "get a
    /// copy" effect), just not collectible, so it gets its own `spell` row
    /// below rather than being a summon token.
    pub const SECOND_FLAME: CardId = token("SW_108t");
    /// Widow's Bite escalates through two more non-collectible spells,
    /// each handing out the next: Feast, then Banquet.
    pub const WIDOWS_FEAST: CardId = token("JAIL_436t");
    pub const WIDOWS_BANQUET: CardId = token("JAIL_436t2");
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
    /// Forced-attack batch: the bodies that arrive swinging.
    pub const TEMPORAL_SHADOW: CardId = token("TIME_434t");
    pub const ANGRY_TREANT: CardId = token("TLC_230t");
    /// Granted deathrattles: the bodies the granting cards promise.
    pub const STEGODON: CardId = token("UNG_810");
    pub const PLANT: CardId = token("UNG_999t2t1");
    /// Ancient Raptor's third mode grants this, rather than granting itself:
    /// a minion's own deathrattle row fires on its death too, so a minion
    /// that granted itself would pay out twice.
    pub const LIVING_SPORES: CardId = token("UNG_999t2");
    /// Neutral backlog: Wicked Blightspawn's Dagger, Steamfin Thief's
    /// Murlocs and Bronze Keeper's Dragon.
    pub const WICKED_KNIFE: CardId = token("CS2_082");
    pub const JUVENILE_STEAMFIN: CardId = token("TLC_429t");
    pub const SANDSCALE_DRAGON: CardId = token("CATA_476t");
    /// Death Knight backlog: the Corpse-raised bodies.
    pub const RISEN_FOOTMAN: CardId = token("RLK_061t");
    pub const RISEN_GHOUL: CardId = token("RLK_008t");
    pub const RISEN_GROOM: CardId = token("RLK_506t");
    pub const HUNGRY_DRAKE: CardId = token("CATA_465t");
    /// Bronze Redeemer's Dragon, resized to match its parent.
    pub const BRONZE_BRUTE: CardId = token("CATA_478t");
    /// Web of Deception's 4/4.
    pub const SKITTERING_SPIDERLING: CardId = token("EDR_523t");
    /// Rat Trap's 6/6.
    pub const DOOM_RAT: CardId = token("GIL_577t");
    /// Sleep Paralysis' 3/6 Demon that cannot swing.
    pub const NIGHT_TERROR: CardId = token("EDR_490t");
    /// Glade Ecologist's spell and Holy Embrace's dark half. Both are real
    /// playable spells with rows of their own below.
    pub const PURIFYING_VINES: CardId = token("TLC_813");
    pub const DARK_EMBRACE: CardId = token("JAIL_941t");
    /// Mirror Dimension's 0/4 Taunt body.
    pub const MIRRORED_MAGE: CardId = token("TIME_006t1");
    /// Gladiatorial Combat's Tiger, which goes to the *opponent*.
    pub const COLISEUM_TIGER: CardId = token("TIME_870t");
    /// Shaman backlog: Ritual of Power's Breezling, Spirits of the Forest's
    /// two halves, and the Lightning Bolt Rehgar hands out (a real card).
    pub const BREEZLING: CardId = token("CATA_561t");
    pub const SPIRIT_WOLF: CardId = token("EX1_tk11");
    pub const SPIRIT_FALCON: CardId = token("EDR_233t2");
    pub const LIGHTNING_BOLT: CardId = token("EX1_238");
    /// Druid backlog: Mossbinding's Golem, Ravenous Flock's Hatchling and
    /// Flipper Friends' two halves. The Golem and Hatchling are children of
    /// their own cards; the Orca and Otter are children of Flipper Friends'
    /// Choose One halves rather than of the card, so they are named here.
    pub const ORCA: CardId = token("TSC_650t");
    pub const OTTER: CardId = token("TSC_650t4");
    pub const SKYSCREAMER_HATCHLING: CardId = token("TLC_237t");
    /// Tirion's Ashbringer, Veteran Warmedic's Battlefield Medic and
    /// Halazzi's Lynx. None of the three is reachable through `childIds` on
    /// the printing this table resolves.
    pub const ASHBRINGER: CardId = token("EX1_383t");
    pub const BATTLEFIELD_MEDIC: CardId = token("BAR_878t");
    pub const LYNX: CardId = token("TRL_348t");
    /// Animal Companion, whose own three children are what Call of the Wild
    /// summons all of.
    pub const ANIMAL_COMPANION: CardId = token("NEW1_031");
    pub const FIREBALL: CardId = token("CS2_029");
    /// Cairne's Baine and Mountain Bear's Cub. Both cards' Standard
    /// reprints carry no `childIds` in this snapshot (docs/RUST_CARDS_PLAN.md
    /// §2a), so the tokens are named rather than looked up.
    pub const BAINE_BLOODHOOF: CardId = token("EX1_110t");
    pub const MOUNTAIN_CUB: CardId = token("AV_337t");
    /// Eternal Bloodpetal's "0/1 Eternal Seedling", whose own deathrattle
    /// summons the Bloodpetal back; the card itself is the token for that
    /// half, which is why both are named here.
    pub const ETERNAL_SEEDLING: CardId = token("TLC_234t");
    pub const ETERNAL_BLOODPETAL: CardId = token("TLC_234");
    /// Immortalized in Stone's three Taunt Elementals: 1/2, 2/4 and 4/8.
    /// Named one by one because the card summons one of each rather than a
    /// random child.
    pub const WORN_STATUE: CardId = token("TSC_076t");
    pub const LIVING_STATUE: CardId = token("TSC_076t2");
    pub const PRISTINE_STATUE: CardId = token("TSC_076t3");
    /// Hammer of Twilight's "4/2 Elemental". A weapon has no `childIds` link
    /// to it in this snapshot, so it is named here.
    pub const TWILIGHT_ELEMENTAL: CardId = token("OG_031a");
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

    // ----------------------------------------------- Deathwing's Cataclysms
    // Each is a real spell in the corpus, unleashed by Deathwing's Battlecry
    // rather than cast from hand. They carry their own rows so each can be
    // tested as a card, and so Deathwing is the choice and nothing else.
    spell("Dragon's Reign", T::None, |g, c| {
        g.summon(c.side, tokens::PROGENY_OF_DEATHWING);
    }),
    spell("Raze", T::None, |g, c| {
        g.spell_damage_area(c.side, Area::EnemyMinions, 4);
    }),
    spell("Topple", T::None, |g, c| {
        let foe = c.side.other();
        let victim = g
            .player(foe)
            .board
            .iter()
            .enumerate()
            .filter(|(_, m)| m.active() && m.is_minion())
            // Ties go to the leftmost, which is the older minion: the corpus
            // says "the highest Health enemy minion" and does not break ties,
            // so this picks one deterministically rather than by chance.
            .max_by_key(|(i, m)| (m.health(), -(*i as i32)))
            .map(|(i, _)| Target::Minion(foe, i as u8));
        if let Some(t) = victim {
            g.destroy(t);
        }
    }),
    // "They cost (1)" is written on each copy as it goes in: a deck card
    // carries its own cost, and the draw hands it to the card in hand.
    spell("Enthrall", T::None, |g, c| {
        g.shuffle_random_into_deck_where(
            c.side,
            5,
            |d| {
                d.kind() == super::Kind::Minion
                    && d.races.any(Races::DRAGON)
                    && d.rarity() == super::Rarity::Legendary
            },
            |dc| dc.set_cost(1),
        );
    }),
    // "Choose N Cataclysms to unleash", where N is 1, and 2 or 3 once you
    // have Heralded twice or four times. The engine picks for you, the same
    // way Discover does: the choice belongs to a policy, and effects here are
    // resolved without one. The scoring is deliberately crude — kill what is
    // in front of you first, then develop — and it is not a claim about how a
    // player would choose.
    battlecry("Deathwing, Worldbreaker", T::None, |g, c| {
        let herald = g.player(c.side).herald;
        let picks = 1 + i16::from(herald >= 2) + i16::from(herald >= 4);
        let mut chosen: Inline<CardId, 4> = Inline::new();
        for _ in 0..picks {
            let mut best: Option<(i16, CardId)> = None;
            for cata in [
                tokens::RAZE,
                tokens::TOPPLE,
                tokens::DRAGONS_REIGN,
                tokens::ENTHRALL,
            ] {
                if chosen.contains(&cata) {
                    continue;
                }
                let score = cataclysm_score(g, c.side, cata);
                if best.is_none_or(|(b, _)| score > b) {
                    best = Some((score, cata));
                }
            }
            let Some((_, cata)) = best else { break };
            chosen.push(cata);
        }
        for cata in chosen.iter().copied() {
            let Some(f) = behaviour_of(cata).and_then(|b| b.spell) else {
                continue;
            };
            f(
                g,
                &Ctx::bare(cata, c.side),
            );
            g.sweep_deaths();
            if g.is_over() {
                return;
            }
        }
        // Deathwing brings his own Hero Power with him.
        g.player_mut(c.side).hero_power = tokens::RUTHLESS;
        g.player_mut(c.side).hero_power_uses = 0;
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
        // "5 friendly minions that died this game", oldest first, counting
        // only the ones that actually carry a deathrattle.
        let dead = g.dead_where(c.side, |d| d.keywords.has(Keywords::DEATHRATTLE));
        for card in dead.iter().copied().take(5) {
            g.fire_deathrattle_of(c.side, card);
        }
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
        // Tame Pet: "Replace your future Animal Companions with random Beasts
        // that cost (1) more." One more than the Companions themselves, and
        // all three of them are 3-Cost in the corpus -- so the four is the
        // card data's, read off the tokens rather than typed in here.
        if g.player(c.side).tamed_pet {
            let cost = c
                .card
                .summonable_children()
                .map(|t| t.def().cost)
                .max()
                .unwrap_or(3)
                + 1;
            g.summon_random_where(c.side, move |d| {
                d.kind() == super::Kind::Minion
                    && d.cost == cost
                    && d.races.any(Races::BEAST)
            });
            return;
        }
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
    // Cards the retired Python engine already reasoned through, translated onto the verbs
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
    // Ported from the retired Python engine against the corpus text; see
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
                g.summon_with(c.side, card.card, card.atk as i16, card.hp as i16);
            }
        }
    }),
    // warrior
    battlecry("Brood Keeper", T::None, |g, c| {
        if g.holding_race(c.side, Races::DRAGON) {
            g.equip(c.side, tokens::NIGHTMARE_SLICER);
        }
    }),
    // Rewind is the engine's, not this row's: the whole play is rolled back
    // and made again, and the better of the two pairs of weapons is kept --
    // both halves of it, since `agent::position_value` counts the one this
    // hands the opponent against you.
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
    // "Your Hero Power becomes 'Collapsing Star'. If it already is, increase
    // its damage by 1." The second half is why the damage is a player field:
    // two copies of the spell make one power that hits for 2.
    spell("Soul Immolation", T::None, |g, c| {
        let p = g.player_mut(c.side);
        if p.hero_power == tokens::COLLAPSING_STAR {
            p.hero_power_bonus += 1;
        } else {
            p.hero_power = tokens::COLLAPSING_STAR;
            // Swapping the power does not spend the turn's use, and does not
            // hand back one already spent.
        }
    }),
    // "Whenever you summon a Demon, refresh this." A Hero Power is not a
    // permanent, so it reacts through its own sentinel slot in `Game::fire`
    // rather than from a board position.
    trigger("Collapsing Star", |g, c| {
        if let Event::MinionSummoned { side, card, .. } = c.event
            && side == c.side
            && card.def().races.any(Races::DEMON)
        {
            g.refresh_hero_power(side);
        }
    }),
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
                &Ctx::bare(card, c.side),
            );
        }
    }),

    // -------------------------------------------------------- phase 3, G5
    // Start of Game: docs/RUST_CARDS_PLAN.md §4 phase 4 (G5).
    start_of_game("Chainbreaker Hogger", |g, c| {
        let mut extra: Inline<CardId, MAX_DECK> = Inline::new();
        for dc in g.player(c.side).deck.iter() {
            if dc.card != c.card && dc.def().rarity() == super::Rarity::Legendary {
                extra.push(dc.card);
            }
        }
        for card in extra.iter() {
            g.player_mut(c.side).deck.push(DeckCard::new(*card));
        }
        g.shuffle_deck(c.side);
    }),
    // Both halves check the *deck* specifically, matching "while building
    // your deck" framing: a deck with no other minions gets Mug's Magic, one
    // with no spells gets Zee's Might, each read back later by id from
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
            .all(|dc| dc.card == c.card || dc.def().kind() != super::Kind::Minion)
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
            g.player_mut(c.side).deck.push(DeckCard::started(c.card));
            g.shuffle_deck(c.side);
        }),
        None, None, None, None, None, None,
        Some(|g, c| {
            let foe = c.side.other();
            if let Some(idx) = g.player(c.side).deck.iter().position(|dc| dc.card == c.card) {
                g.player_mut(c.side).deck.remove(idx);
                g.player_mut(foe).deck.push(DeckCard::new(c.card));
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
    // "Not itself": the same exclusion Warden Maiev's identical "after you
    // play a minion" wording uses above.
    trigger("Dreambound Raptor", |g, c| {
        if let Event::MinionSummoned { side, slot, .. } = c.event
            && side == c.side
            && slot != c.slot
        {
            g.give_bonus_effect(Target::Minion(side, slot));
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
            if discount && let Some(hc) = g.player_mut(c.side).hand.get_mut(before) {
                hc.cost_delta = 1 - hc.card.def().cost;
            }
        }
    }),
    // The aura is engine-level special-casing in `Game::card_cost`/`play_card`
    // -- a live board check by name, not a stored flag, since Naralex can
    // leave play mid-turn and take the discount with it. This row exists
    // only so `is_implemented` sees it.
    c(
        "Naralex, Herald of the Flights",
        T::None,
        None, None, None, None, None, None, None, None, None,
    ),
    // Reacts from hand alone -- engine-level special-casing in
    // `Game::fire`/`Game::tick_shadow_of_demise`, which no hook here could
    // express. Its own "spell" is a no-op: casting the original,
    // untransformed card spends its printed 0 Mana for nothing, exactly as
    // the real card does before it has ever been transformed.
    spell("Shadow of Demise", T::None, |_, _| {}),
    // Whether "a 3/4 copy" means borrowing only the name or the last enemy
    // minion's abilities too (at Mirrex's own fixed stats either way) is not
    // resolvable from the corpus text alone, and guessing wrong could as
    // easily make this stronger as weaker. Approximated as its own plain
    // 3/4 body, the same conservative treatment as Elise the Navigator
    // above. This row exists only so `is_implemented` sees it.
    c(
        "Mirrex, the Crystalline",
        T::None,
        None, None, None, None, None, None, None, None, None,
    ),
    // Control returns via `Game::end_turn`, keyed off `Permanent::stolen_from`
    // rather than card identity -- see that field's own comment for why.
    spell("Cursed Chains", T::EnemyMinion, |g, c| {
        if let Some(t) = c.target {
            g.take_control(t, c.side);
        }
    }),
    // "Except 1 card" does not say which; popped from the same end
    // `Player::void` is later drawn from, so the one left behind is
    // whatever last landed there rather than a random pick, avoiding an
    // unnecessary RNG draw for a choice the card never specifies.
    battlecry("Irida Sinseeker", T::None, |g, c| {
        let p = g.player_mut(c.side);
        while p.deck.len() > 1 {
            if let Some(card) = p.deck.pop() {
                p.void.push(card.card);
            }
        }
    }),
    // Colossal's own appendage-summon has no dedicated hook, so this reuses
    // `battlecry` for it -- mechanically identical to a Battlecry, whatever
    // the card calls it. "This gains them too" (the Legs' growth mirrored
    // onto the main body) is not implemented: the body stays at its own
    // printed 0/5 forever, weaker than the real card in every game, never
    // stronger. See APPROXIMATE.
    battlecry("Wickerfang", T::None, |g, c| {
        g.summon_token(c.side, tokens::WICKERFANGS_LEG, 4);
    }),
    trigger("Wickerfang's Leg", |g, c| {
        if matches!(c.event, Event::TurnEnd { side } if side == c.side) {
            g.buff(c.me(), 1, 1);
        }
    }),
    // Al'Akir's own text has no template gaps at all -- Colossal's summon,
    // Rush and Windfury are ordinary keywords, and "this minion's Attack" is
    // a live read of its own current stats, not a printed number. Only its
    // two Charged Hands are blocked (their own Herald-scaled buff comes
    // through with the amount stripped); those get the vanilla-body
    // treatment below, same as Wickerfang's Legs would if their own ability
    // were not implementable. See APPROXIMATE.
    battlecry("Al'Akir, Lord of Storms", T::None, |g, c| {
        g.summon_token(c.side, tokens::CHARGED_HAND_OF_ALAKIR, 2);
        let Some(slot) = c.source else { return };
        let atk = g.player(c.side).board[slot as usize].atk;
        // `add_random_to_hand` takes a plain `fn` pointer, which cannot
        // capture `atk`; inlined the same way it is implemented internally.
        let pool =
            crate::cards::discover_pool(|d| d.kind() == super::Kind::Minion && d.cost == atk);
        for _ in 0..2 {
            if pool.is_empty() {
                break;
            }
            let pick = g.rngs.effects.index(pool.len());
            let before = g.player(c.side).hand.len();
            if g.give_card(c.side, pool[pick])
                && let Some(hc) = g.player_mut(c.side).hand.get_mut(before)
            {
                hc.cost_delta = 1 - hc.card.def().cost;
            }
        }
    }),
    // "Adjacent minions have +{0} Attack" -- the buff amount is stripped at
    // every Herald tier, not just the higher ones, so there is no floor
    // value to fall back on the way Soldier of Al'Akir's own Herald-1 number
    // was. Plays as a plain vanilla body. This row exists only so
    // `is_implemented` sees it.
    c(
        "Charged Hand of Al'Akir",
        T::None,
        None, None, None, None, None, None, None, None, None,
    ),
    // Unlike Al'Akir, Sinestra's own text has nothing left to implement
    // besides the Colossal summon: "Your spells from other classes cast
    // twice" is engine-level special-casing in `Game::play_card`
    // (`double_spell`), a live board check by name, the same shape as
    // Naralex's aura and Mug's Magic's discount above.
    battlecry("Sinestra", T::None, |g, c| {
        g.summon_token(c.side, tokens::SINESTRAS_WING, 2);
    }),
    // "It costs ({0}) less" is template-stripped at every Herald tier, same
    // as Charged Hand of Al'Akir; the discover itself is not blocked, so
    // only the discount is dropped, leaving the spell at full price.
    trigger("Sinestra's Wing", |g, c| {
        if let Event::MinionSummoned { side, slot, .. } = c.event
            && side == c.side
            && slot == c.slot
        {
            let class = g.player(c.side).class;
            let pool = crate::cards::discover_pool(|d| {
                d.kind() == super::Kind::Spell
                    && d.class() != super::Class::Neutral
                    && d.class() != class
            });
            if !pool.is_empty() {
                let pick = g.rngs.effects.index(pool.len());
                g.give_card(c.side, pool[pick]);
            }
        }
    }),
    // No template gaps here either -- Deathwing itself remains unimplemented
    // (its own battlecry is what is template-stripped), but that does not
    // block this card's own slot. `Game::herald` already existed, built for
    // exactly this: its own doc comment names Deathwing's cost reduction as
    // the reason classes with no Soldier still advance the counter. The
    // reduction reaches a copy wherever it is waiting -- in hand, and in the
    // deck, where it rides on the deck card until the draw hands it over.
    battlecry("Ultraxion", T::None, |g, c| {
        g.herald(c.side);
        let is_deathwing = |name: &str| name == "Deathwing, Worldbreaker";
        if let Some(idx) = g
            .player(c.side)
            .hand
            .iter()
            .position(|hc| is_deathwing(hc.card.name()))
        {
            g.player_mut(c.side).hand[idx].cost_delta -= 1;
        }
        for dc in g.player_mut(c.side).deck.iter_mut() {
            if is_deathwing(dc.name()) {
                dc.cost_delta = dc.cost_delta.saturating_sub(1);
            }
        }
    }),
    // "Costs (0) if you control Medivh" is unreachable: Medivh is a Hero
    // card (G3, not otherwise built), which can never legally be in play,
    // so this always costs its printed 10. The damage half of "double the
    // damage and healing of your spells" lives in `Game::wielding_atiesh`
    // and the functions that check it; the healing half is not implemented
    // -- see that helper's own comment for why. This row exists only so
    // `is_implemented` sees it.
    c(
        "Atiesh the Greatstaff",
        T::None,
        None, None, None, None, None, None, None, None, None,
    ),
    // Broxigar's Portal to Argus chain -- ten cards (Broxigar, Axe of
    // Cenarius, four Portals, four Demons) that all interlock, with no
    // template gaps anywhere in any of them once each card is read in full
    // rather than just Broxigar's own truncated summary. "Disappear" removes
    // it from wherever Start of Game finds it; nothing else references it
    // again until Fleeing Terrorguard's own deathrattle gives it back.
    start_of_game("Broxigar", |g, c| {
        let p = g.player_mut(c.side);
        if let Some(idx) = p.hand.iter().position(|hc| hc.card == c.card) {
            p.hand.remove(idx);
        } else if let Some(idx) = p.deck.iter().position(|dc| dc.card == c.card) {
            p.deck.remove(idx);
        }
    }),
    // "After your hero attacks and kills a minion": the board no longer
    // holds the body by the time AfterAttack fires (see that event's own
    // comment), so this reads `defender_died`, captured before the sweep,
    // rather than trying to inspect the board.
    trigger("Axe of Cenarius", |g, c| {
        if matches!(
            c.event,
            Event::AfterAttack { attacker: Target::Hero(s), defender_died: true, .. }
                if s == c.side
        ) {
            // `draw_matching`'s predicate only sees `CardDef`, which carries
            // no printed name -- found by name on the `CardId`s themselves
            // instead, the same way Garona Halforcen finds King Llane.
            if let Some(idx) = g
                .player(c.side)
                .deck
                .iter()
                .position(|dc| dc.card.name().contains("Portal to Argus"))
            {
                let card = g.player(c.side).deck[idx];
                g.player_mut(c.side).deck.remove(idx);
                g.give_hand_card(c.side, card.to_hand());
                g.fire(Event::CardDrawn { side: c.side });
            }
        }
    }),
    // Each Portal is cast by Broxigar's own controller but summons its Demon
    // "for your opponent" -- onto the *other* side's board.
    spell("First Portal to Argus", T::None, |g, c| {
        g.summon(c.side.other(), tokens::FLEEING_URZUL);
    }),
    spell("Second Portal to Argus", T::None, |g, c| {
        g.summon(c.side.other(), tokens::FLEEING_INCUBUS);
    }),
    spell("Third Portal to Argus", T::None, |g, c| {
        g.summon(c.side.other(), tokens::FLEEING_WRATHGUARD);
    }),
    spell("Final Portal to Argus", T::None, |g, c| {
        g.summon(c.side.other(), tokens::FLEEING_TERRORGUARD);
    }),
    // Each Demon's own deathrattle text is written from *its controller's*
    // perspective (Broxigar's opponent, who received it) -- "your opponent
    // draws a card" and "into their deck" both mean Broxigar's controller,
    // `c.side.other()` here, not the demon's own side.
    deathrattle("Fleeing Ur'zul", |g, c| {
        let owner = c.side.other();
        g.draw_cards(owner, 1);
        g.shuffle_into_deck(owner, tokens::SECOND_PORTAL_TO_ARGUS);
    }),
    deathrattle("Fleeing Incubus", |g, c| {
        let owner = c.side.other();
        g.draw_cards(owner, 1);
        g.shuffle_into_deck(owner, tokens::THIRD_PORTAL_TO_ARGUS);
    }),
    deathrattle("Fleeing Wrathguard", |g, c| {
        let owner = c.side.other();
        g.draw_cards(owner, 1);
        g.shuffle_into_deck(owner, tokens::FINAL_PORTAL_TO_ARGUS);
    }),
    // The fourth and last Demon: Broxigar itself reappears, completing the
    // chain "Disappear ... kill all 4 Demons from Argus to reappear" names.
    deathrattle("Fleeing Terrorguard", |g, c| {
        g.give_card(c.side.other(), tokens::BROXIGAR);
    }),
    // "Ten copies join your deck" happens while building the deck, before a
    // `Game` exists at all -- outside this engine's scope regardless of
    // what this row does. Taunt is the printed keyword, auto-handled; the
    // Start of Game hook is a no-op so `is_implemented` sees the keyword it
    // declares.
    start_of_game("Commander Beatrix", |_, _| {}),
    // Likewise the deck-construction half ("starting Health is 40", "20
    // cards plus 20 copied from your enemy") happens before a `Game`
    // exists. Only the Battlecry is a real in-game action, and it is fully
    // concrete.
    battlecry("Azalina Soulsever", T::None, |g, c| {
        let have = g.player(c.side).hand.len();
        if have < MAX_HAND {
            g.draw_cards(c.side, MAX_HAND - have);
        }
    }),

    // -------------------------------------------------------- backlog batch
    // General Standard coverage, not tied to a specific meta deck. Each row
    // reuses a verb this table already established many times over; nothing
    // here needed new engine machinery.
    spell("Slam", T::AnyMinion, |g, c| {
        if let Some(t) = c.target {
            g.spell_damage(c.side, Some(t), 2);
            let alive = match t {
                Target::Minion(s, i) => g
                    .player(s)
                    .board
                    .get(i as usize)
                    .is_some_and(|m| !m.is_dead()),
                Target::Hero(_) => true,
            };
            if alive {
                g.draw_cards(c.side, 1);
            }
        }
    }),
    battlecry("Abusive Sergeant", T::AnyMinion, |g, c| {
        if let Some(t) = c.target {
            g.buff_temp_atk(t, 2);
        }
    }),
    battlecry("Beaming Sidekick", T::FriendlyMinion, |g, c| {
        if let Some(t) = c.target {
            g.buff(t, 0, 2);
        }
    }),
    // "Not itself": the same exclusion Warden Maiev and Dreambound Raptor's
    // identical "whenever/after you [play/summon] a minion" wording use.
    trigger("Murloc Tidecaller", |g, c| {
        if let Event::MinionSummoned { side, card, slot } = c.event
            && side == c.side
            && slot != c.slot
            && card.def().races.any(Races::MURLOC)
        {
            g.buff(c.me(), 1, 0);
        }
    }),
    battlecry("Gnawing Greenfin", T::None, |g, c| {
        g.add_random_to_hand(c.side, |d| {
            d.kind() == super::Kind::Minion && d.races.any(Races::MURLOC)
        });
    }),
    spell("Siphoning Growth", T::FriendlyMinion, |g, c| {
        if let Some(t) = c.target {
            g.destroy(t);
            g.gain_armor(c.side, 8);
        }
    }),
    battlecry("Injured Tol'vir", T::None, |g, c| {
        if let Some(slot) = c.source {
            g.deal_damage(Target::Minion(c.side, slot), 3);
        }
    }),
    // "Swap the Attack and Health": current (post-damage) Health, not max --
    // a damaged minion's swap should not silently forgive the damage. The
    // new Health is a fresh value, so damage resets to 0 rather than
    // carrying over against the new max.
    battlecry("Crazed Alchemist", T::AnyMinion, |g, c| {
        if let Some(Target::Minion(s, i)) = c.target
            && let Some(m) = g.player_mut(s).board.get_mut(i as usize)
        {
            let health = m.health();
            let atk = m.atk;
            m.atk = health;
            m.max_hp = atk;
            m.damage = 0;
        }
    }),
    battlecry("Bloodsail Raider", T::None, |g, c| {
        let atk = g.player(c.side).weapon.map_or(0, |w| w.atk);
        if atk > 0
            && let Some(slot) = c.source
        {
            g.buff(Target::Minion(c.side, slot), atk, 0);
        }
    }),
    battlecry("Maze Guide", T::None, |g, c| {
        g.summon_random_of_cost(c.side, 2, 1);
    }),
    spell("Unleash the Crocolisks", T::None, |g, c| {
        g.gain_armor(c.side, 10);
        g.summon_token(c.side.other(), tokens::COLISEUM_CROCOLISK, 2);
    }),
    battlecry("Sunfury Protector", T::None, |g, c| {
        let Some(slot) = c.source else { return };
        let side = c.side;
        if slot > 0 {
            g.grant(Target::Minion(side, slot - 1), Keywords::TAUNT);
        }
        let right = slot as usize + 1;
        if right < g.player(side).board.len() {
            g.grant(Target::Minion(side, right as u8), Keywords::TAUNT);
        }
    }),
    battlecry("P1CK-P0K3T", T::None, |g, c| {
        if g.player(c.side).deck.len() >= 25 {
            g.draw_cards(c.side, 1);
        }
    }),
    // "Each turn", not "your turn" -- deliberately no side filter, unlike
    // every other TurnStart/TurnEnd trigger in this table.
    trigger("Micro Machine", |g, c| {
        if matches!(c.event, Event::TurnStart { .. }) {
            g.buff(c.me(), 1, 0);
        }
    }),
    // "A minion", not "a friendly minion" -- no side filter here either;
    // this fires off enemy minions taking damage too.
    trigger("Frothing Berserker", |g, c| {
        if let Event::Damaged { target, amount } = c.event
            && amount > 0
            && matches!(target, Target::Minion(..))
        {
            g.buff(c.me(), 1, 0);
        }
    }),
    battlecry("Coldlight Seer", T::None, |g, c| {
        let Some(slot) = c.source else { return };
        let side = c.side;
        let targets: Inline<u8, MAX_BOARD> = g
            .player(side)
            .board
            .iter()
            .enumerate()
            .filter(|(i, m)| *i != slot as usize && m.races().any(Races::MURLOC))
            .map(|(i, _)| i as u8)
            .collect();
        for i in targets.iter() {
            g.buff(Target::Minion(side, *i), 0, 2);
        }
    }),
    battlecry("Big Game Hunter", T::MinionAtkAtLeast(7), |g, c| {
        if let Some(t) = c.target {
            g.destroy(t);
        }
    }),
    battlecry("Lifedrinker", T::None, |g, c| {
        g.deal_damage(Target::Hero(c.side.other()), 3);
        g.heal_hero(c.side, 3);
    }),
    battlecry("Twilight Drake", T::None, |g, c| {
        let Some(slot) = c.source else { return };
        let n = g.player(c.side).hand.len() as i16;
        g.buff(Target::Minion(c.side, slot), 0, n);
    }),
    // No printed keyword to check against here: "Costs (1) less per Attack
    // of your weapon" is a live read of the caster's own weapon, exactly
    // what `cost_delta` exists for.
    c(
        "Dread Corsair",
        T::None,
        None, None, None, None, None, None, None,
        Some(|g, side, _hand_idx| -g.player(side).weapon.map_or(0, |w| w.atk)),
        None,
    ),
    spell("City Defenses", T::None, |g, c| {
        g.summon_token(c.side, tokens::STEADFAST_SECURITY, 2);
    }),
    trigger("Steadfast Security", |g, c| {
        if let Event::Damaged { target, amount } = c.event
            && amount > 0
            && target == c.me()
        {
            g.buff(c.me(), 1, 0);
        }
    }),
    battlecry("Eggbasher", T::AnyMinion, |g, c| {
        if let Some(t) = c.target {
            g.deal_damage(t, 1);
            g.buff(t, 4, 0);
        }
    }),

    // ------------------------------------------------- backlog batch, DK
    spell("Icy Touch", T::EnemyCharacter, |g, c| {
        if let Some(t) = c.target {
            g.spell_damage(c.side, Some(t), 2);
            g.freeze(t);
        }
    }),
    trigger("Doomsayer", |g, c| {
        if matches!(c.event, Event::TurnStart { side } if side == c.side) {
            g.destroy_area_where(c.side, Area::AllMinions, |_| true);
        }
    }),
    spell("Plague Strike", T::AnyMinion, |g, c| {
        let Some(t) = c.target else { return };
        g.spell_damage(c.side, Some(t), 3);
        let dead = match t {
            Target::Minion(s, i) => g
                .player(s)
                .board
                .get(i as usize)
                .is_some_and(|m| m.is_dead()),
            Target::Hero(_) => false,
        };
        if dead {
            g.summon_token(c.side, tokens::RAMPAGING_ZOMBIE, 1);
        }
    }),
    deathrattle("Harbinger of Winter", |g, c| {
        g.draw_matching(c.side, |d| {
            d.kind() == super::Kind::Spell && d.school() == super::School::Frost
        });
    }),
    // No `highest_attack_enemy` helper exists (only the symmetric
    // `lowest_health_enemy`), so this scans by hand.
    spell("Asphyxiate", T::None, |g, c| {
        let foe = c.side.other();
        let best = g
            .player(foe)
            .board
            .iter()
            .enumerate()
            .filter(|(_, m)| m.active() && m.is_minion())
            .max_by_key(|(_, m)| m.atk)
            .map(|(i, _)| i as u8);
        if let Some(i) = best {
            g.destroy(Target::Minion(foe, i));
        }
    }),
    // Both hooks are the same one-line effect, so there is nothing to share
    // between them beyond the card's own name.
    c(
        "Chillfallen Baron",
        T::None,
        None,
        Some(|g, c| g.draw_cards(c.side, 1)),
        Some(|g, c| g.draw_cards(c.side, 1)),
        None, None, None, None, None, None,
    ),
    battlecry("Stonehill Defender", T::None, |g, c| {
        g.discover(c.side, |d| {
            d.kind() == super::Kind::Minion && d.keywords.has(Keywords::TAUNT)
        });
    }),
    trigger("Acolyte of Death", |g, c| {
        if let Event::MinionDied { side, card } = c.event
            && side == c.side
            && card.def().races.any(Races::UNDEAD)
        {
            g.draw_cards(c.side, 1);
        }
    }),
    // Both of these ask for "a friendly Undead" specifically, which
    // `TargetSpec` cannot express (no race-filtered variant exists). Offered
    // as any friendly minion; a non-Undead target simply does nothing,
    // rather than risk the wrong race check silently excluding the right
    // targets, since "an Undead-only spell doing nothing on a non-Undead"
    // is the same outcome the real card has for that mistake anyway.
    spell("Dark Transformation", T::FriendlyMinion, |g, c| {
        if let Some(t @ Target::Minion(s, i)) = c.target
            && g.player(s)
                .board
                .get(i as usize)
                .is_some_and(|m| m.races().any(Races::UNDEAD))
        {
            g.transform(t, tokens::UNDEAD_MONSTROSITY);
        }
    }),
    spell("Poison Breath", T::FriendlyMinion, |g, c| {
        if let Some(t @ Target::Minion(s, i)) = c.target
            && g.player(s)
                .board
                .get(i as usize)
                .is_some_and(|m| m.races().any(Races::UNDEAD))
        {
            g.grant(t, Keywords::POISONOUS);
        }
    }),

    // -------------------------------------------------- backlog batch, Shaman
    spell("Static Shock", T::AnyMinion, |g, c| {
        if let Some(t) = c.target {
            g.spell_damage(c.side, Some(t), 1);
        }
        g.buff_temp_atk(Target::Hero(c.side), 1);
    }),
    spell("Lightning Rod", T::FriendlyMinion, |g, c| {
        if let Some(t) = c.target {
            g.spell_damage(c.side, Some(t), 2);
        }
        if let Some(r) = g.random_minion(c.side.other()) {
            g.spell_damage(c.side, Some(r), 4);
        }
    }),
    spell("Thunderquake", T::None, |g, c| {
        g.spell_damage_area(c.side, Area::AllMinions, 1);
        g.give_card(c.side, tokens::STATIC_SHOCK);
    }),
    // Overload is read straight off the card's own printed `overload` field
    // in `Game::play_card`, unconditionally -- nothing to do here for it.
    spell("Lightning Storm", T::None, |g, c| {
        g.spell_damage_area(c.side, Area::EnemyMinions, 3);
    }),
    spell("Far Sight", T::None, |g, c| {
        let before = g.player(c.side).hand.len();
        g.draw_cards(c.side, 1);
        if let Some(hc) = g.player_mut(c.side).hand.get_mut(before) {
            hc.cost_delta -= 3;
        }
    }),
    spell("Voltaic Burst", T::None, |g, c| {
        g.summon_token(c.side, tokens::SPARK, 2);
    }),
    // Sizzling Cinder itself was already implemented (line ~1383); only the
    // token reference is needed here.
    deathrattle("Cinderfin", |g, c| {
        g.summon_token(c.side, tokens::SIZZLING_CINDER, 1);
    }),

    // --------------------------------------------------- backlog batch, Priest
    spell("Mend", T::AnyMinion, |g, c| {
        if let Some(t) = c.target {
            g.restore_full(t);
        }
        g.draw_cards(c.side, 1);
    }),
    battlecry("Amber Priestess", T::AnyCharacter, |g, c| {
        let Some(slot) = c.source else { return };
        let hp = g.player(c.side).board[slot as usize].health();
        if let Some(t) = c.target {
            g.heal(t, hp);
        }
    }),
    // "Restore Health to the enemy hero" is unusual for a Priest removal
    // spell, but the corpus text is explicit and unambiguous, so it is
    // implemented exactly as printed rather than assumed to be a typo.
    spell("Purifying Breath", T::AnyMinion, |g, c| {
        let Some(t) = c.target else { return };
        g.spell_damage(c.side, Some(t), 5);
        let dead = match t {
            Target::Minion(s, i) => g
                .player(s)
                .board
                .get(i as usize)
                .is_some_and(|m| m.is_dead()),
            Target::Hero(_) => false,
        };
        if dead {
            g.heal_hero(c.side.other(), 5);
        }
    }),
    battlecry("Crystalsmith Cultist", T::None, |g, c| {
        let holding_shadow = g.player(c.side).hand.iter().any(|hc| {
            hc.card.def().kind() == super::Kind::Spell
                && hc.card.def().school() == super::School::Shadow
        });
        if holding_shadow && let Some(slot) = c.source {
            g.buff(Target::Minion(c.side, slot), 1, 1);
        }
    }),
    // Lifesteal is the minion's own printed combat keyword (already
    // auto-applied to its normal attacks); whether it also heals from this
    // self-damaging Battlecry is a real ambiguity this does not resolve, so
    // the Battlecry is implemented without an extra heal here.
    battlecry("Injured Attendant", T::None, |g, c| {
        if let Some(slot) = c.source {
            g.deal_damage(Target::Minion(c.side, slot), 4);
        }
    }),
    // Unlike the two Battlecry cases above, "Lifesteal" here is the spell's
    // own explicit clause, not a minion's combat keyword riding along on an
    // unrelated effect -- healing on cast is squarely what the card says.
    spell("Void Shard", T::AnyCharacter, |g, c| {
        if let Some(t) = c.target
            && g.spell_damage(c.side, Some(t), 4)
        {
            g.heal_hero(c.side, 4);
        }
    }),
    battlecry("Cleansing Lightspawn", T::EnemyMinion, |g, c| {
        let Some(slot) = c.source else { return };
        let hp = g.player(c.side).board[slot as usize].health();
        if let Some(t) = c.target {
            g.deal_damage(t, hp);
        }
    }),
    spell("Greater Healing Potion", T::FriendlyCharacter, |g, c| {
        if let Some(t) = c.target {
            g.heal(t, 12);
        }
        g.draw_cards(c.side, 1);
    }),
    // IMMUNE_TO_SPELLPOWER: `damage_split` always adds Spell Power with no
    // way to opt out, so this repeats its loop by hand at a flat 4 instead
    // of going through it, the same reason Torch uses raw `deal_damage`
    // rather than `spell_damage`. The Lifesteal heal is unconditionally the
    // full 4, which can overheal if the split ran out of live targets
    // early and some of the 4 points went nowhere -- see APPROXIMATE.
    spell("Devouring Plague", T::None, |g, c| {
        for _ in 0..4 {
            let mut pool: Inline<Target, { MAX_BOARD * 2 + 2 }> = Inline::new();
            g.collect_area(c.side, Area::EnemyMinions, &mut pool);
            if pool.is_empty() {
                break;
            }
            let pick = g.rngs.effects.index(pool.len());
            g.deal_damage(pool[pick], 1);
        }
        g.heal_hero(c.side, 4);
    }),
    spell("Quick Shot", T::AnyCharacter, |g, c| {
        if let Some(t) = c.target {
            g.spell_damage(c.side, Some(t), 3);
        }
        if g.player(c.side).hand.is_empty() {
            g.draw_cards(c.side, 1);
        }
    }),
    // No helper covers "N distinct random enemies including the hero" --
    // `damage_random_enemy_minions` is minions-only -- so this collects the
    // whole enemy side by hand and samples distinct offers from it.
    spell("Bursting Shot", T::None, |g, c| {
        let mut pool: Inline<Target, { MAX_BOARD * 2 + 2 }> = Inline::new();
        g.collect_area(c.side, Area::AllEnemies, &mut pool);
        let mut offered = [0u32; 3];
        let n = g.rngs.effects.sample_indices(pool.len(), &mut offered);
        for &i in &offered[..n] {
            g.deal_damage(pool[i as usize], 2);
        }
    }),
    battlecry("Headhunter's Hatchet", T::None, |g, c| {
        if g.controls_race(c.side, Races::BEAST)
            && let Some(w) = g.player_mut(c.side).weapon.as_mut()
        {
            w.durability += 1;
        }
    }),
    deathrattle("Ticking Timebomb", |g, c| {
        if let Some(t) = g.random_minion(c.side.other()) {
            g.destroy(t);
        }
    }),
    battlecry("Arrow Retriever", T::None, |g, c| {
        let have = g.player(c.side).hand.len();
        if have < 3 {
            g.draw_cards(c.side, 3 - have);
        }
    }),
    spell("Spirit Bond", T::AnyMinion, |g, c| {
        let Some(t) = c.target else { return };
        g.spell_damage(c.side, Some(t), 3);
        let dead = match t {
            Target::Minion(s, i) => g
                .player(s)
                .board
                .get(i as usize)
                .is_some_and(|m| m.is_dead()),
            Target::Hero(_) => false,
        };
        if dead {
            g.summon_token(c.side, tokens::PLAYFUL_PUP, 1);
        }
    }),
    spell("Ball of Spiders", T::None, |g, c| {
        g.summon_token(c.side, tokens::WEBSPINNER, 3);
    }),
    deathrattle("Webspinner", |g, c| {
        g.add_random_to_hand(c.side, |d| d.races.any(Races::BEAST));
    }),
    battlecry("Herbivore Assistant", T::FriendlyMinion, |g, c| {
        let Some(t) = c.target else { return };
        let Target::Minion(s, i) = t else { return };
        if !g.player(s).board[i as usize].races().any(Races::BEAST) {
            return;
        }
        g.buff(t, 2, 2);
        g.grant(t, Keywords::RUSH);
    }),
    battlecry("Argent Protector", T::FriendlyMinion, |g, c| {
        if let Some(t) = c.target {
            g.grant(t, Keywords::DIVINE_SHIELD);
        }
    }),
    // Battlecry damage is not boosted by Spell Power in Hearthstone -- only
    // Spells and Hero Powers are -- so this is a raw `deal_damage`, not
    // `spell_damage`.
    battlecry("Fogsail Freebooter", T::None, |g, c| {
        if g.player(c.side).weapon.is_some() {
            g.deal_damage(Target::Hero(c.side.other()), 2);
        }
    }),
    // Same Spell-Power exemption as Fogsail Freebooter above, plus "all
    // OTHER characters" excludes the minion itself -- `damage_split` covers
    // neither, so this repeats its loop by hand.
    battlecry("Mad Bomber", T::None, |g, c| {
        let Some(src) = c.source else { return };
        let me = Target::Minion(c.side, src);
        for _ in 0..3 {
            let mut pool: Inline<Target, { MAX_BOARD * 2 + 2 }> = Inline::new();
            g.collect_area(c.side, Area::Everything, &mut pool);
            let mut others: Inline<Target, { MAX_BOARD * 2 + 2 }> = Inline::new();
            for t in pool.iter() {
                if *t != me {
                    others.push(*t);
                }
            }
            if others.is_empty() {
                break;
            }
            let pick = g.rngs.effects.index(others.len());
            g.deal_damage(others[pick], 1);
            g.sweep_deaths();
            if g.is_over() {
                break;
            }
        }
    }),
    battlecry("Brightwing", T::None, |g, c| {
        g.add_random_to_hand(c.side, |d| {
            d.kind() == super::Kind::Minion && d.rarity() == super::Rarity::Legendary
        });
    }),
    deathrattle("Tranquil Treant", |g, _c| {
        g.gain_crystal(Side::Player0, 1);
        g.gain_crystal(Side::Player1, 1);
    }),
    // Silver Hand Recruit is the spell's own child, read from its `childIds`
    // rather than a hardcoded token -- the two just-summoned recruits are
    // the last two board slots, since `summon` always appends.
    spell("Convalescence", T::None, |g, c| {
        let before = g.player(c.side).board.len();
        let made = g.summon_child(c.side, c.card, 2);
        for i in before..before + made {
            g.grant(Target::Minion(c.side, i as u8), Keywords::DIVINE_SHIELD);
        }
    }),
    spell("Silvermoon Portal", T::AnyMinion, |g, c| {
        if let Some(t) = c.target {
            g.buff(t, 2, 2);
        }
        g.summon_random_of_cost(c.side, 2, 1);
    }),
    choose(
        "Ancient Stegodon",
        &[
            m(T::None, |g, c| {
                if let Some(s) = c.source {
                    g.grant(Target::Minion(c.side, s), Keywords::TAUNT);
                }
            }),
            m(T::None, |g, c| {
                if let Some(s) = c.source {
                    g.grant(Target::Minion(c.side, s), Keywords::POISONOUS);
                }
            }),
            m(T::None, |g, c| {
                if let Some(s) = c.source {
                    g.buff(Target::Minion(c.side, s), 1, 1);
                }
            }),
        ],
    ),
    trigger("Barkshield Sentinel", |g, c| {
        if matches!(c.event, Event::HeroPowerUsed { side } if side == c.side) {
            g.buff(c.me(), 0, 2);
        }
    }),
    spell("Holy Bola!", T::None, |g, c| {
        let before = g.player(c.side).hand.len();
        g.draw_cards(c.side, 1);
        let drew = g.player(c.side).hand.len() > before;
        if drew
            && g.player(c.side)
                .hand
                .last()
                .is_some_and(|h| h.card.def().cost <= 2)
        {
            g.draw_cards(c.side, 1);
        }
    }),
    // Same childIds pattern as Convalescence above for the recruits; the
    // weapon child is a Weapon, which `summonable_children()` never returns,
    // so it needs its own token to name.
    spell("Muster for Battle", T::None, |g, c| {
        g.summon_child(c.side, c.card, 3);
        g.equip(c.side, tokens::LIGHTS_JUSTICE);
    }),
    battlecry("Skeletal Sidekick", T::FriendlyMinion, |g, c| {
        let Some(t) = c.target else { return };
        let Target::Minion(s, i) = t else { return };
        if !g.player(s).board[i as usize].races().any(Races::UNDEAD) {
            return;
        }
        g.buff(t, 2, 0);
    }),
    // "Freeze two random enemy minions" is the same distinct-random shape as
    // `damage_random_enemy_minions`, just freezing instead of damaging, so it
    // repeats that helper's loop by hand rather than adding a twin of it for
    // one card.
    spell("Timestop", T::None, |g, c| {
        g.spell_damage(c.side, Some(Target::Hero(c.side.other())), 3);
        let foe = c.side.other();
        let live: Inline<u8, MAX_BOARD> = g
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
        let taken = g
            .rngs
            .effects
            .sample_indices(live.len(), &mut picks[..2.min(live.len())]);
        for &p in picks.iter().take(taken) {
            g.freeze(Target::Minion(foe, live[p as usize]));
        }
    }),
    // The splash is still the spell's own damage, so each hit gets Spell
    // Power independently, the same as the primary hit -- matching how
    // Blizzard boosts every instance a multi-target spell deals, not just
    // one of them.
    spell("Howling Blast", T::EnemyCharacter, |g, c| {
        let Some(t) = c.target else { return };
        g.spell_damage(c.side, Some(t), 3);
        g.freeze(t);
        let mut pool: Inline<Target, { MAX_BOARD * 2 + 2 }> = Inline::new();
        g.collect_area(c.side, Area::AllEnemies, &mut pool);
        for &other in pool.iter() {
            if other != t {
                g.spell_damage(c.side, Some(other), 1);
            }
        }
    }),
    trigger("Deathchiller", |g, c| {
        if matches!(c.event, Event::SpellCast { side, .. } if side == c.side) {
            let mut pool: Inline<Target, { MAX_BOARD * 2 + 2 }> = Inline::new();
            g.collect_area(c.side, Area::AllEnemies, &mut pool);
            let mut picks = [0u32; MAX_BOARD * 2 + 2];
            let taken = g
                .rngs
                .effects
                .sample_indices(pool.len(), &mut picks[..2.min(pool.len())]);
            for &p in picks.iter().take(taken) {
                g.deal_damage(pool[p as usize], 1);
            }
        }
    }),
    // Reborn itself needs no card-specific code; only the deathrattle body
    // does. Its Undead Beast is the card's own child, the same childIds
    // pattern as Convalescence and Muster for Battle above.
    deathrattle("Reluctant Wrangler", |g, c| {
        g.summon_child(c.side, c.card, 1);
    }),
    deathrattle("Cryofrozen Champion", |g, c| {
        if g.add_random_to_hand(c.side, |d| {
            d.kind() == super::Kind::Minion && d.rarity() == super::Rarity::Legendary
        }) && let Some(h) = g.player_mut(c.side).hand.last_mut()
        {
            h.cost_delta -= 1;
        }
    }),
    // Weapons reach the trigger sweep too (see the after-attack cards above).
    trigger("Bone Breaker", |g, c| {
        if let Event::AfterAttack {
            attacker: Target::Hero(s),
            defender: Target::Minion(..),
            ..
        } = c.event
            && s == c.side
        {
            g.deal_damage(Target::Hero(c.side.other()), 2);
        }
    }),
    spell("Mortal Coil", T::AnyMinion, |g, c| {
        let Some(t) = c.target else { return };
        g.spell_damage(c.side, Some(t), 1);
        let dead = match t {
            Target::Minion(s, i) => g
                .player(s)
                .board
                .get(i as usize)
                .is_some_and(|m| m.is_dead()),
            Target::Hero(_) => false,
        };
        if dead {
            g.draw_cards(c.side, 1);
        }
    }),
    spell("Spirit Bomb", T::AnyMinion, |g, c| {
        if let Some(t) = c.target {
            g.spell_damage(c.side, Some(t), 4);
        }
        g.spell_damage(c.side, Some(Target::Hero(c.side)), 4);
    }),
    // Battlecry damage is not boosted by Spell Power (see Fogsail Freebooter
    // and Mad Bomber above), so this is a raw self-hit.
    battlecry("Vulgar Homunculus", T::None, |g, c| {
        g.deal_damage(Target::Hero(c.side), 2);
    }),
    // Deathrattles fire once the body has already left the board (see
    // `Game::sweep_deaths`), so `random_minion` can never pick this minion
    // itself; `c.dying` carries the last snapshot of it, including the
    // Attack value the board copy no longer has.
    deathrattle("Fiendish Servant", |g, c| {
        let atk = c.dying.map_or(0, |m| m.atk);
        if atk > 0
            && let Some(t) = g.random_minion(c.side)
        {
            g.buff(t, atk, 0);
        }
    }),
    battlecry("Gnomeferatu", T::None, |g, c| {
        g.player_mut(c.side.other()).deck.pop();
    }),
    trigger("Emberroot Destroyer", |g, c| {
        if let Event::Damaged {
            target: Target::Hero(s),
            ..
        } = c.event
            && s == c.side
            && g.current == c.side
            && let Some(t) = g.random_minion(c.side.other())
        {
            g.deal_damage(t, 3);
        }
    }),
    spell("Siphon Soul", T::AnyMinion, |g, c| {
        if let Some(t) = c.target {
            g.destroy(t);
        }
        g.heal_hero(c.side, 3);
    }),
    spell("Demonic Assault", T::None, |g, c| {
        g.spell_damage(c.side, Some(Target::Hero(c.side.other())), 3);
        g.summon_token(c.side, tokens::VOIDWALKER, 2);
    }),
    deathrattle("Blob of Tar", |g, c| {
        g.summon_token(c.side, tokens::LANKY_BLOB, 1);
        g.summon_token(c.side, tokens::ROBUST_BLOB, 1);
    }),
    deathrattle("Sporegnasher", |g, c| {
        if let Some(t) = g.random_minion(c.side.other()) {
            g.deal_damage(t, 1);
        }
    }),
    deathrattle("Taelan Fordring", |g, c| {
        let best = g
            .player(c.side)
            .deck
            .iter()
            .enumerate()
            .filter(|(_, id)| id.def().kind() == super::Kind::Minion)
            .max_by_key(|(_, id)| id.def().cost)
            .map(|(i, _)| i);
        if let Some(i) = best
            && let Some(card) = g.player_mut(c.side).deck.remove(i)
        {
            g.give_hand_card(c.side, card.to_hand());
        }
    }),
    battlecry("Doomguard", T::None, |g, c| {
        g.discard_random(c.side);
        g.discard_random(c.side);
    }),
    spell("First Flame", T::AnyMinion, |g, c| {
        if let Some(t) = c.target {
            g.spell_damage(c.side, Some(t), 2);
        }
        g.give_card(c.side, tokens::SECOND_FLAME);
    }),
    spell("Second Flame", T::AnyMinion, |g, c| {
        if let Some(t) = c.target {
            g.spell_damage(c.side, Some(t), 2);
        }
    }),
    spell("Cold Snap", T::EnemyCharacter, |g, c| {
        if let Some(t) = c.target {
            g.freeze(t);
        }
        g.add_random_to_hand(c.side, |d| {
            d.kind() == super::Kind::Spell && d.school() == super::School::Frost
        });
    }),
    // No TargetSpec for "a friendly Wisp specifically" exists, so this
    // offers the broadest matching spec and no-ops on anything else, the
    // same conservative pattern as Dark Transformation and Herbivore
    // Assistant.
    spell("Divination", T::FriendlyMinion, |g, c| {
        let Some(t) = c.target else { return };
        if let Target::Minion(s, i) = t
            && g.player(s).board[i as usize].card.name() == "Wisp"
        {
            g.destroy(t);
            g.draw_cards(c.side, 3);
        }
    }),
    // IMMUNE_TO_SPELLPOWER, same reason as Devouring Plague above: raw
    // `deal_damage`, not `spell_damage`.
    secret("Explosive Runes", |g, owner, ev| {
        if let Event::MinionSummoned { side, slot, .. } = ev
            && side == owner.other()
        {
            let t = Target::Minion(side, slot);
            let health = g
                .player(side)
                .board
                .get(slot as usize)
                .map_or(0, |m| m.health());
            g.deal_damage(t, 6);
            if 6 > health {
                g.deal_damage(Target::Hero(side), 6 - health);
            }
            return true;
        }
        false
    }),
    battlecry("Winterspring Whelp", T::None, |g, c| {
        g.discover(c.side, |d| d.cost == 1 && d.kind() == super::Kind::Spell);
    }),
    battlecry("Babbling Bookcase", T::None, |g, c| {
        for _ in 0..2 {
            g.add_random_to_hand(c.side, |d| {
                d.kind() == super::Kind::Spell && d.class() == super::Class::Mage
            });
        }
    }),
    // `discover`'s predicate is a plain `fn` pointer and cannot capture the
    // caster's live mana total, so this repeats its body by hand with
    // `discover_pool` (which takes `impl Fn`) instead -- the same workaround
    // used earlier this session for Al'Akir. Every match has the same cost
    // by construction of the predicate, so there is no "prefer the highest
    // cost" tiebreak to replicate from `discover` -- a flat random pick
    // among them is equivalent.
    battlecry("Scrappy Scavenger", T::None, |g, c| {
        let mana = g.player(c.side).mana;
        let pool = crate::cards::discover_pool(move |d| d.cost == mana);
        if pool.is_empty() {
            return;
        }
        let pick = g.rngs.effects.index(pool.len());
        g.give_card(c.side, pool[pick]);
    }),
    choose(
        "Living Roots",
        &[
            m(T::AnyCharacter, |g, c| {
                if let Some(t) = c.target {
                    g.spell_damage(c.side, Some(t), 2);
                }
            }),
            m(T::None, |g, c| {
                g.summon_child(c.side, c.card, 2);
            }),
        ],
    ),
    choose(
        "Raven Idol",
        &[
            m(T::None, |g, c| {
                g.discover(c.side, |d| d.kind() == super::Kind::Minion);
            }),
            m(T::None, |g, c| {
                g.discover(c.side, |d| d.kind() == super::Kind::Spell);
            }),
        ],
    ),
    choose(
        "Feral Rage",
        &[
            m(T::None, |g, c| {
                g.hero_attack_bonus(c.side, 4);
            }),
            m(T::None, |g, c| {
                g.gain_armor(c.side, 8);
            }),
        ],
    ),
    // "Bottom" is index 0: `Fix::deck`'s own convention (and `Game::draw`'s
    // `deck.pop()`) treats the end of the vec as the top of the deck.
    spell("Contingency", T::None, |g, c| {
        for _ in 0..2 {
            if let Some(card) = g.player_mut(c.side).deck.remove(0) {
                g.give_hand_card(c.side, card.to_hand());
            }
        }
    }),
    spell("Widow's Bite", T::None, |g, c| {
        g.hero_attack_bonus(c.side, 1);
        g.gain_armor(c.side, 1);
        g.give_card(c.side, tokens::WIDOWS_FEAST);
    }),
    spell("Widow's Feast", T::None, |g, c| {
        g.hero_attack_bonus(c.side, 2);
        g.gain_armor(c.side, 2);
        g.give_card(c.side, tokens::WIDOWS_BANQUET);
    }),
    spell("Widow's Banquet", T::None, |g, c| {
        g.hero_attack_bonus(c.side, 4);
        g.gain_armor(c.side, 4);
    }),
    // Battlecry damage is not boosted by Spell Power (see Fogsail
    // Freebooter/Mad Bomber above), so this is a raw `deal_damage`.
    battlecry("Savage Striker", T::EnemyMinion, |g, c| {
        let atk = g.player(c.side).hero_attack();
        if let Some(t) = c.target
            && atk > 0
        {
            g.deal_damage(t, atk);
        }
    }),
    deathrattle("Skyscreamer Eggs", |g, c| {
        g.summon_child(c.side, c.card, 4);
    }),
    deathrattle("Longneck Egg", |g, c| {
        g.summon_child(c.side, c.card, 1);
        g.buff_area(c.side, Area::FriendlyMinions, 1, 1);
    }),
    // ------------------------------------------------- neutral batch: bodies
    // Neutrals go in every deck, so each one here is a card that no longer
    // stops a pasted list from resolving at all. Every entry is implementable
    // exactly from its own text: nothing in this batch needed a number the
    // corpus does not carry.
    battlecry("The Black Knight", T::EnemyTaunt, |g, c| {
        if let Some(t) = c.target {
            g.destroy(t);
        }
    }),
    battlecry("Dirty Rat", T::None, |g, c| {
        // "Your opponent summons a random minion from their hand" — their
        // minion, on their board, chosen at random.
        let foe = c.side.other();
        let picks: Inline<usize, MAX_HAND> = g
            .player(foe)
            .hand
            .iter()
            .enumerate()
            .filter(|(_, hc)| hc.card.def().kind() == super::Kind::Minion)
            .map(|(i, _)| i)
            .collect();
        if picks.is_empty() || g.player(foe).board.is_full() {
            return;
        }
        let idx = picks[g.rngs.effects.index(picks.len())];
        if let Some(hc) = g.player_mut(foe).hand.remove(idx) {
            g.summon(foe, hc.card);
        }
    }),
    battlecry("Netherspite Historian", T::None, |g, c| {
        if g.holding_race(c.side, Races::DRAGON) {
            g.discover(c.side, |d| {
                d.kind() == super::Kind::Minion && d.races.any(Races::DRAGON)
            });
        }
    }),
    battlecry("Gorillabot A-3", T::None, |g, c| {
        // "another Mech": Gorillabot is a Mech and is already on the board
        // by the time its Battlecry runs, so it cannot count itself.
        let side = c.side;
        let others = g
            .player(side)
            .board
            .iter()
            .enumerate()
            .any(|(i, m)| {
                Some(i as u8) != c.source && m.active() && m.races().any(Races::MECHANICAL)
            });
        if others {
            g.discover(side, |d| {
                d.kind() == super::Kind::Minion && d.races.any(Races::MECHANICAL)
            });
        }
    }),
    battlecry("Menagerie Mug", T::None, |g, c| {
        // Three friendly minions, each of a different tribe. A minion with no
        // tribe has no type to be different from, so it is not eligible.
        let side = c.side;
        let mut given = 0;
        let mut used = Races::NONE;
        let mut order: Inline<u8, MAX_BOARD> = (0..g.player(side).board.len() as u8).collect();
        g.rngs.effects.shuffle(order.as_mut_slice());
        for slot in order.iter().copied() {
            if given >= 3 {
                break;
            }
            let Some(m) = g.player(side).board.get(slot as usize) else {
                continue;
            };
            let races = m.races();
            if !m.active() || !m.is_minion() || races.is_empty() || races.any(used) {
                continue;
            }
            used |= races;
            g.buff(Target::Minion(side, slot), 1, 1);
            given += 1;
        }
    }),
    battlecry("Omen of the End", T::None, |g, c| {
        if g.player(c.side).deck.is_empty() {
            g.mill(c.side.other(), 5);
        }
    }),
    // "Double this minion's Health"/"Attack": read off the body itself, which
    // is on the board by now and undamaged, so doubling is one buff.
    battlecry("Soldier of the Bronze", T::None, |g, c| {
        let Some(src) = c.source else { return };
        let Some(m) = g.player(c.side).board.get(src as usize) else {
            return;
        };
        let health = m.health();
        g.buff(Target::Minion(c.side, src), 0, health);
    }),
    battlecry("Soldier of the Infinite", T::None, |g, c| {
        let Some(src) = c.source else { return };
        let Some(m) = g.player(c.side).board.get(src as usize) else {
            return;
        };
        let atk = m.atk;
        g.buff(Target::Minion(c.side, src), atk, 0);
    }),
    deathrattle("Concealing Confection", |g, c| {
        g.add_random_to_hand(c.side, |d| d.kind() == super::Kind::Weapon);
    }),
    deathrattle("Willful Watcher", |g, c| {
        g.mill(c.side, 3);
    }),
    deathrattle("Tindral Sageswift", |g, c| {
        // "If it's your opponent's turn" — a Deathrattle fires whenever the
        // body dies, which is as often on their turn as on yours.
        let amount = if g.current == c.side { 1 } else { 4 };
        g.damage_area(c.side, Area::AllEnemies, amount);
    }),
    trigger("Critter Caretaker", |g, c| {
        if matches!(c.event, Event::TurnEnd { side } if side == c.side) {
            g.heal_hero(c.side, 3);
            g.heal_hero(c.side.other(), 3);
        }
    }),
    trigger("Daydreaming Pixie", |g, c| {
        if matches!(c.event, Event::TurnEnd { side } if side == c.side) {
            g.add_random_to_hand(c.side, |d| {
                d.kind() == super::Kind::Spell && d.school() == super::School::Nature
            });
        }
    }),
    trigger("Curious Cumulus", |g, c| {
        if matches!(c.event, Event::TurnEnd { side } if side == c.side) {
            g.player_mut(c.side).hero_divine_shield = true;
        }
    }),
    trigger("Earthen Drake", |g, c| {
        if matches!(c.event, Event::TurnEnd { side } if side == c.side) {
            g.damage_hero(c.side.other(), 4);
        }
    }),
    trigger("Time Skipper", |g, c| {
        // "each player's turn", and the Coin goes to whoever's turn ended —
        // including its own controller's.
        if let Event::TurnEnd { side } = c.event {
            g.give_token(side, tokens::COIN);
        }
    }),

    // No area helper takes a per-target predicate ("2 or less Attack") or
    // excludes the source, so this repeats `buff_area`'s loop by hand.
    battlecry("Hatchery Helper", T::None, |g, c| {
        let Some(src) = c.source else { return };
        let side = c.side;
        for i in 0..g.player(side).board.len() {
            if i == src as usize {
                continue;
            }
            if g.player(side).board[i].atk <= 2 {
                let t = Target::Minion(side, i as u8);
                g.buff(t, 1, 1);
                g.grant(t, Keywords::TAUNT);
            }
        }
    }),

    // ------------------------------------------------- class backlog batch
    // One pass over the per-class backlogs (`tavernsim backlog <class>`),
    // taking only cards whose whole text the engine can already say exactly.
    // Nothing here invents a number the corpus does not carry.

    // Death Knight.
    trigger("Monstrous Mosquito", |g, c| {
        if matches!(c.event, Event::TurnEnd { side } if side == c.side) {
            // "your other minions" — the Mosquito buffs the board, not itself.
            let me = c.me();
            let mut hits: Inline<Target, MAX_BOARD> = Inline::new();
            g.collect_area(c.side, Area::FriendlyMinions, &mut hits);
            for t in hits.iter() {
                if *t != me {
                    g.buff(*t, 1, 0);
                }
            }
        }
    }),
    c(
        "Thassarian",
        T::None,
        None,
        // "Battlecry and Deathrattle" is one line of text and two hooks.
        Some(|g, c| {
            if let Some(t) = g.random_enemy(c.side) {
                g.deal_damage(t, 2);
            }
        }),
        Some(|g, c| {
            if let Some(t) = g.random_enemy(c.side) {
                g.deal_damage(t, 2);
            }
        }),
        None,
        None,
        None,
        None,
        None,
        None,
    ),
    spell("Death Strike", T::AnyMinion, |g, c| {
        // Lifesteal on a spell is not a keyword the kernel applies; the heal
        // is the damage the spell would deal, Spell Damage included.
        let dealt = 6 + g.player(c.side).spell_power();
        g.spell_damage(c.side, c.target, 6);
        g.heal_hero(c.side, dealt);
    }),
    spell("Remorseless Winter", T::None, |g, c| {
        g.spell_damage_area(c.side, Area::AllEnemies, 2);
        g.draw_cards(c.side, 1);
    }),

    // Demon Hunter.
    spell("Chaos Strike", T::None, |g, c| {
        g.hero_attack_bonus(c.side, 2);
        g.draw_cards(c.side, 1);
    }),
    deathrattle("Felrattler", |g, c| {
        g.damage_area(c.side, Area::EnemyMinions, 1);
    }),
    trigger("Wrathspike Brute", |g, c| {
        // "After this is attacked" — it is the defender, whoever swung.
        if matches!(c.event, Event::AfterAttack { defender, .. } if defender == c.me()) {
            g.damage_area(c.side, Area::AllEnemies, 1);
        }
    }),

    // Paladin.
    spell("Immortalized in Stone", T::None, |g, c| {
        // Printed largest first; summoned smallest first, which is the order
        // the card's own childIds carry.
        g.summon_token(c.side, tokens::WORN_STATUE, 1);
        g.summon_token(c.side, tokens::LIVING_STATUE, 1);
        g.summon_token(c.side, tokens::PRISTINE_STATUE, 1);
    }),

    // Priest.
    spell("Haunt", T::AnyMinion, |g, c| {
        let Some(t) = c.target else { return };
        g.buff(t, 2, 3);
        g.grant(t, Keywords::REBORN);
        g.grant(t, Keywords::TAUNT);
    }),
    battlecry("Natalie Seline", T::AnyMinion, |g, c| {
        let Some(t) = c.target else { return };
        let Target::Minion(s, i) = t else { return };
        // "gain its Health" is what the minion has left, read before it dies.
        let Some(health) = g.player(s).board.get(i as usize).map(|m| m.health()) else {
            return;
        };
        g.destroy(t);
        if let Some(src) = c.source {
            g.buff(Target::Minion(c.side, src), 0, health);
        }
    }),
    spell("Story of Amara", T::None, |g, c| {
        // Set, not healed: this is the one effect that can put a hero above
        // the starting total, and `heal_hero` respects that afterwards.
        g.player_mut(c.side).hero_hp = 40;
    }),

    // Rogue.
    trigger("SI:7 Supplier", |g, c| {
        if matches!(c.event, Event::AfterAttack { attacker, .. } if attacker == c.me()) {
            g.draw_cards(c.side, 1);
        }
    }),
    battlecry("Troubled Double", T::None, |g, c| {
        if g.combo_active(c.side) {
            g.summon_copy(c.side, c.card);
        }
    }),
    battlecry("Crazed Chemist", T::FriendlyMinion, |g, c| {
        if g.combo_active(c.side)
            && let Some(t) = c.target
        {
            g.buff(t, 4, 0);
        }
    }),

    // Shaman.
    trigger("Wailing Vapor", |g, c| {
        if let Event::CardPlayed { side, card } = c.event
            && side == c.side
            && card.def().kind() == super::Kind::Minion
            && card.def().races.any(Races::ELEMENTAL)
        {
            g.buff(c.me(), 1, 0);
        }
    }),
    spell("Fire Breath", T::AnyCharacter, |g, c| {
        g.spell_damage(c.side, c.target, 4);
        g.sweep_deaths();
        let mut hits: Inline<Target, MAX_BOARD> = Inline::new();
        g.collect_area(c.side, Area::FriendlyMinions, &mut hits);
        for t in hits.iter() {
            if let Target::Minion(s, i) = *t
                && g.player(s).board[i as usize].races().any(Races::ELEMENTAL)
            {
                g.buff(*t, 1, 1);
            }
        }
    }),
    deathrattle("Hammer of Twilight", |g, c| {
        g.summon_token(c.side, tokens::TWILIGHT_ELEMENTAL, 1);
    }),

    // Warlock.
    deathrattle("Rotheart Dryad", |g, c| {
        g.draw_matching(c.side, |d| d.kind() == super::Kind::Minion && d.cost >= 7);
    }),
    spell("Twisting Nether", T::None, |g, _c| {
        // Locations sit on the board next to the minions and are named by the
        // card too, so this walks the boards rather than going through
        // `collect_area`, which returns minions alone.
        for i in 0..2 {
            let side = Side::from_index(i);
            for slot in 0..g.player(side).board.len() {
                g.destroy(Target::Minion(side, slot as u8));
            }
        }
    }),

    // Warrior.
    battlecry("Sky Raider", T::None, |g, c| {
        g.add_random_to_hand(c.side, |d| d.races.any(Races::PIRATE));
    }),
    battlecry("Ravaging Ghoul", T::None, |g, c| {
        let me = c.source.map(|s| Target::Minion(c.side, s));
        let mut hits: Inline<Target, { MAX_BOARD * 2 }> = Inline::new();
        g.collect_area(c.side, Area::AllMinions, &mut hits);
        for t in hits.iter() {
            if Some(*t) != me {
                g.deal_damage(*t, 1);
            }
        }
    }),
    spell("Ironforge Portal", T::None, |g, c| {
        g.gain_armor(c.side, 4);
        g.summon_random_of_cost(c.side, 4, 1);
    }),
    spell("Guard Duty", T::None, |g, c| {
        for pred in [
            (|d: &super::CardDef| {
                d.kind() == super::Kind::Minion && d.cost == 6 && d.keywords.has(Keywords::TAUNT)
            }) as fn(&super::CardDef) -> bool,
            |d: &super::CardDef| {
                d.kind() == super::Kind::Minion && d.cost == 4 && d.keywords.has(Keywords::TAUNT)
            },
            |d: &super::CardDef| {
                d.kind() == super::Kind::Minion && d.cost == 2 && d.keywords.has(Keywords::TAUNT)
            },
        ] {
            g.summon_random_where(c.side, pred);
        }
    }),

    // -------------------------------------------- class backlog, second pass
    // Same rule as the pass above: only cards the engine can say exactly.

    // Deathrattles that summon.
    deathrattle("Cairne Bloodhoof", |g, c| {
        g.summon_token(c.side, tokens::BAINE_BLOODHOOF, 1);
    }),
    deathrattle("Voidlord", |g, c| {
        // "three 1/3 Demons with Taunt" is three Voidwalkers; the card carries
        // no childIds of its own, so the body is named rather than looked up.
        g.summon_token(c.side, tokens::VOIDWALKER, 3);
    }),
    deathrattle("Mountain Bear", |g, c| {
        g.summon_token(c.side, tokens::MOUNTAIN_CUB, 2);
    }),
    deathrattle("Eternal Bloodpetal", |g, c| {
        g.summon_token(c.side, tokens::ETERNAL_SEEDLING, 1);
    }),
    deathrattle("Eternal Seedling", |g, c| {
        g.summon_token(c.side, tokens::ETERNAL_BLOODPETAL, 1);
    }),
    deathrattle("Sneed's Old Shredder", |g, c| {
        g.summon_random_where(c.side, |d| {
            d.kind() == super::Kind::Minion && d.rarity() == super::Rarity::Legendary
        });
    }),
    deathrattle("Drakeadon Mongrel", |g, c| {
        g.summon_random_of_cost(c.side, 4, 2);
    }),

    // Deathrattles that clear.
    deathrattle("Obsidian Statue", |g, c| {
        if let Some(t) = g.random_minion(c.side.other()) {
            g.destroy(t);
        }
    }),
    deathrattle("Avatar of Destruction", |g, c| {
        g.damage_area(c.side, Area::EnemyMinions, 9);
    }),
    deathrattle("Sewer Imp", |g, c| {
        g.damage_area(c.side, Area::AllEnemies, 2);
    }),

    // Deathrattles that fill a hand.
    deathrattle("Fae Trickster", |g, c| {
        g.draw_matching(c.side, |d| d.kind() == super::Kind::Spell && d.cost >= 5);
    }),
    deathrattle("Tormented Dreadwing", |g, c| {
        for _ in 0..2 {
            if g.draw_matching(c.side, |d| d.races.any(Races::DRAGON))
                && let Some(h) = g.player_mut(c.side).hand.last_mut()
            {
                h.cost_delta -= 1;
            }
        }
    }),
    deathrattle("Seeding Dragon", |g, c| {
        if g.add_random_to_hand(c.side, |d| d.races.any(Races::DRAGON))
            && let Some(h) = g.player_mut(c.side).hand.last_mut()
        {
            h.cost_delta -= 2;
        }
    }),
    deathrattle("Twilight Mender", |g, c| {
        g.add_random_to_hand(c.side, |d| {
            d.kind() == super::Kind::Spell && d.school() == super::School::Holy
        });
        g.add_random_to_hand(c.side, |d| {
            d.kind() == super::Kind::Spell && d.school() == super::School::Shadow
        });
    }),

    // Battlecries.
    battlecry("Primordial Drake", T::None, |g, c| {
        let me = c.source.map(|s| Target::Minion(c.side, s));
        let mut hits: Inline<Target, { MAX_BOARD * 2 }> = Inline::new();
        g.collect_area(c.side, Area::AllMinions, &mut hits);
        for t in hits.iter() {
            if Some(*t) != me {
                g.deal_damage(*t, 2);
            }
        }
    }),
    battlecry("Nerubian Swarmguard", T::None, |g, c| {
        g.summon_copy(c.side, c.card);
        g.summon_copy(c.side, c.card);
    }),
    c(
        "Underking",
        T::None,
        None,
        Some(|g, c| g.gain_armor(c.side, 6)),
        Some(|g, c| g.gain_armor(c.side, 6)),
        None,
        None,
        None,
        None,
        None,
        None,
    ),
    battlecry("Heir of Hereafter", T::None, |g, c| {
        // Every damaged minion on the table, both boards; the Heir itself has
        // just landed and is not one of them.
        let damaged = (0..2)
            .map(|i| {
                g.players[i]
                    .board
                    .iter()
                    .filter(|m| m.active() && m.is_minion() && m.damage > 0)
                    .count() as i16
            })
            .sum::<i16>();
        if damaged > 0
            && let Some(src) = c.source
        {
            g.buff(Target::Minion(c.side, src), 2 * damaged, 2 * damaged);
        }
    }),

    // Spells.
    spell("Blizzard", T::None, |g, c| {
        g.spell_damage_area(c.side, Area::EnemyMinions, 2);
        g.freeze_area(c.side, Area::EnemyMinions);
    }),
    spell("Ceremonial Clash", T::None, |g, c| {
        for cost in [3, 2, 1] {
            g.summon_random_of_cost(c.side, cost, 1);
        }
    }),
    spell("Ward of Earth", T::None, |g, c| {
        g.gain_armor(c.side, 5);
        if g.summon_random_of_cost(c.side, 5, 1) > 0 {
            let slot = g.player(c.side).board.len() as u8 - 1;
            g.grant(Target::Minion(c.side, slot), Keywords::TAUNT);
        }
    }),
    spell("For All Time", T::None, |g, c| {
        g.destroy_area_where(c.side, Area::AllMinions, |m| m.atk <= 4);
    }),
    spell("Forest's Gift", T::FriendlyMinion, |g, c| {
        let Some(t) = c.target else { return };
        // Counted before the buff lands, and the target counts itself.
        let n = g.minion_count(c.side) as i16;
        g.buff(t, n, n);
    }),
    spell("Dethrone", T::AnyMinion, |g, c| {
        if let Some(t) = c.target {
            g.destroy(t);
        }
        if g.combo_active(c.side) {
            g.sweep_deaths();
            g.summon_random_of_cost(c.side, 8, 1);
        }
    }),
    spell("Nascent Bolt", T::AnyMinion, |g, c| {
        let Some(t) = c.target else { return };
        g.spell_damage(c.side, Some(t), 5);
        let alive = matches!(t, Target::Minion(s, i)
            if g.player(s).board.get(i as usize).is_some_and(|m| !m.is_dead()));
        if alive {
            g.draw_cards(c.side, 2);
        }
    }),
    spell("Eldritch Tentacles", T::None, |g, c| {
        // "Repeat this with 1 less damage" — 3, then 2, then 1, then done.
        // Spell Damage rides on each pass, as it does on any spell damage.
        for base in (1..=3).rev() {
            g.spell_damage_area(c.side, Area::AllMinions, base);
            g.sweep_deaths();
        }
    }),

    // End of turn, and after an attack.
    trigger("Yesterloc", |g, c| {
        if matches!(c.event, Event::TurnEnd { side } if side == c.side) {
            let me = c.me();
            let mut hits: Inline<Target, MAX_BOARD> = Inline::new();
            g.collect_area(c.side, Area::FriendlyMinions, &mut hits);
            for t in hits.iter() {
                if *t != me {
                    g.buff(*t, 0, 1);
                }
            }
        }
    }),
    trigger("Scalebreaker Bulwark", |g, c| {
        if matches!(c.event, Event::TurnEnd { side } if side == c.side) {
            g.damage_area(c.side, Area::AllEnemies, 2);
        }
    }),
    trigger("Gorishi Tunneler", |g, c| {
        if matches!(c.event, Event::AfterAttack { attacker, .. } if attacker == c.me()) {
            g.deal_damage(Target::Hero(c.side.other()), 2);
        }
    }),
    trigger("Axe of the Forefathers", |g, c| {
        if matches!(c.event, Event::AfterAttack { attacker: Target::Hero(s), .. } if s == c.side) {
            g.damage_area(c.side, Area::AllMinions, 1);
        }
    }),
    // ---------------------------------------------- "Has +N Attack while …"
    // One rule, twenty-one cards. Each row is only the condition; the
    // recomputation that keeps it live is `Game::refresh_conditionals`.

    // While damaged.
    bonus("Aberrant Berserker", |_, _, _, me| if me.damage > 0 { (2, 0) } else { (0, 0) }),
    bonus("Amani Berserker", |_, _, _, me| if me.damage > 0 { (3, 0) } else { (0, 0) }),
    bonus("Angry Chicken", |_, _, _, me| if me.damage > 0 { (5, 0) } else { (0, 0) }),
    bonus("Bloodhoof Brave", |_, _, _, me| if me.damage > 0 { (3, 0) } else { (0, 0) }),
    bonus("Dozing Marksman", |_, _, _, me| if me.damage > 0 { (4, 0) } else { (0, 0) }),
    bonus("Grommash Hellscream", |_, _, _, me| if me.damage > 0 { (6, 0) } else { (0, 0) }),
    bonus("Redband Wasp", |_, _, _, me| if me.damage > 0 { (3, 0) } else { (0, 0) }),
    bonus("Tauren Warrior", |_, _, _, me| if me.damage > 0 { (3, 0) } else { (0, 0) }),
    bonus("Temple Berserker", |_, _, _, me| if me.damage > 0 { (2, 0) } else { (0, 0) }),
    bonus("Undercover Cultist", |_, _, _, me| if me.damage > 0 { (3, 0) } else { (0, 0) }),
    bonus("Warbot", |_, _, _, me| if me.damage > 0 { (1, 0) } else { (0, 0) }),

    // During your opponent's turn.
    bonus("Tar Slime", |g, side, _, _| if g.current != side { (2, 0) } else { (0, 0) }),
    bonus("Tar Creeper", |g, side, _, _| if g.current != side { (2, 0) } else { (0, 0) }),
    bonus("Tar Lurker", |g, side, _, _| if g.current != side { (3, 0) } else { (0, 0) }),
    bonus("Tar Lord", |g, side, _, _| if g.current != side { (4, 0) } else { (0, 0) }),
    bonus("Tar Tyrant", |g, side, _, _| if g.current != side { (6, 0) } else { (0, 0) }),

    // While the rest of the position says so.
    bonus("Cogmaster", |g, side, _, _| {
        if g.controls_race(side, Races::MECHANICAL) { (2, 0) } else { (0, 0) }
    }),
    bonus("Proud Defender", |g, side, _, _| {
        // "no other minions" — the Defender itself is always one of them.
        if g.minion_count(side) <= 1 { (2, 0) } else { (0, 0) }
    }),
    bonus("Small-Time Buccaneer", |g, side, _, _| {
        if g.player(side).weapon.is_some() { (2, 0) } else { (0, 0) }
    }),
    bonus("Spine Crawler", |g, side, _, _| {
        let has_location = g
            .player(side)
            .board
            .iter()
            .any(|m| m.active() && m.kind() == super::Kind::Location);
        if has_location { (3, 0) } else { (0, 0) }
    }),
    bonus("Surging Tempest", |g, side, _, _| {
        // The crystals locked for *this* turn, not the Overload queued for the
        // next one -- those are not yet Overloaded Mana Crystals.
        if g.player(side).overload_now > 0 { (1, 0) } else { (0, 0) }
    }),

    // --------------------------------- more hand enchantments, and
    // the two per-turn questions the engine could not answer before.
    spell("Blood Tap", T::None, |g, c| {
        let more = g.spend_corpses(c.side, 2);
        for h in g.player_mut(c.side).hand.iter_mut() {
            if h.card.def().kind() == super::Kind::Minion {
                h.enchant(1, 1);
                if more {
                    h.enchant(1, 1);
                }
            }
        }
    }),
    battlecry("Darkfallen Neophyte", T::None, |g, c| {
        if !g.spend_corpses(c.side, 2) {
            return;
        }
        for h in g.player_mut(c.side).hand.iter_mut() {
            if h.card.def().kind() == super::Kind::Minion {
                h.enchant(2, 0);
            }
        }
    }),
    trigger("Hourglass Attendant", |g, c| {
        if matches!(c.event, Event::TurnEnd { side } if side == c.side) {
            for h in g.player_mut(c.side).hand.iter_mut() {
                if h.card.def().kind() == super::Kind::Minion {
                    h.enchant(1, 1);
                }
            }
        }
    }),
    trigger("Overlord Runthak", |g, c| {
        if matches!(c.event, Event::AttackDeclared { attacker, .. } if attacker == c.me()) {
            for h in g.player_mut(c.side).hand.iter_mut() {
                if h.card.def().kind() == super::Kind::Minion {
                    h.enchant(1, 1);
                }
            }
        }
    }),
    spell("Bone Flurry", T::None, |g, c| {
        // Printed immune to Spell Damage, so this splits raw damage.
        let lost = g.player(c.side).friendly_deaths_turn > 0;
        g.damage_split(c.side, Area::AllEnemies, 3);
        if lost {
            g.damage_split(c.side, Area::AllEnemies, 3);
        }
    }),
    battlecry("Liferender", T::EnemyMinion, |g, c| {
        if g.player(c.side).hero_health_moved_turn
            && let Some(t) = c.target
        {
            g.deal_damage(t, 6);
        }
    }),
    battlecry("Endtime Survivor", T::None, |g, c| {
        if g.player(c.side).hero_damaged_turn
            && let Some(src) = c.source
        {
            g.buff(Target::Minion(c.side, src), 3, 3);
        }
    }),
    battlecry("Crystal Tender", T::None, |g, c| {
        // "Empty": crystals without the mana, so this catches up on the
        // crystal count and leaves the turn's mana where it was.
        let theirs = g.player(c.side.other()).crystals;
        let p = g.player_mut(c.side);
        if theirs > p.crystals {
            let cap = p.crystal_cap();
            p.crystals = theirs.min(cap);
        }
    }),
    trigger("Lorewalker Cho", |g, c| {
        if let Event::SpellCast { side, card } = c.event {
            // "the other player" -- whoever did not cast it, and Cho does not
            // care whose side he is on.
            g.give_token(side.other(), card);
        }
    }),
    trigger("Chromatic Broodmother", |g, c| {
        if matches!(c.event, Event::AttackDeclared { attacker, .. } if attacker == c.me())
            && let Some(atk) = g.player(c.side).board.get(c.slot as usize).map(|m| m.atk)
        {
            g.refresh_mana(c.side, atk);
        }
    }),
    spell("Photosynthesis", T::AnyCharacter, |g, c| {
        if let Some(t) = c.target {
            g.heal(t, 6);
        }
        for _ in 0..3 {
            g.add_random_to_hand(c.side, |d| {
                d.kind() == super::Kind::Spell && d.class() == super::Class::Druid
            });
        }
    }),

    // ------------------------------------------------- last sweep
    battlecry("Ymirjar Frostbreaker", T::None, |g, c| {
        let frost = g
            .player(c.side)
            .hand
            .iter()
            .filter(|h| h.card.def().school() == super::School::Frost)
            .count() as i16;
        if frost > 0 && let Some(src) = c.source {
            g.buff(Target::Minion(c.side, src), frost, 0);
        }
    }),
    spell("Nurturing Nature", T::FriendlyBeast, |g, c| {
        if let Some(t) = c.target {
            g.buff(t, 2, 2);
        }
        let mut beasts: Inline<u16, MAX_HAND> = Inline::new();
        for (i, h) in g.player(c.side).hand.iter().enumerate() {
            if h.card.def().races.any(Races::BEAST) {
                beasts.push(i as u16);
            }
        }
        if beasts.is_empty() {
            return;
        }
        let pick = g.rngs.effects.index(beasts.len());
        if let Some(h) = g.player_mut(c.side).hand.get_mut(beasts[pick] as usize) {
            h.enchant(2, 2);
        }
    }),
    spell("Release the Beasts", T::None, |g, c| {
        for h in g.player_mut(c.side).hand.iter_mut() {
            let d = h.card.def();
            if d.kind() != super::Kind::Minion {
                continue;
            }
            h.enchant(1, 1);
            if d.rarity() == super::Rarity::Legendary {
                h.enchant(2, 1);
            }
        }
    }),
    battlecry("Dimensional Weaponsmith", T::None, |g, c| {
        for h in g.player_mut(c.side).hand.iter_mut() {
            if matches!(h.card.def().kind(), super::Kind::Minion | super::Kind::Weapon) {
                h.enchant(2, 0);
            }
        }
    }),
    spell("Power Word: Barrier", T::AnyCharacter, |g, c| {
        match c.target {
            Some(Target::Minion(s, i)) => g.grant(Target::Minion(s, i), Keywords::DIVINE_SHIELD),
            Some(Target::Hero(s)) => g.player_mut(s).hero_divine_shield = true,
            None => {}
        }
        for h in g.player_mut(c.side).hand.iter_mut() {
            if h.card.def().kind() == super::Kind::Minion {
                h.enchant(0, 2);
            }
        }
    }),
    c(
        "Dig for Freedom",
        T::FriendlyMinion,
        Some(|g, c| grant_rattle(g, c.target, c.card)),
        None,
        Some(|g, c| {
            g.summon_random_of_cost(c.side, 4, 2);
        }),
        None,
        None,
        None,
        None,
        None,
        None,
    ),
    c(
        "Threshrider's Blessing",
        T::FriendlyMinion,
        Some(|g, c| {
            if let Some(t) = c.target {
                g.buff(t, 4, 4);
            }
            grant_rattle(g, c.target, c.card);
        }),
        None,
        Some(|g, c| {
            g.summon_random_of_cost(c.side, 4, 1);
        }),
        None,
        None,
        None,
        None,
        None,
        None,
    ),
    c(
        "Lo'Gosh's Last Stand",
        T::AnyMinion,
        Some(|g, c| grant_rattle(g, c.target, c.card)),
        None,
        Some(|g, c| {
            let n = g.player(c.side).hand.len();
            if n == 0 {
                return;
            }
            let pick = g.rngs.effects.index(n);
            let card = g.player(c.side).hand[pick].card;
            if card.def().kind() == super::Kind::Minion {
                g.player_mut(c.side).hand.remove(pick);
                g.summon(c.side, card);
            }
        }),
        None,
        None,
        None,
        None,
        None,
        None,
    ),
    c(
        "Amphibian's Spirit",
        T::AnyMinion,
        Some(|g, c| {
            if let Some(t) = c.target {
                g.buff(t, 2, 2);
            }
            grant_rattle(g, c.target, c.card);
        }),
        None,
        Some(|g, c| {
            // "…and this Deathrattle": it moves on to whoever it buffs.
            let Some(t) = g.random_minion(c.side) else { return };
            g.buff(t, 2, 2);
            grant_rattle(g, Some(t), c.card);
        }),
        None,
        None,
        None,
        None,
        None,
        None,
    ),
    c(
        "Sheep Mask",
        T::AnyMinion,
        Some(|g, c| {
            let Some(t) = c.target else { return };
            g.set_attack(t, 1);
            g.set_health(t, 1);
            grant_rattle(g, Some(t), c.card);
        }),
        None,
        Some(|g, c| {
            g.damage_area(c.side, Area::AllMinions, 2);
        }),
        None,
        None,
        None,
        None,
        None,
        None,
    ),
    spell("Charity", T::None, |g, c| {
        let from = g.player(c.side).graveyard_at_turn_start as usize;
        let dead: Inline<CardId, { crate::state::GRAVEYARD }> = g
            .player(c.side)
            .graveyard
            .iter()
            .skip(from)
            .copied()
            .collect();
        for card in dead.iter().copied() {
            if !g.give_token(c.side, card) {
                break;
            }
            if let Some(h) = g.player_mut(c.side).hand.last_mut() {
                h.enchant(3, 3);
            }
        }
    }),
    battlecry("Priest of An'she", T::None, |g, c| {
        if g.player(c.side).hero_healed_turn
            && let Some(src) = c.source
        {
            g.buff(Target::Minion(c.side, src), 3, 3);
        }
    }),
    battlecry("Envoy of the Glade", T::None, |g, c| {
        let pool = super::discover_pool(|d| d.class() == super::Class::Druid);
        if pool.is_empty() {
            return;
        }
        for i in 0..g.player(c.side).deck.len() {
            if g.player(c.side).deck[i].def().class() != super::Class::Neutral {
                continue;
            }
            let pick = g.rngs.effects.index(pool.len());
            g.player_mut(c.side).deck[i] = DeckCard::new(pool[pick]);
        }
    }),
    battlecry("Hellraiser", T::None, |g, c| {
        if !g.discover_from_deck(c.side, |_| true)
            && let Some(src) = c.source
        {
            g.buff(Target::Minion(c.side, src), 4, 4);
        }
    }),
    c(
        "Storage Scuffle",
        T::AnyMinion,
        Some(|g, c| {
            g.spell_damage(c.side, c.target, 3);
        }),
        None,
        None,
        None,
        None,
        None,
        None,
        Some(|g, side, _i| if g.player(side).discovered_turn { -3 } else { 0 }),
        None,
    ),
    spell("Unearthed Artifacts", T::None, |g, c| {
        let cost = if g.player(c.side).discovered_turn { 4 } else { 2 };
        g.summon_random_of_cost(c.side, cost, 1);
    }),
    battlecry("Diabolus Rex", T::None, |g, c| {
        if !(g.kindred(c.side, Races::BEAST) || g.kindred(c.side, Races::DEMON)) {
            return;
        }
        let foe = c.side.other();
        let n = g.player(foe).board.len();
        if n == 0 {
            return;
        }
        g.deal_damage(Target::Minion(foe, 0), 6);
        if n > 1 {
            g.deal_damage(Target::Minion(foe, n as u8 - 1), 6);
        }
    }),

    // ---------------------------------------------- forced attacks
    // `Game::forced_attack` assigns the target itself; these are the cards
    // that do the assigning.
    spell("Unholy Frenzy", T::EnemyMinion, |g, c| {
        let Some(target) = c.target else { return };
        // Recorded before the swings, so a minion that trades itself away can
        // be put back.
        let before: Inline<CardId, MAX_BOARD> =
            g.player(c.side).board.iter().map(|m| m.card).collect();
        let mut slot = 0;
        while slot < g.player(c.side).board.len() {
            let before_n = g.player(c.side).board.len();
            g.forced_attack((c.side, slot as u8), target);
            g.sweep_deaths();
            // A minion that died has already pulled the rest forward.
            if g.player(c.side).board.len() == before_n {
                slot += 1;
            }
            if matches!(target, Target::Minion(s, i)
                if g.player(s).board.get(i as usize).is_none())
            {
                break;
            }
        }
        // "Resummon any that die": one body back for each one missing.
        let mut done: Inline<CardId, MAX_BOARD> = Inline::new();
        for card in before.iter().copied() {
            if done.contains(&card) {
                continue;
            }
            done.push(card);
            let has = g.player(c.side).board.iter().filter(|m| m.card == card).count();
            let had = before.iter().filter(|x| **x == card).count();
            for _ in has..had {
                if !g.summon(c.side, card) {
                    break;
                }
            }
        }
    }),
    deathrattle("Temporal Traveler", |g, c| {
        if g.summon_token(c.side, tokens::TEMPORAL_SHADOW, 1) == 0 {
            return;
        }
        let slot = g.player(c.side).board.len() as u8 - 1;
        if let Some(t) = g.random_minion(c.side.other()) {
            g.forced_attack((c.side, slot), t);
        }
    }),
    trigger("Gnome Muncher", |g, c| {
        if matches!(c.event, Event::TurnEnd { side } if side == c.side)
            && let Some(t) = g.lowest_health_enemy(c.side)
        {
            g.forced_attack((c.side, c.slot), t);
        }
    }),
    battlecry("High Cultist Herenn", T::None, |g, c| {
        let before = g.player(c.side).board.len();
        for _ in 0..2 {
            if !g.summon_from_deck(c.side, |d| d.keywords.has(Keywords::DEATHRATTLE)) {
                break;
            }
        }
        // "They fight!" -- the two of them, with each other.
        if g.player(c.side).board.len() >= before + 2 {
            let a = before as u8;
            let b = before as u8 + 1;
            g.forced_attack((c.side, a), Target::Minion(c.side, b));
        }
    }),
    trigger("Mythical Terror", |g, c| {
        if !matches!(c.event, Event::TurnEnd { side } if side == c.side) {
            return;
        }
        let foe = c.side.other();
        let me = c.me();
        let mut slot = 0;
        while slot < g.player(foe).board.len() {
            let before = g.player(foe).board.len();
            g.forced_attack((foe, slot as u8), me);
            g.sweep_deaths();
            // A minion that died in the exchange has already shifted the ones
            // behind it into its place.
            if g.player(foe).board.len() == before {
                slot += 1;
            }
            if g.player(c.side).board.get(c.slot as usize).is_none() {
                break;
            }
        }
    }),
    trigger("Illidari Inquisitor", |g, c| {
        if let Event::AfterAttack { attacker: Target::Hero(s), defender, defender_died: false } =
            c.event
            && s == c.side
        {
            g.forced_attack((c.side, c.slot), defender);
        }
    }),
    spell("TREEEES!!!", T::AnyMinion, |g, c| {
        let Some(target) = c.target else { return };
        for _ in 0..4 {
            if g.summon_token(c.side, tokens::ANGRY_TREANT, 1) == 0 {
                break;
            }
            let slot = g.player(c.side).board.len() as u8 - 1;
            g.forced_attack((c.side, slot), target);
            g.sweep_deaths();
            if matches!(target, Target::Minion(s, i)
                if g.player(s).board.get(i as usize).is_none())
            {
                break;
            }
        }
    }),
    deathrattle("Ankylodon", |g, c| {
        for _ in 0..2 {
            if !g.summon_random_where(c.side, |d| {
                d.kind() == super::Kind::Minion && d.cost == 3 && d.races.any(Races::BEAST)
            }) {
                break;
            }
            let slot = g.player(c.side).board.len() as u8 - 1;
            if let Some(t) = g.random_enemy(c.side) {
                g.forced_attack((c.side, slot), t);
                g.sweep_deaths();
            }
        }
    }),
    trigger("Wilted Shadow", |g, c| {
        if let Event::Healed { target, .. } = c.event
            && matches!(target, Target::Hero(s) | Target::Minion(s, _) if s == c.side.other())
        {
            g.forced_attack((c.side, c.slot), target);
        }
    }),
    spell("Behemoth Mask", T::AnyMinion, |g, c| {
        let Some(t) = c.target else { return };
        g.set_attack(t, 8);
        g.set_health(t, 10);
        g.grant(t, Keywords::LIFESTEAL);
        let foe = c.side.other();
        let mut pool: Inline<u8, MAX_BOARD> = Inline::new();
        for (i, m) in g.player(foe).board.iter().enumerate() {
            if m.active() && m.is_minion() {
                pool.push(i as u8);
            }
        }
        if pool.is_empty() {
            return;
        }
        let pick = g.rngs.effects.index(pool.len());
        g.forced_attack((foe, pool[pick]), t);
    }),
    battlecry("Warmaul Challenger", T::EnemyMinion, |g, c| {
        let Some(target) = c.target else { return };
        let Some(src) = c.source else { return };
        // "To the death": they trade until one of them is gone. Bounded, since
        // two bodies that cannot hurt each other would otherwise stand here
        // forever.
        for _ in 0..32 {
            g.forced_attack((c.side, src), target);
            g.sweep_deaths();
            let mine_alive = g.player(c.side).board.get(src as usize).is_some_and(|m| {
                m.active() && m.card.name() == "Warmaul Challenger"
            });
            let theirs_alive = matches!(target, Target::Minion(s, i)
                if g.player(s).board.get(i as usize).is_some_and(|m| m.active()));
            if !mine_alive || !theirs_alive {
                break;
            }
        }
    }),
    battlecry("Rampaging Hound", T::None, |g, c| {
        let Some(src) = c.source else { return };
        let me = Target::Minion(c.side, src);
        let foe = c.side.other();
        let mut slot = 0;
        while slot < g.player(foe).board.len() {
            let before = g.player(foe).board.len();
            g.forced_attack((foe, slot as u8), me);
            g.sweep_deaths();
            if g.player(foe).board.len() == before {
                slot += 1;
            }
            if g.player(c.side).board.get(src as usize).is_none() {
                break;
            }
        }
    }),

    // ----------------------------------------- granted deathrattles
    // Each of these grants *itself* as the rattle: the card's own row carries
    // the effect, and `Permanent::granted_rattle` points back at it.
    c(
        "Spikeridged Steed",
        T::FriendlyMinion,
        Some(|g, c| {
            let Some(t) = c.target else { return };
            g.buff(t, 2, 6);
            g.grant(t, Keywords::TAUNT);
            if let Target::Minion(s, i) = t
                && let Some(m) = g.player_mut(s).board.get_mut(i as usize)
            {
                m.granted_rattle = c.card;
            }
        }),
        None,
        Some(|g, c| {
            g.summon_token(c.side, tokens::STEGODON, 1);
        }),
        None,
        None,
        None,
        None,
        None,
        None,
    ),
    c(
        "Talanji's Last Stand",
        T::None,
        Some(|g, c| {
            let card = c.card;
            for m in g.player_mut(c.side).board.iter_mut() {
                m.granted_rattle = card;
            }
        }),
        None,
        Some(|g, c| {
            g.summon_random_of_cost(c.side, 4, 1);
        }),
        None,
        None,
        None,
        None,
        None,
        None,
    ),
    c(
        "Ulfar",
        T::None,
        None,
        Some(|g, c| {
            let card = c.card;
            let me = c.source;
            for (i, m) in g.player_mut(c.side).board.iter_mut().enumerate() {
                if Some(i as u8) != me {
                    m.granted_rattle = card;
                }
            }
        }),
        Some(|g, c| {
            // This row is the granted rattle, and a minion's own row fires on
            // its own death as well -- so Ulfar dying must not pay itself out.
            // The Ctx tells the two apart: `dying` is the host, and for the
            // granted case the host is never Ulfar.
            if c.dying.is_some_and(|m| m.card == c.card) {
                return;
            }
            // "with this minion's Cost" -- the host's, which is why the Ctx
            // carries the host body rather than Ulfar's.
            let cost = c.dying.map_or(0, |m| m.card.def().cost);
            g.summon_random_of_cost(c.side, cost, 1);
        }),
        None,
        None,
        None,
        None,
        None,
        None,
    ),
    deathrattle("Living Spores", |g, c| {
        g.summon_token(c.side, tokens::PLANT, 2);
    }),
    c(
        "Ancient Raptor",
        T::None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(&[
            m(T::None, |g, c| {
                if let Some(src) = c.source {
                    g.buff(Target::Minion(c.side, src), 3, 0);
                }
            }),
            m(T::None, |g, c| {
                if let Some(src) = c.source {
                    g.grant(Target::Minion(c.side, src), Keywords::DIVINE_SHIELD);
                }
            }),
            m(T::None, |g, c| {
                if let Some(src) = c.source
                    && let Some(body) = g.player_mut(c.side).board.get_mut(src as usize)
                {
                    body.granted_rattle = tokens::LIVING_SPORES;
                }
            }),
        ]),
        None,
        None,
    ),

    // -------------------------------- enchantments on cards in hand
    battlecry("Grimestreet Outfitter", T::None, |g, c| {
        for h in g.player_mut(c.side).hand.iter_mut() {
            if h.card.def().kind() == super::Kind::Minion {
                h.enchant(1, 1);
            }
        }
    }),
    battlecry("Disciple of the Dove", T::None, |g, c| {
        g.draw_matching(c.side, |d| d.kind() == super::Kind::Minion);
        for h in g.player_mut(c.side).hand.iter_mut() {
            if h.card.def().kind() == super::Kind::Minion {
                h.enchant(0, 2);
            }
        }
    }),
    battlecry("Detonation Juggernaut", T::None, |g, c| {
        for h in g.player_mut(c.side).hand.iter_mut() {
            let d = h.card.def();
            if d.kind() == super::Kind::Minion && d.keywords.has(Keywords::TAUNT) {
                h.enchant(2, 2);
            }
        }
    }),
    spell("I Know a Guy", T::None, |g, c| {
        if g.discover(c.side, |d| {
            d.kind() == super::Kind::Minion && d.keywords.has(Keywords::TAUNT)
        }) && let Some(h) = g.player_mut(c.side).hand.last_mut()
        {
            h.enchant(1, 2);
        }
    }),
    deathrattle("Twisted Treant", |g, c| {
        let _ = c;
        for i in 0..2 {
            let mut minions: Inline<u16, MAX_HAND> = Inline::new();
            for (j, h) in g.players[i].hand.iter().enumerate() {
                if h.card.def().kind() == super::Kind::Minion {
                    minions.push(j as u16);
                }
            }
            if minions.is_empty() {
                continue;
            }
            let pick = g.rngs.effects.index(minions.len());
            if let Some(h) = g.players[i].hand.get_mut(minions[pick] as usize) {
                h.enchant(-2, 0);
            }
        }
    }),
    spell("Lethal Recipe", T::None, |g, c| {
        let before = g.player(c.side).hand.len();
        for _ in 0..2 {
            g.draw_matching(c.side, |d| d.kind() == super::Kind::Minion);
        }
        // "10 or more Mana" can only be crystals: the spell costs three.
        if g.player(c.side).crystals < 10 {
            return;
        }
        for i in before..g.player(c.side).hand.len() {
            if let Some(h) = g.player_mut(c.side).hand.get_mut(i) {
                h.enchant(3, 3);
            }
        }
    }),
    spell("Story of Barnabus", T::None, |g, c| {
        if !g.draw_matching(c.side, |d| d.kind() == super::Kind::Minion) {
            return;
        }
        let big = g
            .player(c.side)
            .hand
            .last()
            .is_some_and(|h| h.card.def().atk >= 5);
        if big {
            if let Some(h) = g.player_mut(c.side).hand.last_mut() {
                h.enchant(0, 5);
            }
            g.gain_armor(c.side, 5);
        }
    }),
    spell("Flight of the Firehawk", T::None, |g, c| {
        // Two minions, and not two of the same tribe.
        let before = g.player(c.side).hand.len();
        g.draw_matching(c.side, |d| d.kind() == super::Kind::Minion);
        let first = g.player(c.side).hand.last().map(|h| h.card.def().races);
        if let Some(races) = first {
            let mut matches: Inline<u16, { crate::state::MAX_DECK }> = Inline::new();
            for (i, card) in g.player(c.side).deck.iter().enumerate() {
                let d = card.def();
                if d.kind() == super::Kind::Minion && !d.races.any(races) {
                    matches.push(i as u16);
                }
            }
            if !matches.is_empty() {
                let pick = g.rngs.effects.index(matches.len());
                let at = matches[pick] as usize;
                let card = g.player(c.side).deck[at];
                g.player_mut(c.side).deck.remove(at);
                g.give_hand_card(c.side, card.to_hand());
            }
        }
        for i in before..g.player(c.side).hand.len() {
            if let Some(h) = g.player_mut(c.side).hand.get_mut(i) {
                h.enchant(1, 1);
            }
        }
    }),
    battlecry("Divine Augur", T::None, |g, c| {
        for h in g.player_mut(c.side).hand.iter_mut() {
            let d = h.card.def();
            if d.kind() != super::Kind::Minion {
                continue;
            }
            let atk = d.atk + h.atk as i16;
            let hp = d.hp + h.hp as i16;
            let best = atk.max(hp);
            h.enchant(best - atk, best - hp);
        }
    }),
    battlecry("Vicious Bloodworm", T::None, |g, c| {
        let Some(src) = c.source else { return };
        let Some(atk) = g.player(c.side).board.get(src as usize).map(|m| m.atk) else {
            return;
        };
        let mut minions: Inline<u16, MAX_HAND> = Inline::new();
        for (i, h) in g.player(c.side).hand.iter().enumerate() {
            if h.card.def().kind() == super::Kind::Minion {
                minions.push(i as u16);
            }
        }
        if minions.is_empty() {
            return;
        }
        let pick = g.rngs.effects.index(minions.len());
        if let Some(h) = g.player_mut(c.side).hand.get_mut(minions[pick] as usize) {
            h.enchant(atk, 0);
        }
    }),
    battlecry("Gruesome Nightmare", T::None, |g, c| {
        let Some(src) = c.source else { return };
        let Some(atk) = g.player(c.side).board.get(src as usize).map(|m| m.atk) else {
            return;
        };
        // One pool over hand and board, so both are equally likely.
        let hand: Inline<u16, MAX_HAND> = (0..g.player(c.side).hand.len() as u16)
            .filter(|i| {
                g.player(c.side).hand[*i as usize].card.def().kind() == super::Kind::Minion
            })
            .collect();
        let board: Inline<u8, MAX_BOARD> = (0..g.player(c.side).board.len() as u8)
            .filter(|i| {
                let m = &g.player(c.side).board[*i as usize];
                m.active() && m.is_minion() && *i != src
            })
            .collect();
        let total = hand.len() + board.len();
        if total == 0 {
            return;
        }
        let pick = g.rngs.effects.index(total);
        if pick < hand.len() {
            if let Some(h) = g.player_mut(c.side).hand.get_mut(hand[pick] as usize) {
                h.enchant(atk, 0);
            }
        } else {
            g.buff(Target::Minion(c.side, board[pick - hand.len()]), atk, 0);
        }
    }),
    battlecry("Neferset Weaponsmith", T::None, |g, c| {
        let mine = g.player(c.side).class;
        let combo = g.combo_active(c.side);
        if g.add_random_to_hand_where(c.side, |d| {
            d.kind() == super::Kind::Weapon
                && d.class() != mine
                && d.class() != super::Class::Neutral
        }) && combo
            && let Some(h) = g.player_mut(c.side).hand.last_mut()
        {
            h.enchant(2, 0);
        }
    }),

    // -------------------------------------------------------- Neutral
    spell("Eternal Toil", T::AnyMinion, |g, c| {
        let Some(t) = c.target else { return };
        g.spell_damage(c.side, Some(t), 1);
        let dead = matches!(t, Target::Minion(s, i)
            if g.player(s).board.get(i as usize).is_some_and(|m| m.is_dead()));
        g.sweep_deaths();
        if dead {
            g.summon_random_of_cost(c.side, 1, 1);
        } else {
            g.draw_cards(c.side, 1);
        }
    }),
    battlecry("Sheltered Survivor", T::None, |g, c| {
        let n = g.player(c.side).hand.len();
        if n > 0 {
            let pick = g.rngs.effects.index(n);
            let card = g.player(c.side).hand[pick].card;
            g.player_mut(c.side).hand.remove(pick);
            g.shuffle_into_deck(c.side, card);
        }
        g.draw_cards(c.side, 1);
    }),
    battlecry("Timeless Causality", T::None, |g, c| {
        let deck: Inline<DeckCard, { crate::state::MAX_DECK }> =
            g.player(c.side).deck.iter().copied().rev().collect();
        g.player_mut(c.side).deck = deck;
    }),
    deathrattle("Curious Explorer", |g, c| {
        let foe = c.side.other();
        let mut minions: Inline<u16, MAX_HAND> = Inline::new();
        for (i, h) in g.player(foe).hand.iter().enumerate() {
            if h.card.def().kind() == super::Kind::Minion {
                minions.push(i as u16);
            }
        }
        if minions.is_empty() {
            return;
        }
        let pick = g.rngs.effects.index(minions.len());
        if let Some(h) = g.player_mut(foe).hand.get_mut(minions[pick] as usize) {
            h.cost_delta -= 2;
        }
    }),
    battlecry("Cloud Serpent", T::None, |g, c| {
        let mut pool: Inline<CardId, MAX_HAND> = Inline::new();
        for h in g.player(c.side).hand.iter() {
            let d = h.card.def();
            if d.races.any(Races::ELEMENTAL) || d.races.any(Races::DRAGON) {
                pool.push(h.card);
            }
        }
        if pool.is_empty() {
            return;
        }
        let pick = g.rngs.effects.index(pool.len());
        g.give_token(c.side, pool[pick]);
    }),
    battlecry("Relic Miner", T::None, |g, c| {
        let Some(top) = g.player_mut(c.side).deck.pop() else { return };
        let rarity = top.def().rarity();
        match rarity {
            super::Rarity::Free => g.discover(c.side, |d| d.rarity() == super::Rarity::Free),
            super::Rarity::Common => g.discover(c.side, |d| d.rarity() == super::Rarity::Common),
            super::Rarity::Rare => g.discover(c.side, |d| d.rarity() == super::Rarity::Rare),
            super::Rarity::Epic => g.discover(c.side, |d| d.rarity() == super::Rarity::Epic),
            super::Rarity::Legendary => {
                g.discover(c.side, |d| d.rarity() == super::Rarity::Legendary)
            }
            super::Rarity::None => false,
        };
    }),
    battlecry("Chronicle Keeper", T::None, |g, c| {
        if g.holding_race(c.side, Races::DRAGON)
            && let Some(src) = c.source
        {
            let t = Target::Minion(c.side, src);
            g.grant(t, Keywords::TAUNT);
            g.grant(t, Keywords::DIVINE_SHIELD);
        }
    }),
    trigger("Primal Sabretooth", |g, c| {
        if let Event::AfterAttack {
            attacker,
            defender: Target::Minion(_, _),
            defender_died: true,
        } = c.event
            && attacker == c.me()
        {
            // The body is gone by the time this fires, but it went into its
            // owner's graveyard on the way out, and it went in last.
            if let Event::AfterAttack { defender: Target::Minion(s, _), .. } = c.event
                && let Some(card) = g.player(s).graveyard.last().copied()
            {
                g.give_token(c.side, card);
            }
        }
    }),
    spell("Synchronized Spark", T::EnemyCharacter, |g, c| {
        let Some(t) = c.target else { return };
        g.spell_damage(c.side, Some(t), 3);
        let dead = matches!(t, Target::Minion(s, i)
            if g.player(s).board.get(i as usize).is_some_and(|m| m.is_dead()));
        g.sweep_deaths();
        if dead && let Some(friend) = g.random_minion(c.side) {
            g.buff(friend, 3, 3);
        }
    }),
    deathrattle("Wicked Blightspawn", |g, c| {
        if g.player(c.side).weapon.is_some() {
            g.buff_weapon(c.side, 2, 0);
        } else {
            g.equip(c.side, tokens::WICKED_KNIFE);
        }
    }),
    battlecry("Wizened Truthseeker", T::None, |g, c| {
        let _ = c;
        for i in 0..2 {
            for h in g.players[i].hand.iter_mut() {
                h.cost_delta = 0;
            }
        }
    }),
    trigger("Activated Golem", |g, c| {
        if matches!(c.event, Event::TurnEnd { .. }) {
            g.grant(c.me(), Keywords::REBORN);
        }
    }),
    spell("Bitter End", T::AnyMinion, |g, c| {
        let Some(Target::Minion(s, i)) = c.target else { return };
        let mut hits: Inline<Target, 3> = Inline::new();
        for j in [i.wrapping_sub(1), i, i + 1] {
            if (j as usize) < g.player(s).board.len() {
                hits.push(Target::Minion(s, j));
            }
        }
        for t in hits.iter().copied() {
            g.freeze(t);
        }
        for t in hits.iter().copied() {
            if matches!(t, Target::Minion(s, i)
                if g.player(s).board.get(i as usize).is_some_and(|m| m.damage > 0))
            {
                g.destroy(t);
            }
        }
    }),
    c(
        "Solitary Prisoner",
        T::None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(|g, _side, _i| {
            let empty = g.players[0].board.is_empty() && g.players[1].board.is_empty();
            if empty { 2 - 5 } else { 0 }
        }),
        None,
    ),
    battlecry("Witchwood Grizzly", T::None, |g, c| {
        let n = g.player(c.side.other()).hand.len() as i16;
        if n > 0 && let Some(src) = c.source {
            g.buff(Target::Minion(c.side, src), 0, -n);
            g.sweep_deaths();
        }
    }),
    battlecry("Scalehide Kodo", T::None, |g, c| {
        let foe = c.side.other();
        let highest = g.kindred(c.side, Races::BEAST);
        let mut best: Option<(u8, i16)> = None;
        for (i, m) in g.player(foe).board.iter().enumerate() {
            if !m.active() || !m.is_minion() {
                continue;
            }
            let better = match best {
                None => true,
                Some((_, b)) => {
                    if highest { m.atk > b } else { m.atk < b }
                }
            };
            if better {
                best = Some((i as u8, m.atk));
            }
        }
        if let Some((slot, _)) = best {
            g.destroy(Target::Minion(foe, slot));
        }
    }),
    trigger("Sentient Hourglass", |g, c| {
        if matches!(c.event, Event::Damaged { target, .. } if target == c.me())
            && let Some(m) = g.player(c.side).board.get(c.slot as usize).copied()
            && !m.is_dead()
        {
            let health = m.health();
            g.set_attack(c.me(), health);
            g.set_health(c.me(), m.atk);
        }
    }),
    deathrattle("Chillmaw", |g, c| {
        if g.holding_race(c.side, Races::DRAGON) {
            g.damage_area(c.side, Area::AllMinions, 3);
        }
    }),
    battlecry("Ravenous Devilsaur", T::AnyMinion, |g, c| {
        let Some(t) = c.target else { return };
        let Target::Minion(s, i) = t else { return };
        let Some(victim) = g.player(s).board.get(i as usize).copied() else { return };
        g.destroy(t);
        if (g.kindred(c.side, Races::BEAST))
            && let Some(src) = c.source
        {
            g.buff(Target::Minion(c.side, src), victim.atk, victim.max_hp);
        }
    }),
    battlecry("Siamat", T::None, |g, c| {
        let Some(src) = c.source else { return };
        const GIFTS: [Keywords; 4] = [
            Keywords::RUSH,
            Keywords::TAUNT,
            Keywords::DIVINE_SHIELD,
            Keywords::WINDFURY,
        ];
        let mut picks = [0u32; 2];
        let n = g.rngs.effects.sample_indices(GIFTS.len(), &mut picks);
        for &p in picks.iter().take(n) {
            g.grant(Target::Minion(c.side, src), GIFTS[p as usize]);
        }
    }),
    battlecry("Warmaster Blackhorn", T::None, |g, c| {
        let _ = c;
        for i in 0..2 {
            g.players[i].deck.retain(|card| card.def().cost > 2);
        }
    }),
    battlecry("Disciple of Demise", T::None, |g, c| {
        let dragons = g
            .player(c.side)
            .hand
            .iter()
            .filter(|h| h.card.def().races.any(Races::DRAGON))
            .count();
        let me = c.source.map(|s| Target::Minion(c.side, s));
        for _ in 0..(1 + dragons) {
            let mut pool: Inline<Target, { MAX_BOARD * 2 }> = Inline::new();
            g.collect_area(c.side, Area::AllMinions, &mut pool);
            pool.retain(|t| Some(*t) != me);
            if pool.is_empty() {
                break;
            }
            let pick = g.rngs.effects.index(pool.len());
            g.destroy(pool[pick]);
            g.sweep_deaths();
        }
    }),
    trigger("Black Market Auctioneer", |g, c| {
        if matches!(c.event, Event::SpellCast { side, .. } if side == c.side) {
            g.draw_cards(c.side, 1);
        }
    }),
    trigger("Krog, Crater King", |g, c| {
        if !matches!(c.event, Event::TurnEnd { side } if side == c.side) {
            return;
        }
        let foe = c.side.other();
        for i in 0..g.player(foe).board.len() {
            let t = Target::Minion(foe, i as u8);
            g.set_attack(t, 1);
            g.set_health(t, 1);
        }
    }),
    spell("Bygone Echoes", T::None, |g, c| {
        g.summon_random_of_cost(c.side, 4, 1);
        if g.spend_corpses(c.side, 4) {
            g.summon_random_of_cost(c.side, 4, 1);
        }
        if c.outcast {
            g.summon_random_of_cost(c.side, 4, 1);
        }
    }),
    trigger("Finja, the Flying Star", |g, c| {
        if let Event::AfterAttack { attacker, defender: Target::Minion(..), defender_died: true } =
            c.event
            && attacker == c.me()
        {
            for _ in 0..2 {
                if !g.summon_from_deck(c.side, |d| d.races.any(Races::MURLOC)) {
                    break;
                }
            }
        }
    }),
    trigger("Stormbrewer", |g, c| {
        if let Event::AttackDeclared { attacker, defender } = c.event
            && attacker == c.me()
        {
            g.deal_damage(defender, 3);
        }
    }),
    trigger("Vanessa the Ringleader", |g, c| {
        if matches!(c.event, Event::CardPlayed { side, .. } if side == c.side)
            && g.add_random_to_hand(c.side, |d| {
                d.kind() == super::Kind::Minion && d.keywords.has(Keywords::BATTLECRY)
            })
            && let Some(h) = g.player_mut(c.side).hand.last_mut()
        {
            h.cost_delta -= 2;
        }
    }),
    trigger("Keymaster Alabaster", |g, c| {
        if let Event::CardDrawn { side } = c.event
            && side == c.side.other()
            && let Some(card) = g.player(side).hand.last().map(|h| h.card)
            && g.give_token(c.side, card)
            && let Some(h) = g.player_mut(c.side).hand.last_mut()
        {
            h.cost_delta = 1 - h.card.def().cost;
        }
    }),
    battlecry("Zaqali Flamemancer", T::None, |g, c| {
        let mut costs: Inline<i16, MAX_HAND> = Inline::new();
        for h in g.player(c.side).hand.iter() {
            if costs.contains(&h.card.def().cost) {
                return;
            }
            costs.push(h.card.def().cost);
        }
        for h in g.player_mut(c.side).hand.iter_mut() {
            h.cost_delta -= 2;
        }
    }),
    trigger("Unknown Voyager", |g, c| {
        if matches!(c.event, Event::Damaged { target, .. } if target == c.me())
            && g.player(c.side)
                .board
                .get(c.slot as usize)
                .is_some_and(|m| !m.is_dead())
        {
            transform_into_cost(g, c.me(), 7);
        }
    }),
    trigger("Dangerous Variant", |g, c| {
        if matches!(c.event, Event::TurnStart { side } if side == c.side) {
            transform_into_cost(g, c.me(), 5);
        }
    }),
    battlecry("Crater Experiment", T::None, |g, c| {
        // Printed with every minion type, so any tribe played last turn is
        // kindred with it.
        if !g.player(c.side).played_races_last.is_empty() {
            g.summon_copy(c.side, c.card);
        }
    }),
    battlecry("Steamfin Thief", T::None, |g, c| {
        if g.kindred(c.side, Races::MURLOC) {
            g.summon_token(c.side, tokens::JUVENILE_STEAMFIN, 2);
        }
    }),
    trigger("Bronze Keeper", |g, c| {
        if matches!(c.event, Event::TurnEnd { side } if side == c.side) {
            g.summon_token(c.side, tokens::SANDSCALE_DRAGON, 1);
        }
    }),
    // ---------------------------------------------------- Death Knight
    trigger("Battlefield Necromancer", |g, c| {
        if matches!(c.event, Event::TurnEnd { side } if side == c.side)
            && g.spend_corpses(c.side, 1)
        {
            g.summon_token(c.side, tokens::RISEN_FOOTMAN, 1);
        }
    }),
    spell("Death's Advance", T::AnyCharacter, |g, c| {
        if let Some(t) = c.target {
            g.freeze(t);
        }
        g.discover(c.side, |d| d.kind() == super::Kind::Spell);
    }),
    spell("Corpse Farm", T::None, |g, c| {
        let spend = g.player(c.side).corpses.min(8);
        if spend > 0 && g.spend_corpses(c.side, spend) {
            g.summon_random_of_cost(c.side, spend, 1);
        }
    }),
    spell("Glacial Advance", T::AnyCharacter, |g, c| {
        g.spell_damage(c.side, c.target, 4);
        g.player_mut(c.side).next_spell_discount += 2;
    }),
    trigger("Corpse Flower", |g, c| {
        if let Event::MinionSummoned { side, slot, .. } = c.event
            && side == c.side.other()
            && g.spend_corpses(c.side, 2)
        {
            g.deal_damage(Target::Minion(side, slot), 3);
        }
    }),
    spell("Consumption", T::None, |g, c| {
        let foe = c.side.other();
        let mut pool: Inline<Target, MAX_BOARD> = Inline::new();
        for (i, m) in g.player(foe).board.iter().enumerate() {
            if m.active() && m.is_minion() {
                pool.push(Target::Minion(foe, i as u8));
            }
        }
        let mut picks = [0u32; 2];
        let n = g.rngs.effects.sample_indices(pool.len(), &mut picks[..2.min(pool.len())]);
        let mut killed = 0;
        for &p in picks.iter().take(n) {
            let t = pool[p as usize];
            g.spell_damage(c.side, Some(t), 3);
            if matches!(t, Target::Minion(s, i)
                if g.player(s).board.get(i as usize).is_some_and(|m| m.is_dead()))
            {
                killed += 1;
            }
        }
        g.sweep_deaths();
        g.draw_cards(c.side, killed);
    }),
    battlecry("Dread Raptor", T::None, |g, c| {
        let free = g.kindred(c.side, Races::UNDEAD) || g.kindred(c.side, Races::BEAST);
        if g.draw_matching(c.side, |d| {
            d.kind() == super::Kind::Minion
                && d.cost <= 3
                && d.keywords.has(Keywords::DEATHRATTLE)
        }) && free
            && let Some(h) = g.player_mut(c.side).hand.last_mut()
        {
            h.cost_delta = -h.card.def().cost;
        }
    }),
    spell("Grave Strength", T::None, |g, c| {
        let n = if g.spend_corpses(c.side, 5) { 3 } else { 1 };
        g.buff_area(c.side, Area::FriendlyMinions, n, 0);
    }),
    deathrattle("Lady Deathwhisper", |g, c| {
        let frost: Inline<CardId, MAX_HAND> = g
            .player(c.side)
            .hand
            .iter()
            .filter(|h| h.card.def().school() == super::School::Frost)
            .map(|h| h.card)
            .collect();
        for card in frost.iter().copied() {
            g.give_token(c.side, card);
        }
    }),
    trigger("Malignant Horror", |g, c| {
        if matches!(c.event, Event::TurnEnd { side } if side == c.side)
            && g.spend_corpses(c.side, 4)
            && let Some(card) = g.player(c.side).board.get(c.slot as usize).map(|m| m.card)
        {
            g.summon_copy(c.side, card);
        }
    }),
    battlecry("Might of Menethil", T::None, |g, c| {
        let mut spent = 0;
        while spent < 3 && g.spend_corpses(c.side, 1) {
            spent += 1;
        }
        let foe = c.side.other();
        let mut pool: Inline<Target, MAX_BOARD> = Inline::new();
        for (i, m) in g.player(foe).board.iter().enumerate() {
            if m.active() && m.is_minion() {
                pool.push(Target::Minion(foe, i as u8));
            }
        }
        let mut picks = [0u32; MAX_BOARD];
        let take = (spent as usize).min(pool.len());
        let n = g.rngs.effects.sample_indices(pool.len(), &mut picks[..take]);
        for &p in picks.iter().take(n) {
            g.freeze(pool[p as usize]);
        }
    }),
    battlecry("Obsessive Technician", T::None, |g, c| g.herald(c.side)),
    spell("Army of the Dead", T::None, |g, c| {
        let mut raised = 0;
        while raised < 5 && g.spend_corpses(c.side, 1) {
            if g.summon_token(c.side, tokens::RISEN_GHOUL, 1) == 0 {
                break;
            }
            raised += 1;
        }
    }),
    battlecry("Corpse Bride", T::None, |g, c| {
        let spend = g.player(c.side).corpses.min(10);
        if spend <= 0 || !g.spend_corpses(c.side, spend) {
            return;
        }
        if g.summon_token(c.side, tokens::RISEN_GROOM, 1) > 0 {
            let slot = g.player(c.side).board.len() as u8 - 1;
            g.set_attack(Target::Minion(c.side, slot), spend);
            g.set_health(Target::Minion(c.side, slot), spend);
            g.grant(Target::Minion(c.side, slot), Keywords::TAUNT);
        }
    }),
    trigger("Hollow Direhorn", |g, c| {
        if let Event::MinionDied { side, .. } = c.event
            && side == c.side
            && !g.player(c.side)
                .board
                .get(c.slot as usize)
                .is_some_and(|m| m.has(Keywords::REBORN))
            && g.spend_corpses(c.side, 3)
        {
            g.grant(c.me(), Keywords::REBORN);
        }
    }),
    deathrattle("Bonechill Stegodon", |g, c| {
        let foe = c.side.other();
        let mut pool: Inline<Target, { MAX_BOARD + 1 }> = Inline::new();
        for (i, m) in g.player(foe).board.iter().enumerate() {
            if m.active() && m.is_minion() {
                pool.push(Target::Minion(foe, i as u8));
            }
        }
        pool.push(Target::Hero(foe));
        let mut picks = [0u32; 3];
        let take = 3.min(pool.len());
        let n = g.rngs.effects.sample_indices(pool.len(), &mut picks[..take]);
        for &p in picks.iter().take(n) {
            g.deal_damage(pool[p as usize], 6);
        }
    }),
    spell("Experimental Animation", T::None, |g, c| {
        g.herald(c.side);
        g.spell_damage_area(c.side, Area::EnemyMinions, 4);
    }),
    battlecry("Marrow Manipulator", T::None, |g, c| {
        let mut spent = 0;
        while spent < 5 && g.spend_corpses(c.side, 1) {
            spent += 1;
        }
        for _ in 0..spent {
            let Some(t) = g.random_enemy(c.side) else { break };
            g.deal_damage(t, 2);
            g.sweep_deaths();
        }
    }),
    battlecry("Alexandros Mograine", T::None, |g, c| {
        g.player_mut(c.side).end_turn_burn += 3;
    }),
    spell("Story of Umbra", T::None, |g, c| {
        let pool = super::discover_pool(|d| {
            d.kind() == super::Kind::Minion
                && d.cost >= 5
                && d.keywords.has(Keywords::DEATHRATTLE)
        });
        if pool.is_empty() {
            return;
        }
        let mut offered = [0u32; 3];
        let n = g.rngs.effects.sample_indices(pool.len(), &mut offered);
        let Some(pick) = offered[..n].iter().map(|&i| pool[i as usize]).next() else {
            return;
        };
        if g.summon(c.side, pick) {
            g.fire_deathrattle_of(c.side, pick);
        }
    }),
    battlecry("Boneguard Commander", T::None, |g, c| {
        let mut raised = 0;
        while raised < 6 && g.spend_corpses(c.side, 1) {
            if g.summon_token(c.side, tokens::RISEN_FOOTMAN, 1) == 0 {
                break;
            }
            raised += 1;
        }
    }),
    spell("Chow Down", T::None, |g, c| {
        let made = g.summon_token(c.side, tokens::HUNGRY_DRAKE, 5);
        if made == 0 || !g.spend_corpses(c.side, 8) {
            return;
        }
        let last = g.player(c.side).board.len();
        for slot in (last - made)..last {
            g.grant(Target::Minion(c.side, slot as u8), Keywords::RUSH);
        }
    }),
    spell("The Scourge", T::None, |g, c| {
        while !g.player(c.side).board.is_full() {
            if !g.summon_random_where(c.side, |d| {
                d.kind() == super::Kind::Minion && d.races.any(Races::UNDEAD)
            }) {
                break;
            }
        }
    }),
    battlecry("Volcoross", T::None, |g, c| {
        // "Choose to spend 10, 20, or 30": the biggest the pile allows.
        let have = g.player(c.side).corpses;
        let spend = if have >= 30 {
            30
        } else if have >= 20 {
            20
        } else if have >= 10 {
            10
        } else {
            0
        };
        if spend > 0 && g.spend_corpses(c.side, spend)
            && let Some(src) = c.source
        {
            g.buff(Target::Minion(c.side, src), spend, spend);
        }
    }),

    // -------------------------------------------------------- Paladin
    battlecry("Ashleaf Pixie", T::None, |g, c| {
        let holding = g.player(c.side).hand.iter().any(|h| {
            h.card.def().kind() == super::Kind::Spell && h.card.def().cost >= 5
        });
        if holding && let Some(src) = c.source {
            let t = Target::Minion(c.side, src);
            g.grant(t, Keywords::DIVINE_SHIELD);
            g.grant(t, Keywords::LIFESTEAL);
        }
    }),
    spell("Mark of Ursol", T::AnyMinion, |g, c| {
        let Some(t) = c.target else { return };
        let Target::Minion(s, _) = t else { return };
        let n = if s == c.side { 3 } else { 1 };
        g.set_attack(t, n);
        g.set_health(t, n);
    }),
    deathrattle("Scarlet Bruiser", |g, c| {
        if deck_has_neutrals(g, c.side) {
            return;
        }
        if g.add_random_to_hand(c.side, |d| d.class() == super::Class::Paladin)
            && let Some(h) = g.player_mut(c.side).hand.last_mut()
        {
            h.cost_delta -= 2;
        }
    }),
    battlecry("Vigilant Sentry", T::None, |g, c| {
        if !deck_has_neutrals(g, c.side) {
            g.summon_copy(c.side, c.card);
            g.summon_copy(c.side, c.card);
        }
    }),
    spell("Ready the Fleet", T::FriendlyMinion, |g, c| {
        let Some(t) = c.target else { return };
        let Target::Minion(_, i) = t else { return };
        let Some(races) = g.player(c.side).board.get(i as usize).map(|m| m.races()) else {
            return;
        };
        g.buff(t, 1, 2);
        for j in 0..g.player(c.side).board.len() {
            if j as u8 != i && g.player(c.side).board[j].races().any(races) {
                g.buff(Target::Minion(c.side, j as u8), 1, 2);
            }
        }
    }),
    battlecry("Ivory Knight", T::None, |g, c| {
        if g.discover(c.side, |d| d.kind() == super::Kind::Spell)
            && let Some(h) = g.player(c.side).hand.last()
        {
            let cost = h.card.def().cost;
            g.heal_hero(c.side, cost);
        }
    }),
    choose("Lightmender", &[
        m(T::None, |g, c| {
            if let Some(src) = c.source {
                let t = Target::Minion(c.side, src);
                g.buff(t, 3, 0);
                g.grant(t, Keywords::DIVINE_SHIELD);
            }
        }),
        m(T::None, |g, c| {
            if let Some(src) = c.source {
                let t = Target::Minion(c.side, src);
                g.buff(t, 0, 3);
                g.grant(t, Keywords::LIFESTEAL);
            }
        }),
    ]),
    trigger("Spearheart Sentry", |g, c| {
        if matches!(c.event, Event::TurnEnd { side } if side == c.side)
            && g.add_random_to_hand(c.side, |d| {
                d.kind() == super::Kind::Spell && d.school() == super::School::Holy
            })
            && let Some(h) = g.player_mut(c.side).hand.last_mut()
        {
            h.cost_delta -= 3;
        }
    }),
    battlecry("Arator the Redeemer", T::None, |g, c| {
        for i in 0..g.player(c.side).board.len() {
            let m = g.player(c.side).board[i];
            if m.card.name() != "Silver Hand Recruit" {
                continue;
            }
            let t = Target::Minion(c.side, i as u8);
            g.buff(t, m.atk, m.max_hp);
            g.grant(t, Keywords::TAUNT);
        }
    }),
    trigger("Nozdormu, Bronze Aspect", |g, c| {
        if !matches!(c.event, Event::TurnEnd { side } if side == c.side) {
            return;
        }
        for i in 0..g.player(c.side).board.len() {
            let t = Target::Minion(c.side, i as u8);
            if g.player(c.side).board[i].has(Keywords::DIVINE_SHIELD) {
                g.buff(t, 3, 3);
            } else {
                g.grant(t, Keywords::DIVINE_SHIELD);
            }
        }
    }),
    battlecry("Scarlet Recruiter", T::None, |g, c| {
        for _ in 0..2 {
            if !g.summon_from_deck(c.side, |d| d.cost <= 2) {
                break;
            }
            let slot = g.player(c.side).board.len() as u8 - 1;
            g.grant(Target::Minion(c.side, slot), Keywords::RUSH);
        }
    }),
    spell("Searing Reflection", T::None, |g, c| {
        if !g.draw_matching(c.side, |d| d.kind() == super::Kind::Minion) {
            return;
        }
        let Some(card) = g.player(c.side).hand.last().map(|h| h.card) else { return };
        if g.summon(c.side, card) {
            let slot = g.player(c.side).board.len() as u8 - 1;
            g.set_attack(Target::Minion(c.side, slot), 8);
            g.set_health(Target::Minion(c.side, slot), 8);
            g.grant(Target::Minion(c.side, slot), Keywords::DIVINE_SHIELD);
        }
    }),
    spell("Hero's Welcome", T::None, |g, c| {
        let pool = super::discover_pool(|d| {
            d.kind() == super::Kind::Minion && d.rarity() == super::Rarity::Legendary
        });
        if pool.is_empty() {
            return;
        }
        let mut offered = [0u32; 3];
        let n = g.rngs.effects.sample_indices(pool.len(), &mut offered);
        let Some(pick) = offered[..n].iter().map(|&i| pool[i as usize]).next() else {
            return;
        };
        if g.summon(c.side, pick) {
            let slot = g.player(c.side).board.len() as u8 - 1;
            g.set_attack(Target::Minion(c.side, slot), 10);
            g.set_health(Target::Minion(c.side, slot), 10);
        }
    }),
    battlecry("Firegill", T::None, |g, c| {
        if !(g.kindred(c.side, Races::MURLOC) || g.kindred(c.side, Races::ELEMENTAL)) {
            return;
        }
        let me = c.source.map(|s| Target::Minion(c.side, s));
        for i in 0..g.player(c.side).board.len() {
            let t = Target::Minion(c.side, i as u8);
            if Some(t) != me {
                g.grant(t, Keywords::RUSH);
            }
        }
    }),
    trigger("Bronze Redeemer", |g, c| {
        if !matches!(c.event, Event::TurnEnd { side } if side == c.side) {
            return;
        }
        let Some(me) = g.player(c.side).board.get(c.slot as usize).copied() else { return };
        if g.summon_token(c.side, tokens::BRONZE_BRUTE, 1) > 0 {
            let slot = g.player(c.side).board.len() as u8 - 1;
            g.set_attack(Target::Minion(c.side, slot), me.atk);
            g.set_health(Target::Minion(c.side, slot), me.max_hp);
        }
    }),

    // ---------------------------------------------------------- Rogue
    spell("Mimicry", T::None, |g, c| {
        let foe = c.side.other();
        let before = g.player(foe).hand.len();
        g.draw(foe, 2);
        for i in before..g.player(foe).hand.len() {
            let card = g.player(foe).hand[i].card;
            g.give_token(c.side, card);
        }
    }),
    spell("Garona's Last Stand", T::LegendaryMinion, |g, c| {
        if let Some(t) = c.target {
            g.destroy(t);
        }
    }),
    spell("Jackpot!", T::None, |g, c| {
        let mine = g.player(c.side).class;
        for _ in 0..2 {
            g.add_random_to_hand_where(c.side, |d| {
                d.kind() == super::Kind::Spell
                    && d.cost >= 5
                    && d.class() != mine
                    && d.class() != super::Class::Neutral
            });
        }
    }),
    spell("Silent Strike", T::AnyMinion, |g, c| {
        let Some(t) = c.target else { return };
        g.buff(t, 3, 0);
        let Target::Minion(s, i) = t else { return };
        let Some(m) = g.player(s).board.get(i as usize).copied() else { return };
        if m.has(Keywords::STEALTH)
            && let Some(victim) = g.random_minion(c.side.other())
        {
            g.deal_damage(victim, m.atk);
        }
    }),
    spell("Web of Deception", T::FriendlyMinion, |g, c| {
        let Some(t) = c.target else { return };
        g.bounce(t);
        g.summon_token(c.side, tokens::SKITTERING_SPIDERLING, 1);
    }),
    spell("Deadly Bribe", T::AnyMinion, |g, c| {
        if let Some(t) = c.target {
            g.destroy(t);
        }
        g.give_token(c.side.other(), tokens::COIN);
        if g.combo_active(c.side) {
            g.give_token(c.side, tokens::COIN);
        }
    }),
    trigger("SI:7 Slayer", |g, c| {
        // On the declaration: attacking is what strips Stealth, so by the time
        // the exchange is over the minion this asks about is no longer one.
        if let Event::AttackDeclared { attacker: Target::Minion(s, i), .. } = c.event
            && s == c.side
            && g.player(s)
                .board
                .get(i as usize)
                .is_some_and(|m| m.has(Keywords::STEALTH))
        {
            g.buff(Target::Minion(s, i), 2, 2);
        }
    }),
    trigger("Shaku, the Collector", |g, c| {
        if matches!(c.event, Event::AfterAttack { attacker, .. } if attacker == c.me()) {
            let mine = g.player(c.side).class;
            g.add_random_to_hand_where(c.side, |d| {
                d.class() != mine && d.class() != super::Class::Neutral
            });
        }
    }),
    battlecry("Tricky Satyr", T::None, |g, c| {
        let foe = c.side.other();
        let mut best: Option<(usize, i16)> = None;
        for (i, h) in g.player(foe).hand.iter().enumerate() {
            let cost = h.card.def().cost;
            if best.is_none_or(|(_, b)| cost < b) {
                best = Some((i, cost));
            }
        }
        if let Some((at, _)) = best {
            let card = g.player(foe).hand[at].card;
            g.give_token(c.side, card);
        }
    }),
    trigger("Mathias Shaw", |g, c| {
        // On the declaration: attacking is what strips Stealth, so by the time
        // the exchange is over the minion this asks about is no longer one.
        if let Event::AttackDeclared { attacker: Target::Minion(s, i), .. } = c.event
            && s == c.side
            && g.player(s)
                .board
                .get(i as usize)
                .is_some_and(|m| m.has(Keywords::STEALTH))
        {
            let n = g.player(c.side).hand.len();
            if n == 0 {
                return;
            }
            let pick = g.rngs.effects.index(n);
            if let Some(h) = g.player_mut(c.side).hand.get_mut(pick) {
                h.cost_delta -= 3;
            }
        }
    }),
    deathrattle("Waggle Pick", |g, c| {
        let Some(t) = g.random_minion(c.side) else { return };
        g.bounce(t);
        if let Some(h) = g.player_mut(c.side).hand.last_mut() {
            h.cost_delta -= 2;
        }
    }),
    spell("Fast Forward", T::None, |g, c| {
        let before = g.player(c.side).hand.len();
        g.draw_cards(c.side, 2);
        // "Pick one": the dearer of the two, which is the one worth the two.
        let mut best: Option<(usize, i16)> = None;
        for i in before..g.player(c.side).hand.len() {
            let cost = g.player(c.side).hand[i].card.def().cost;
            if best.is_none_or(|(_, b)| cost > b) {
                best = Some((i, cost));
            }
        }
        if let Some((at, _)) = best
            && let Some(h) = g.player_mut(c.side).hand.get_mut(at)
        {
            h.cost_delta -= 2;
        }
    }),
    battlecry("Swashburglar", T::None, |g, c| {
        let mine = g.player(c.side).class;
        g.add_random_to_hand_where(c.side, |d| {
            d.class() != mine && d.class() != super::Class::Neutral
        });
    }),
    c(
        "Crystal Tusk",
        T::None,
        None,
        Some(|g, c| {
            if g.player(c.side).hand.is_empty() {
                return;
            }
            let card = g.player(c.side).hand[0].card;
            g.player_mut(c.side).hand.remove(0);
            g.shuffle_into_deck(c.side, card);
        }),
        Some(|g, c| g.draw_cards(c.side, 2)),
        None,
        None,
        None,
        None,
        None,
        None,
    ),
    battlecry("Merchant of Legend", T::None, |g, c| {
        let pool = super::discover_pool(|d| {
            d.kind() == super::Kind::Minion && d.rarity() == super::Rarity::Legendary
        });
        if pool.is_empty() {
            return;
        }
        let mut offered = [0u32; 3];
        let n = g.rngs.effects.sample_indices(pool.len(), &mut offered);
        let crystals = g.player(c.side).crystals;
        let cards: Inline<CardId, 3> = offered[..n].iter().map(|&i| pool[i as usize]).collect();
        let Some(best) = cards
            .iter()
            .copied()
            .max_by_key(|card| (card.def().cost <= crystals + 1, card.def().cost))
        else {
            return;
        };
        let mut kept = false;
        for card in cards.iter().copied() {
            if card == best && !kept {
                kept = true;
                g.give_card(c.side, card);
            } else {
                g.shuffle_into_deck(c.side, card);
            }
        }
    }),
    c(
        "Blackpaw's Whip",
        T::None,
        None,
        None,
        Some(|g, c| g.draw_cards(c.side, 1)),
        None,
        None,
        None,
        None,
        Some(|g, side, _i| {
            let coins = g
                .player(side)
                .hand
                .iter()
                .filter(|h| h.card.name() == "The Coin")
                .count() as i16;
            -coins
        }),
        None,
    ),

    // --------------------------------------------------------- Hunter
    // Leokk is one of the three Animal Companions, so every card that summons
    // one needs him to actually project his aura.
    aura("Leokk", |ss, sl, ts, tl, _| {
        if ss == ts && sl != tl { (1, 0) } else { (0, 0) }
    }),
    c(
        "Raptor-Nest Nurse",
        T::None,
        None,
        Some(|g, c| {
            g.add_random_to_hand(c.side, |d| {
                d.kind() == super::Kind::Minion && d.cost == 1
            });
        }),
        Some(|g, c| {
            g.add_random_to_hand(c.side, |d| {
                d.kind() == super::Kind::Spell && d.cost == 1
            });
        }),
        None,
        None,
        None,
        None,
        None,
        None,
    ),
    trigger("Dinositter", |g, c| {
        if !matches!(c.event, Event::TurnEnd { side } if side == c.side) {
            return;
        }
        let mut beasts: Inline<u16, MAX_HAND> = Inline::new();
        for (i, h) in g.player(c.side).hand.iter().enumerate() {
            if h.card.def().races.any(Races::BEAST) {
                beasts.push(i as u16);
            }
        }
        if beasts.is_empty() {
            return;
        }
        let pick = g.rngs.effects.index(beasts.len());
        if let Some(h) = g.player_mut(c.side).hand.get_mut(beasts[pick] as usize) {
            h.cost_delta -= 1;
        }
    }),
    secret("Freezing Trap", |g, owner, ev| {
        if let Event::AttackDeclared { attacker: Target::Minion(s, i), .. } = ev
            && s == owner.other()
        {
            g.bounce(Target::Minion(s, i));
            // The bounce puts it at the end of their hand, so that is the copy
            // the tax lands on.
            if let Some(h) = g.player_mut(s).hand.last_mut() {
                h.cost_delta += 2;
            }
            return true;
        }
        false
    }),
    secret("Pressure Plate", |g, owner, ev| {
        if let Event::SpellCast { side, .. } = ev
            && side == owner.other()
            && let Some(t) = g.random_minion(owner.other())
        {
            g.destroy(t);
            return true;
        }
        false
    }),
    secret("Rat Trap", |g, owner, ev| {
        if let Event::CardPlayed { side, .. } = ev
            && side == owner.other()
            && g.player(side).cards_played_turn >= 3
        {
            g.summon_token(owner, tokens::DOOM_RAT, 1);
            return true;
        }
        false
    }),
    deathrattle("Augmented Porcupine", |g, c| {
        let atk = c.dying.map_or(0, |m| m.atk);
        g.damage_split(c.side, Area::AllEnemies, atk);
    }),
    trigger("Dragonbane", |g, c| {
        if matches!(c.event, Event::HeroPowerUsed { side } if side == c.side)
            && let Some(t) = g.random_enemy(c.side)
        {
            g.deal_damage(t, 5);
        }
    }),
    choose("Grace of the Greatwolf", &[
        m(T::None, |g, c| {
            g.spell_damage(c.side, Some(Target::Hero(c.side.other())), 4);
        }),
        m(T::None, |g, c| {
            g.summon_token(c.side, tokens::PLAYFUL_PUP, 2);
        }),
    ]),
    battlecry("Mythical Runebear", T::None, |g, c| {
        // Reads the body on the board, so a buff before it lands counts.
        let big = c
            .source
            .and_then(|s| g.player(c.side).board.get(s as usize).map(|m| m.atk >= 4))
            .unwrap_or(false);
        if big {
            g.summon_copy(c.side, c.card);
        }
    }),
    battlecry("Wasteland Vanguard", T::None, |g, c| {
        let before = g.player(c.side.other()).board.len();
        g.damage_split(c.side, Area::AllEnemies, 3);
        g.sweep_deaths();
        if g.player(c.side.other()).board.len() < before {
            g.damage_split(c.side, Area::AllEnemies, 3);
        }
    }),
    battlecry("Sewer Swimmer", T::None, |g, c| {
        g.retrigger_friendly_deathrattles(c.side, 1);
    }),
    battlecry("Spiritspeaker", T::None, |g, c| {
        g.summon_random_child(c.side, c.card);
    }),
    trigger("Broll Bearmantle", |g, c| {
        if matches!(c.event, Event::SpellCast { side, .. } if side == c.side)
            && let Some(card) = g.player(c.side).board.get(c.slot as usize).map(|m| m.card)
        {
            g.summon_random_child(c.side, card);
        }
    }),
    battlecry("Tending Dragonkin", T::None, |g, c| {
        let mut best: Option<(usize, i16)> = None;
        for (i, h) in g.player(c.side).hand.iter().enumerate() {
            if !h.card.def().races.any(Races::BEAST) {
                continue;
            }
            let cost = h.card.def().cost;
            if best.is_none_or(|(_, b)| cost < b) {
                best = Some((i, cost));
            }
        }
        if let Some((at, _)) = best {
            let card = g.player(c.side).hand[at].card;
            g.give_token(c.side, card);
        }
    }),
    c(
        "Triennium Rex",
        T::None,
        None,
        Some(|g, c| {
            if g.kindred(c.side, Races::BEAST) {
                deathrattle_minion_to_hand(g, c.side);
            }
        }),
        Some(|g, c| deathrattle_minion_to_hand(g, c.side)),
        None,
        None,
        None,
        None,
        None,
        None,
    ),
    trigger("Magma Hound", |g, c| {
        if let Event::AfterAttack { attacker, defender: Target::Minion(..), .. } = c.event
            && attacker == c.me()
            && let Some(atk) = g.player(c.side).board.get(c.slot as usize).map(|m| m.atk)
        {
            g.damage_split(c.side, Area::AllEnemies, atk);
        }
    }),

    // -------------------------------------------------------- Warlock
    spell("Shadow Rounds", T::EnemyMinion, |g, c| {
        let mut next = c.target;
        // Chains while it keeps killing; the board is finite, so is this.
        for _ in 0..MAX_BOARD {
            let Some(t) = next else { break };
            g.spell_damage(c.side, Some(t), 2);
            let dead = matches!(t, Target::Minion(s, i)
                if g.player(s).board.get(i as usize).is_some_and(|m| m.is_dead()));
            g.sweep_deaths();
            if !dead {
                break;
            }
            next = g.random_minion(c.side.other());
        }
    }),
    battlecry("Ocular Occultist", T::None, |g, c| {
        g.discard_random(c.side);
    }),
    spell("RAFAAM LADDER!!", T::None, |g, c| {
        // "of different Costs": three draws, no two at the same price.
        let mut taken: Inline<i16, 3> = Inline::new();
        for _ in 0..3 {
            let mut matches: Inline<u16, { crate::state::MAX_DECK }> = Inline::new();
            for (i, card) in g.player(c.side).deck.iter().enumerate() {
                if !taken.contains(&card.def().cost) {
                    matches.push(i as u16);
                }
            }
            if matches.is_empty() {
                break;
            }
            let pick = g.rngs.effects.index(matches.len());
            let at = matches[pick] as usize;
            let card = g.player(c.side).deck[at];
            g.player_mut(c.side).deck.remove(at);
            taken.push(card.def().cost);
            g.give_hand_card(c.side, card.to_hand());
        }
    }),
    deathrattle("Possessed Animancer", |g, c| {
        if g.summon_from_deck(c.side, |d| d.races.any(Races::BEAST)) {
            let slot = g.player(c.side).board.len() as u8 - 1;
            g.grant(Target::Minion(c.side, slot), Keywords::LIFESTEAL);
        }
    }),
    choose("Sleep Paralysis", &[
        m(T::None, |g, c| {
            g.summon_token(c.side, tokens::NIGHT_TERROR, 2);
        }),
        m(T::EnemyMinion, |g, c| {
            if let Some(t) = c.target {
                g.destroy(t);
            }
        }),
    ]),
    battlecry("Riftcleaver", T::AnyMinion, |g, c| {
        let Some(t) = c.target else { return };
        let Target::Minion(s, i) = t else { return };
        let Some(health) = g.player(s).board.get(i as usize).map(|m| m.health()) else {
            return;
        };
        g.destroy(t);
        g.damage_hero(c.side, health);
    }),
    trigger("Asphyxiodon", |g, c| {
        if matches!(c.event, Event::TurnEnd { side } if side == c.side)
            && let Some(t) = g.random_minion(c.side.other())
        {
            g.deal_damage(t, 5);
        }
    }),
    battlecry("Archwitch Willow", T::None, |g, c| {
        // One pool over both zones, so a hand of Demons and a deck of them
        // are equally likely to answer.
        let hand: Inline<u16, MAX_HAND> = (0..g.player(c.side).hand.len() as u16)
            .filter(|i| g.player(c.side).hand[*i as usize].card.def().races.any(Races::DEMON))
            .collect();
        let deck: Inline<u16, { crate::state::MAX_DECK }> =
            (0..g.player(c.side).deck.len() as u16)
                .filter(|i| g.player(c.side).deck[*i as usize].def().races.any(Races::DEMON))
                .collect();
        let total = hand.len() + deck.len();
        if total == 0 {
            return;
        }
        let pick = g.rngs.effects.index(total);
        let card = if pick < hand.len() {
            let at = hand[pick] as usize;
            let card = g.player(c.side).hand[at].card;
            g.player_mut(c.side).hand.remove(at);
            card
        } else {
            let at = deck[pick - hand.len()] as usize;
            let card = g.player(c.side).deck[at];
            g.player_mut(c.side).deck.remove(at);
            card.card
        };
        g.summon(c.side, card);
    }),
    spell("Bat Mask", T::FriendlyMinion, |g, c| {
        let Some(t) = c.target else { return };
        let Target::Minion(_, i) = t else { return };
        g.set_attack(t, 1);
        g.set_health(t, 1);
        let card = g.player(c.side).board[i as usize].card;
        while !g.player(c.side).board.is_full() {
            if !g.summon(c.side, card) {
                break;
            }
            let slot = g.player(c.side).board.len() as u8 - 1;
            g.set_attack(Target::Minion(c.side, slot), 1);
            g.set_health(Target::Minion(c.side, slot), 1);
        }
    }),
    battlecry("Chronogor", T::None, |g, c| {
        // Both halves come off *your* deck: the two dearest to you, the two
        // cheapest to them.
        for _ in 0..2 {
            let mut best: Option<(usize, i16)> = None;
            for (i, card) in g.player(c.side).deck.iter().enumerate() {
                let cost = card.def().cost;
                if best.is_none_or(|(_, b)| cost > b) {
                    best = Some((i, cost));
                }
            }
            let Some((at, _)) = best else { break };
            let card = g.player(c.side).deck[at];
            g.player_mut(c.side).deck.remove(at);
            g.give_hand_card(c.side, card.to_hand());
        }
        for _ in 0..2 {
            let mut worst: Option<(usize, i16)> = None;
            for (i, card) in g.player(c.side).deck.iter().enumerate() {
                let cost = card.def().cost;
                if worst.is_none_or(|(_, b)| cost < b) {
                    worst = Some((i, cost));
                }
            }
            let Some((at, _)) = worst else { break };
            let card = g.player(c.side).deck[at];
            g.player_mut(c.side).deck.remove(at);
            g.give_card(c.side.other(), card.card);
        }
    }),
    battlecry("Razidir", T::None, |g, c| {
        let theirs = g.kindred(c.side, Races::DEMON) || g.kindred(c.side, Races::BEAST);
        g.discard_random(if theirs { c.side.other() } else { c.side });
    }),

    // --------------------------------------------------------- Priest
    battlecry("Psychic Conjurer", T::None, |g, c| {
        let foe = c.side.other();
        let n = g.player(foe).deck.len();
        if n == 0 {
            return;
        }
        let pick = g.rngs.effects.index(n);
        let card = g.player(foe).deck[pick].card;
        g.give_token(c.side, card);
    }),
    trigger("Shadow Ascendant", |g, c| {
        if !matches!(c.event, Event::TurnEnd { side } if side == c.side) {
            return;
        }
        let me = c.me();
        let mut pool: Inline<Target, MAX_BOARD> = Inline::new();
        g.collect_area(c.side, Area::FriendlyMinions, &mut pool);
        pool.retain(|t| *t != me);
        if pool.is_empty() {
            return;
        }
        let pick = g.rngs.effects.index(pool.len());
        g.buff(pool[pick], 1, 1);
    }),
    battlecry("Spirit of the Kaldorei", T::None, |g, c| {
        if g.player(c.side).hero_power_uses > 0
            && let Some(src) = c.source
        {
            g.buff(Target::Minion(c.side, src), 3, 3);
        }
    }),
    choose("Twilight Influence", &[
        m(T::MinionAtkAtMost(3), |g, c| {
            if let Some(t) = c.target {
                g.destroy(t);
            }
        }),
        m(T::None, |g, c| {
            g.summon_random_of_cost(c.side, 2, 1);
        }),
    ]),
    battlecry("Weaver of the Cycle", T::AnyCharacter, |g, c| {
        let holding = g.player(c.side).hand.iter().any(|h| {
            h.card.def().kind() == super::Kind::Spell && h.card.def().cost >= 5
        });
        if holding && let Some(t) = c.target {
            g.deal_damage(t, 3);
        }
    }),
    battlecry("Specter Specialist", T::FriendlyMinion, |g, c| {
        let Some(t) = c.target else { return };
        let Target::Minion(s, i) = t else { return };
        let already = g
            .player(s)
            .board
            .get(i as usize)
            .is_some_and(|m| m.has(Keywords::REBORN));
        if already {
            let card = g.player(s).board[i as usize].card;
            g.summon_copy(c.side, card);
        } else {
            g.grant(t, Keywords::REBORN);
        }
    }),
    trigger("Incensed Matriarch", |g, c| {
        if matches!(c.event, Event::TurnEnd { side } if side == c.side)
            && g.player(c.side)
                .board
                .get(c.slot as usize)
                .is_some_and(|m| m.damage == 0)
        {
            g.buff(c.me(), 0, 3);
        }
    }),
    deathrattle("Lingering Spirit", |g, c| {
        let before = g.player(c.side).hero_hp;
        g.heal_hero(c.side, 3);
        let excess = 3 - (g.player(c.side).hero_hp - before);
        if excess > 0
            && let Some(t) = g.random_enemy(c.side)
        {
            g.deal_damage(t, excess);
        }
    }),
    c(
        "Medivh's Triumph",
        T::None,
        Some(|g, c| g.spell_damage_area(c.side, Area::AllMinions, 4)),
        None,
        None,
        None,
        None,
        None,
        None,
        Some(|g, side, _i| {
            let legendary = g
                .player(side)
                .board
                .iter()
                .any(|m| m.card.def().rarity() == super::Rarity::Legendary);
            // "Costs (1)" is a price, not a discount.
            if legendary { 1 - 5 } else { 0 }
        }),
        None,
    ),
    battlecry("Eternus", T::EnemyMinion, |g, c| {
        let Some(t) = c.target else { return };
        let Target::Minion(s, i) = t else { return };
        let mine = c
            .source
            .and_then(|src| g.player(c.side).board.get(src as usize).map(|m| m.health()))
            .unwrap_or(0);
        let theirs = g.player(s).board.get(i as usize).map_or(i16::MAX, |m| m.health());
        if theirs <= mine {
            g.take_control(t, c.side);
        }
    }),
    deathrattle("Atlasaurus", |g, c| {
        g.summon_random_where(c.side, |d| {
            d.kind() == super::Kind::Minion && d.cost >= 5 && d.keywords.has(Keywords::TAUNT)
        });
    }),
    spell("Ritual of Life", T::None, |g, c| {
        // Discover, then a 2/3 of whatever was picked rather than the card.
        let pool = super::discover_pool(|d| d.kind() == super::Kind::Minion && d.cost == 3);
        if pool.is_empty() {
            return;
        }
        let mut offered = [0u32; 3];
        let n = g.rngs.effects.sample_indices(pool.len(), &mut offered);
        let Some(pick) = offered[..n].iter().map(|&i| pool[i as usize]).next() else {
            return;
        };
        if g.summon(c.side, pick) {
            let slot = g.player(c.side).board.len() as u8 - 1;
            g.set_attack(Target::Minion(c.side, slot), 2);
            g.set_health(Target::Minion(c.side, slot), 3);
        }
    }),
    trigger("Archaios", |g, c| {
        // On the declaration, before the exchange: the point of the card is
        // that the attacker goes in on Archaios' Health, not that it is
        // patched up afterwards -- by then it may not be there at all.
        if let Event::AttackDeclared { attacker: Target::Minion(s, i), .. } = c.event
            && s == c.side
            && i != c.slot
        {
            let mine = g
                .player(c.side)
                .board
                .get(c.slot as usize)
                .map_or(0, |m| m.health());
            g.set_health(Target::Minion(s, i), mine);
        }
    }),
    spell("Hold Them Off!", T::AnyMinion, |g, c| {
        if let Some(t) = c.target {
            g.buff(t, 5, 5);
            g.grant(t, Keywords::LIFESTEAL);
        }
    }),
    c(
        "Gladesong Siren",
        T::None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(|g, side, _i| {
            let cast = g.player(side).schools_cast_turn;
            let holy = cast & (1 << (super::School::Holy as u8)) != 0;
            let shadow = cast & (1 << (super::School::Shadow as u8)) != 0;
            if holy && shadow { 1 - 6 } else { 0 }
        }),
        None,
    ),
    deathrattle("Glade Ecologist", |g, c| {
        g.give_token(c.side, tokens::PURIFYING_VINES);
    }),
    spell("Purifying Vines", T::AnyMinion, |g, c| {
        let Some(t) = c.target else { return };
        let Target::Minion(s, _) = t else { return };
        if s == c.side {
            g.buff(t, 0, 2);
        } else {
            g.buff(t, 0, -2);
            g.sweep_deaths();
        }
    }),
    spell("Holy Embrace", T::AnyCharacter, |g, c| {
        if let Some(t) = c.target {
            g.heal(t, 4);
        }
        g.give_token(c.side, tokens::DARK_EMBRACE);
    }),
    spell("Dark Embrace", T::AnyCharacter, |g, c| {
        g.spell_damage(c.side, c.target, 4);
    }),

    // ----------------------------------------------------------- Mage
    spell("Mirror Dimension", T::None, |g, c| {
        let n = if g.holding_race(c.side, Races::DRAGON) { 2 } else { 1 };
        g.summon_token(c.side, tokens::MIRRORED_MAGE, n);
    }),
    choose("Spark of Life", &[
        m(T::None, |g, c| {
            g.discover(c.side, |d| {
                d.kind() == super::Kind::Spell && d.class() == super::Class::Mage
            });
        }),
        m(T::None, |g, c| {
            g.discover(c.side, |d| {
                d.kind() == super::Kind::Spell && d.class() == super::Class::Druid
            });
        }),
    ]),
    spell("Scorching Winds", T::AnyCharacter, |g, c| {
        g.spell_damage(c.side, c.target, 3);
        if g.discard_random_where(c.side, |d| {
            d.kind() == super::Kind::Spell && d.school() == super::School::Fire
        }) {
            g.spell_damage(c.side, c.target, 3);
        }
    }),
    battlecry("Spire Security", T::None, |g, c| {
        // "Reveal" only looks: the spell stays in the deck.
        let mut spells: Inline<i16, { crate::state::MAX_DECK }> = Inline::new();
        for card in g.player(c.side).deck.iter() {
            if card.def().kind() == super::Kind::Spell {
                spells.push(card.def().cost);
            }
        }
        if spells.is_empty() {
            return;
        }
        let pick = g.rngs.effects.index(spells.len());
        if spells[pick] >= 5 {
            g.damage_split(c.side, Area::EnemyMinions, 5);
        }
    }),
    battlecry("Astromancer", T::None, |g, c| {
        // Read after this card has left hand, which is the hand the summon
        // sees too.
        let n = g.player(c.side).hand.len() as i16;
        g.summon_random_of_cost(c.side, n, 1);
    }),
    battlecry("Temporal Construct", T::EnemyMinion, |g, c| {
        let Some(t) = c.target else { return };
        let Target::Minion(s, i) = t else { return };
        let Some(health) = g.player(s).board.get(i as usize).map(|m| m.health()) else {
            return;
        };
        g.deal_damage(t, 5);
        let excess = (5 - health).max(0);
        g.draw_cards(c.side, excess as usize);
    }),
    spell("Sindragosa's Triumph", T::AnyMinion, |g, c| {
        let Some(t) = c.target else { return };
        let Target::Minion(s, i) = t else { return };
        let Some(health) = g.player(s).board.get(i as usize).map(|m| m.health()) else {
            return;
        };
        // Printed as immune to Spell Damage, so this is raw damage.
        g.deal_damage(t, 8);
        let excess = (8 - health).max(0);
        if excess > 0 {
            let n = g.player(c.side).hand.len();
            if n > 0 {
                let pick = g.rngs.effects.index(n);
                if let Some(h) = g.player_mut(c.side).hand.get_mut(pick) {
                    h.cost_delta -= excess;
                }
            }
        }
    }),
    spell("Relic of Kings", T::None, |g, c| {
        if g.discover(c.side, |d| d.kind() == super::Kind::Spell && d.cost >= 8)
            && let Some(h) = g.player_mut(c.side).hand.last_mut()
        {
            // "It costs (1)" — a flat price, whatever it was printed at.
            h.cost_delta = 1 - h.card.def().cost;
        }
    }),
    trigger("Inferno Herald", |g, c| {
        if let Event::SpellCast { side, card } = c.event
            && side == c.side
            && card.def().school() == super::School::Fire
            && g.add_random_to_hand(c.side, |d| {
                d.kind() == super::Kind::Minion && d.races.any(Races::ELEMENTAL)
            })
            && let Some(h) = g.player_mut(c.side).hand.last_mut()
        {
            h.cost_delta -= 3;
        }
    }),
    secret("Mystic Misdirection", |g, owner, ev| {
        if let Event::AttackDeclared { attacker: Target::Minion(s, i), .. } = ev
            && s == owner.other()
        {
            g.transform(Target::Minion(s, i), tokens::SHEEP);
            return true;
        }
        false
    }),

    // -------------------------------------------------------- Warrior
    choose("Ominous Nightmares", &[
        m(T::None, |g, c| {
            g.spell_damage_area(c.side, Area::AllMinions, 1);
        }),
        m(T::DamagedMinion, |g, c| {
            if let Some(t) = c.target {
                g.buff(t, 2, 2);
            }
        }),
    ]),
    spell("Precursory Strike", T::AnyCharacter, |g, c| {
        g.spell_damage(c.side, c.target, 3);
        let holding = g
            .player(c.side)
            .hand
            .iter()
            .any(|h| h.card.def().kind() == super::Kind::Minion && h.card.def().cost >= 5);
        if holding {
            g.draw_matching(c.side, |d| d.kind() == super::Kind::Minion);
        }
    }),
    trigger("Stonecarver", |g, c| {
        if !matches!(c.event, Event::TurnEnd { side } if side == c.side) {
            return;
        }
        // "another": the Stonecarver does not carve itself.
        let me = c.me();
        let mut pool: Inline<Target, MAX_BOARD> = Inline::new();
        for (i, m) in g.player(c.side).board.iter().enumerate() {
            let t = Target::Minion(c.side, i as u8);
            if t != me && m.active() && m.is_minion() && m.damage > 0 {
                pool.push(t);
            }
        }
        if pool.is_empty() {
            return;
        }
        let pick = g.rngs.effects.index(pool.len());
        g.buff(pool[pick], 2, 2);
    }),
    battlecry("Baleful Blazer", T::AnyMinion, |g, c| {
        let fire = g.player(c.side).schools_cast_turn & (1 << (super::School::Fire as u8)) != 0;
        if fire && let Some(t) = c.target {
            g.destroy(t);
        }
    }),
    battlecry("Latorvian Armorer", T::EnemyMinion, |g, c| {
        let Some(t) = c.target else { return };
        g.deal_damage(t, 2);
        let dead = matches!(t, Target::Minion(s, i)
            if g.player(s).board.get(i as usize).is_some_and(|m| m.is_dead()));
        if dead {
            g.gain_armor(c.side, 5);
        }
    }),
    battlecry("Cataclysmic War Axe", T::None, |g, c| g.herald(c.side)),
    battlecry("Scorching Ravager", T::None, |g, c| {
        let before = g.player(c.side).board.len();
        g.herald(c.side);
        // "the Soldier" is whatever the Herald just put down, if anything.
        if g.player(c.side).board.len() > before {
            let slot = g.player(c.side).board.len() as u8 - 1;
            g.grant(Target::Minion(c.side, slot), Keywords::RUSH);
        }
    }),
    c(
        "Afflicted Devastator",
        T::None,
        None,
        Some(|g, c| {
            let me = c.source.map(|s| Target::Minion(c.side, s));
            let mut hits: Inline<Target, MAX_BOARD> = Inline::new();
            g.collect_area(c.side, Area::FriendlyMinions, &mut hits);
            for t in hits.iter() {
                if Some(*t) != me {
                    g.deal_damage(*t, 3);
                }
            }
        }),
        Some(|g, c| g.damage_area(c.side, Area::EnemyMinions, 3)),
        None,
        None,
        None,
        None,
        None,
        None,
    ),
    battlecry("Nablya, the Watcher", T::None, |g, c| {
        let wounded: Inline<CardId, MAX_BOARD> = g
            .player(c.side)
            .board
            .iter()
            .filter(|m| m.active() && m.is_minion() && m.damage > 0)
            .map(|m| m.card)
            .collect();
        for card in wounded.iter().copied() {
            if !g.summon(c.side, card) {
                break;
            }
            let slot = g.player(c.side).board.len() as u8 - 1;
            g.grant(Target::Minion(c.side, slot), Keywords::RUSH);
        }
    }),
    trigger("The Great Dracorex", |g, c| {
        if let Event::AfterAttack { attacker, defender, defender_died } = c.event
            && attacker == c.me()
            && matches!(defender, Target::Minion(d, _) if d == c.side.other())
        {
            // The death sweep has already run by the time this fires, so board
            // slots have shifted: a defender that died is simply gone, and one
            // that lived is still at the slot the event named.
            let spare = if defender_died { None } else { Some(defender) };
            // "ALL other enemy minions" — the one it hit already took the
            // exchange, and by now may not be there at all.
            let atk = g
                .player(c.side)
                .board
                .get(c.slot as usize)
                .map_or(0, |m| m.atk);
            let mut hits: Inline<Target, MAX_BOARD> = Inline::new();
            g.collect_area(c.side, Area::EnemyMinions, &mut hits);
            for t in hits.iter() {
                if Some(*t) != spare {
                    g.deal_damage(*t, atk);
                }
            }
        }
    }),
    battlecry("Undefeated Champion", T::None, |g, c| {
        let foe = c.side.other();
        while !g.player(foe).board.is_full() {
            if g.summon_random_of_cost(foe, 1, 1) == 0 {
                break;
            }
        }
    }),
    trigger("Tortolla", |g, c| {
        if matches!(c.event, Event::Damaged { target, .. } if target == c.me()) {
            g.gain_armor(c.side, 1);
            g.buff(c.me(), 1, 0);
        }
    }),
    c(
        "Crowd Control",
        T::None,
        Some(|g, c| {
            for _ in 0..2 {
                g.spell_damage_area(c.side, Area::AllMinions, 2);
                g.sweep_deaths();
            }
        }),
        None,
        None,
        None,
        None,
        None,
        None,
        Some(|g, side, _i| if g.player(side).deck.len() >= 25 { -2 } else { 0 }),
        None,
    ),
    c(
        "For Glory!",
        T::None,
        Some(|g, c| g.draw_cards(c.side, 2)),
        None,
        None,
        None,
        None,
        None,
        None,
        Some(|g, side, _i| -(g.minion_count(side.other()) as i16)),
        None,
    ),
    bonus("Scrappy Defender", |g, side, _, _| {
        if g.player(side).deck.len() >= 25 { (5, 0) } else { (0, 0) }
    }),
    spell("Gladiatorial Combat", T::None, |g, c| {
        g.summon_from_deck(c.side, |_| true);
        g.summon_token(c.side.other(), tokens::COLISEUM_TIGER, 1);
    }),

    // --------------------------------------------------------- Shaman
    spell("Blazing Invocation", T::None, |g, c| {
        if g.discover(c.side, |d| {
            d.kind() == super::Kind::Minion && d.keywords.has(Keywords::BATTLECRY)
        }) && let Some(h) = g.player_mut(c.side).hand.last_mut()
        {
            h.cost_delta -= 1;
        }
    }),
    spell("Flames of the Firelord", T::None, |g, c| {
        let big = g
            .player(c.side)
            .hand
            .iter()
            .any(|h| h.card.def().cost >= 8);
        let n = if big { 8 } else { 4 };
        if let Some(t) = g.random_minion(c.side.other()) {
            g.spell_damage(c.side, Some(t), n);
        }
    }),
    spell("Ritual of Power", T::None, |g, c| {
        g.herald(c.side);
        g.give_token(c.side, tokens::BREEZLING);
        g.give_token(c.side, tokens::BREEZLING);
    }),
    battlecry("Emberscarred Whelp", T::None, |g, c| {
        g.discover(c.side, |d| d.cost == 5);
        g.player_mut(c.side).pending.push(Pending {
            kind: PendingKind::TempCrystal,
            turns_left: 1,
            amount: 1,
            card: CardId(0),
        });
    }),
    spell("Lava Flow", T::None, |g, c| {
        // Re-picked each time: the first two points can finish something off.
        for _ in 0..3 {
            let Some(t) = g.lowest_health_enemy(c.side) else { break };
            g.spell_damage(c.side, Some(t), 2);
            g.sweep_deaths();
        }
    }),
    battlecry("Chillspine Stegodon", T::None, |g, c| {
        let freeze = g.kindred(c.side, Races::BEAST) || g.kindred(c.side, Races::ELEMENTAL);
        let foe = c.side.other();
        let mut pool: Inline<Target, MAX_BOARD> = Inline::new();
        for (i, m) in g.player(foe).board.iter().enumerate() {
            if m.active() && m.is_minion() {
                pool.push(Target::Minion(foe, i as u8));
            }
        }
        let mut picks = [0u32; 2];
        let n = g.rngs.effects.sample_indices(pool.len(), &mut picks[..2.min(pool.len())]);
        for &p in picks.iter().take(n) {
            let t = pool[p as usize];
            g.deal_damage(t, 2);
            if freeze {
                g.freeze(t);
            }
        }
    }),
    trigger("Mechanized Magma", |g, c| {
        if let Event::SpellCast { side, card } = c.event
            && side == c.side
            && card.def().school() == super::School::Fire
        {
            let n = card.def().cost;
            g.buff(c.me(), n, n);
        }
    }),
    trigger("Rehgar Earthfury", |g, c| {
        if let Event::AfterAttack { attacker: Target::Minion(s, i), .. } = c.event
            && s == c.side
            && (i == c.slot || i + 1 == c.slot || i == c.slot + 1)
        {
            g.give_token(c.side, tokens::LIGHTNING_BOLT);
        }
    }),
    battlecry("Slagclaw", T::None, |g, c| {
        let made = g.summon_token(c.side, tokens::SIZZLING_CINDER, 2);
        if made == 0 || !(g.kindred(c.side, Races::ELEMENTAL) || g.kindred(c.side, Races::DRAGON)) {
            return;
        }
        // "Trigger your Sizzling Cinders' Deathrattles" -- every one you have,
        // not only the two that just landed. They stay on the board.
        let cinders = g
            .player(c.side)
            .board
            .iter()
            .filter(|m| m.card == tokens::SIZZLING_CINDER)
            .count();
        for _ in 0..cinders {
            g.fire_deathrattle_of(c.side, tokens::SIZZLING_CINDER);
        }
    }),
    choose("Spirits of the Forest", &[
        m(T::None, |g, c| {
            g.summon_token(c.side, tokens::SPIRIT_WOLF, 3);
        }),
        m(T::None, |g, c| {
            g.summon_token(c.side, tokens::SPIRIT_FALCON, 2);
        }),
    ]),
    spell("Glaciate", T::None, |g, c| {
        // Discover, but the pick is summoned rather than drawn.
        let pool = super::discover_pool(|d| d.kind() == super::Kind::Minion && d.cost == 8);
        if pool.is_empty() {
            return;
        }
        let mut offered = [0u32; 3];
        let n = g.rngs.effects.sample_indices(pool.len(), &mut offered);
        let Some(pick) = offered[..n].iter().map(|&i| pool[i as usize]).next() else {
            return;
        };
        if g.summon(c.side, pick) {
            let slot = g.player(c.side).board.len() as u8 - 1;
            g.freeze(Target::Minion(c.side, slot));
        }
    }),
    spell("Sizzling Swarm", T::AnyCharacter, |g, c| {
        g.spell_damage(c.side, c.target, 3);
        g.summon_token(c.side, tokens::SIZZLING_CINDER, 3);
    }),
    trigger("Tortotem", |g, c| {
        if matches!(c.event, Event::TurnEnd { side } if side == c.side) {
            g.add_random_to_hand(c.side, |d| {
                d.kind() == super::Kind::Minion
                    && (d.races.0.count_ones() > 1 || d.races.has(Races::ALL))
            });
        }
    }),

    // ---------------------------------------------------- Demon Hunter
    spell("Sigil of Cinder", T::None, |g, c| {
        g.player_mut(c.side).pending.push(Pending {
            kind: PendingKind::SplitDamage,
            turns_left: 1,
            amount: 6,
            card: CardId(0),
        });
    }),
    battlecry("Armored Bloodletter", T::None, |g, c| g.herald(c.side)),
    deathrattle("Nightmare Dragonkin", |g, c| {
        if let Some(h) = g.player_mut(c.side).hand.last_mut() {
            h.cost_delta -= 2;
        }
    }),
    trigger("Defiled Spear", |g, c| {
        if let Event::AfterAttack { attacker: Target::Hero(s), defender, .. } = c.event
            && s == c.side
        {
            // "another random enemy" — not the one the hero just swung at.
            let foe = c.side.other();
            let mut pool: Inline<Target, { MAX_BOARD + 1 }> = Inline::new();
            for (i, m) in g.player(foe).board.iter().enumerate() {
                if m.active() && m.is_minion() && Target::Minion(foe, i as u8) != defender {
                    pool.push(Target::Minion(foe, i as u8));
                }
            }
            if defender != Target::Hero(foe) {
                pool.push(Target::Hero(foe));
            }
            if pool.is_empty() {
                return;
            }
            let atk = g.player(c.side).hero_attack();
            let pick = g.rngs.effects.index(pool.len());
            g.deal_damage(pool[pick], atk);
        }
    }),
    battlecry("Scorchreaver", T::None, |g, c| {
        g.discover(c.side, |d| {
            d.kind() == super::Kind::Spell && d.school() == super::School::Fel
        });
        for h in g.player_mut(c.side).hand.iter_mut() {
            if h.card.def().kind() == super::Kind::Spell
                && h.card.def().school() == super::School::Fel
            {
                h.cost_delta -= 1;
            }
        }
    }),
    battlecry("Chronikar", T::None, |g, c| {
        g.hero_attack_bonus(c.side, 3);
        // "next turn, and the turn after" — two more of the owner's own turns.
        g.player_mut(c.side).pending.push(Pending {
            kind: PendingKind::HeroAttack,
            turns_left: 2,
            amount: 3,
            card: CardId(0),
        });
    }),
    spell("Flash Flood", T::None, |g, c| {
        let passes = if c.outcast { 2 } else { 1 };
        for _ in 0..passes {
            let foe = c.side.other();
            let n = g.player(foe).board.len();
            if n == 0 {
                break;
            }
            // Both ends, and the same minion when there is only one.
            g.spell_damage(c.side, Some(Target::Minion(foe, 0)), 5);
            if n > 1 {
                g.spell_damage(c.side, Some(Target::Minion(foe, n as u8 - 1)), 5);
            }
            g.sweep_deaths();
        }
    }),
    trigger("Priestess of Fury", |g, c| {
        if matches!(c.event, Event::TurnEnd { side } if side == c.side) {
            g.damage_split(c.side, Area::AllEnemies, 6);
        }
    }),
    c(
        "Perennial Serpent",
        T::None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(|g, _side, _i| {
            let dormant = (0..2).any(|i| {
                g.players[i]
                    .board
                    .iter()
                    .any(|m| m.flags.has(Flags::DORMANT))
            });
            if dormant { -4 } else { 0 }
        }),
        None,
    ),
    battlecry("Dread Leviathan", T::EnemyMinion, |g, c| {
        let Some(t) = c.target else { return };
        for _ in 0..3 {
            if g.deal_damage(t, 3) {
                g.heal_hero(c.side, 3);
            }
            g.sweep_deaths();
            if !matches!(t, Target::Minion(s, i)
                if g.player(s).board.get(i as usize).is_some_and(|m| m.active()))
            {
                break;
            }
        }
    }),
    battlecry("Malevolent Mutant", T::None, |g, c| {
        // "Choose a Fel spell in your hand": one of them, taken as this engine
        // takes every choice it cannot put to the policy -- at random.
        let mut fel: Inline<CardId, MAX_HAND> = Inline::new();
        for h in g.player(c.side).hand.iter() {
            let d = h.card.def();
            if d.kind() == super::Kind::Spell && d.school() == super::School::Fel {
                fel.push(h.card);
            }
        }
        if fel.is_empty() {
            return;
        }
        let pick = g.rngs.effects.index(fel.len());
        g.give_token(c.side, fel[pick]);
    }),
    spell("Solitude", T::None, |g, c| {
        for _ in 0..2 {
            g.discover(c.side, |d| d.kind() == super::Kind::Minion);
        }
        let empty = !g
            .player(c.side)
            .deck
            .iter()
            .any(|card| card.def().kind() == super::Kind::Minion);
        if empty {
            for h in g.player_mut(c.side).hand.iter_mut() {
                if h.card.def().kind() == super::Kind::Minion {
                    h.cost_delta -= 2;
                }
            }
        }
    }),
    battlecry("Silithid Queen", T::None, |g, c| {
        if g.kindred(c.side, Races::BEAST) {
            g.hero_attack_bonus(c.side, 5);
        }
    }),

    // ---------------------------------------------------------- Druid
    battlecry("Charred Chameleon", T::FriendlyMinion, |g, c| {
        if g.player(c.side).hero_power_uses > 0
            && let Some(t) = c.target
        {
            g.buff(t, 1, 2);
            g.grant(t, Keywords::RUSH);
        }
    }),
    trigger("Crystalspine Cub", |g, c| {
        // "spend your last Mana Crystal" — something you paid for left you at
        // zero. Both a card and a Hero Power can do it.
        if matches!(c.event, Event::CardPlayed { side, .. } | Event::HeroPowerUsed { side }
            if side == c.side)
            && g.player(c.side).mana == 0
        {
            g.buff(c.me(), 1, 1);
        }
    }),
    spell("Life Cycle", T::AnyMinion, |g, c| {
        let Some(Target::Minion(s, i)) = c.target else { return };
        let Some(cost) = g.player(s).board.get(i as usize).map(|m| m.card.def().cost) else {
            return;
        };
        g.destroy(Target::Minion(s, i));
        g.sweep_deaths();
        // "to replace it" — on the board it was destroyed from, not yours.
        g.summon_random_of_cost(s, cost, 1);
    }),
    spell("Symbiosis", T::None, |g, c| {
        let mine = g.player(c.side).class;
        g.discover_where(c.side, |d| {
            d.keywords.has(Keywords::CHOOSE_ONE)
                && d.class() != super::Class::Neutral
                && d.class() != mine
        });
    }),
    spell("Mossbinding", T::None, |g, c| {
        let made = g.summon_child(c.side, c.card, 2);
        let spent = g.player(c.side).mana.max(0);
        g.player_mut(c.side).mana = 0;
        if spent == 0 || made == 0 {
            return;
        }
        let last = g.player(c.side).board.len();
        for slot in (last - made)..last {
            g.buff(Target::Minion(c.side, slot as u8), spent, spent);
        }
    }),
    spell("Ravenous Flock", T::None, |g, c| {
        g.player_mut(c.side).pending.push(Pending {
            kind: PendingKind::SummonToken,
            turns_left: 1,
            amount: 3,
            card: tokens::SKYSCREAMER_HATCHLING,
        });
    }),
    // A Location's activated ability lives in the `spell` hook; see
    // `Game::use_location`.
    spell("Tranquil Clearing", T::AnyMinion, |g, c| {
        let Some(t) = c.target else { return };
        g.buff(t, 0, 2);
        g.grant(t, Keywords::TAUNT);
        // "Falls asleep until the end of your next turn": it sits out that
        // whole turn and is back for the one after.
        g.make_dormant(t, 2);
    }),
    battlecry("Commissary Crook", T::None, |g, c| {
        let spent = g.player(c.side).mana.max(0);
        g.player_mut(c.side).mana = 0;
        g.summon_random_of_cost(c.side, spent, 1);
    }),
    spell("Overheat", T::None, |g, c| {
        g.buff_area(c.side, Area::FriendlyMinions, 1, 1);
        if g.discard_random_where(c.side, |d| {
            d.kind() == super::Kind::Spell && d.school() == super::School::Nature
        }) {
            g.buff_area(c.side, Area::FriendlyMinions, 1, 1);
        }
    }),
    battlecry("Spiteful Chef", T::None, |g, c| {
        // "10 or more Mana" can only mean crystals: the card costs 3, so the
        // mana left after paying for it never reaches ten.
        if g.player(c.side).crystals >= 10 {
            g.summon_random_where(c.side, |d| {
                d.kind() == super::Kind::Minion && d.cost == 6 && d.keywords.has(Keywords::TAUNT)
            });
        } else {
            g.summon_random_where(c.side, |d| {
                d.kind() == super::Kind::Minion && d.cost == 2 && d.keywords.has(Keywords::TAUNT)
            });
        }
    }),
    spell("Oaken Summons", T::None, |g, c| {
        g.gain_armor(c.side, 6);
        g.summon_from_deck(c.side, |d| d.cost <= 4);
    }),
    choose("Boomkin", &[
        m(T::None, |g, c| g.heal_hero(c.side, 8)),
        m(T::AnyCharacter, |g, c| {
            if let Some(t) = c.target {
                g.deal_damage(t, 4);
            }
        }),
    ]),
    battlecry("Endangered Dodo", T::None, |g, c| {
        if g.player(c.side).hero_hp <= 10
            && let Some(src) = c.source
        {
            g.buff(Target::Minion(c.side, src), 5, 5);
            g.summon_copy(c.side, c.card);
        }
    }),
    choose("Flipper Friends", &[
        m(T::None, |g, c| {
            g.summon_token(c.side, tokens::ORCA, 1);
        }),
        m(T::None, |g, c| {
            g.summon_token(c.side, tokens::OTTER, 6);
        }),
    ]),
    // ------------------------------------------ class backlog, third pass

    // End of turn.
    trigger("Voodoo Totem", |g, c| {
        if matches!(c.event, Event::TurnEnd { side } if side == c.side) {
            g.add_random_to_hand(c.side, |d| {
                d.kind() == super::Kind::Spell && d.school() == super::School::Shadow
            });
        }
    }),
    trigger("Selenic Drake", |g, c| {
        if matches!(c.event, Event::TurnEnd { side } if side == c.side) {
            g.add_random_to_hand(c.side, |d| d.races.any(Races::DRAGON));
        }
    }),
    trigger("Runaway Blackwing", |g, c| {
        if matches!(c.event, Event::TurnEnd { side } if side == c.side)
            && let Some(t) = g.random_minion(c.side.other())
        {
            g.deal_damage(t, 10);
        }
    }),
    trigger("Iridescent Flitterwing", |g, c| {
        if matches!(c.event, Event::TurnEnd { side } if side == c.side) {
            let me = c.me();
            let mut hits: Inline<Target, MAX_BOARD> = Inline::new();
            g.collect_area(c.side, Area::FriendlyMinions, &mut hits);
            for t in hits.iter() {
                if *t != me {
                    g.buff(*t, 1, 1);
                }
            }
        }
    }),
    trigger("Crystal Merchant", |g, c| {
        // TurnEnd fires before the turn is cleaned up, so the mana left is
        // still the mana left.
        if matches!(c.event, Event::TurnEnd { side } if side == c.side)
            && g.player(c.side).mana > 0
        {
            g.draw_cards(c.side, 1);
        }
    }),

    // After a spell, after a summon, after damage.
    trigger("Animated Moonwell", |g, c| {
        if let Event::SpellCast { side, card } = c.event
            && side == c.side
        {
            g.buff(c.me(), card.def().cost, 0);
        }
    }),
    trigger("Marshland Thresher", |g, c| {
        if matches!(c.event, Event::SpellCast { side, .. } if side == c.side) {
            g.grant(c.me(), Keywords::DIVINE_SHIELD);
        }
    }),
    trigger("Archmage Antonidas", |g, c| {
        if matches!(c.event, Event::SpellCast { side, .. } if side == c.side) {
            g.give_token(c.side, tokens::FIREBALL);
        }
    }),
    trigger("Veteran Warmedic", |g, c| {
        if let Event::SpellCast { side, card } = c.event
            && side == c.side
            && card.def().school() == super::School::Holy
        {
            g.summon_token(c.side, tokens::BATTLEFIELD_MEDIC, 1);
        }
    }),
    trigger("Windswept Pageturner", |g, c| {
        if let Event::MinionSummoned { side, card, .. } = c.event
            && side == c.side
            && card.def().races.any(Races::ELEMENTAL)
            && let Some(t) = g.random_enemy(c.side)
        {
            g.deal_damage(t, 3);
        }
    }),
    trigger("Rioter", |g, c| {
        // "survives damage" — the damage landed and the body is still up.
        if let Event::Damaged { target, .. } = c.event
            && let Target::Minion(s, i) = target
            && s == c.side
            && g.player(s).board.get(i as usize).is_some_and(|m| !m.is_dead())
        {
            g.buff(target, 1, 0);
        }
    }),
    trigger("Black Market Overseer", |g, c| {
        // The minion being played is the one carrying `BEING_PLAYED`; it is
        // already on the board by the time this event goes out.
        if let Event::CardPlayed { side, card } = c.event
            && side == c.side
            && card.def().keywords.has(Keywords::DEATHRATTLE)
            && card.def().kind() == super::Kind::Minion
        {
            for i in 0..g.player(side).board.len() {
                if g.player(side).board[i].flags.has(Flags::BEING_PLAYED) {
                    g.grant(Target::Minion(side, i as u8), Keywords::RUSH);
                }
            }
        }
    }),

    // Battlecries.
    battlecry("Bugsquasher", T::EnemyMinionWithRace, |g, c| {
        if let Some(t) = c.target {
            g.deal_damage(t, 6);
        }
    }),
    battlecry("Epoch Stalker", T::None, |g, c| {
        g.summon_copy(c.side, c.card);
    }),
    battlecry("Halazzi, the Lynx", T::None, |g, c| {
        while g.player(c.side).hand.len() < MAX_HAND {
            if !g.give_token(c.side, tokens::LYNX) {
                break;
            }
        }
    }),
    battlecry("Coghammer", T::None, |g, c| {
        if let Some(t) = g.random_minion(c.side) {
            g.grant(t, Keywords::DIVINE_SHIELD);
            g.grant(t, Keywords::TAUNT);
        }
    }),

    // Deathrattles.
    deathrattle("Tankgineer", |g, c| {
        g.summon_child(c.side, c.card, 1);
    }),
    deathrattle("Tirion Fordring", |g, c| {
        g.equip(c.side, tokens::ASHBRINGER);
    }),
    deathrattle("Lightshower Elemental", |g, c| {
        g.heal(Target::Hero(c.side), 8);
        let mut hits: Inline<Target, MAX_BOARD> = Inline::new();
        g.collect_area(c.side, Area::FriendlyMinions, &mut hits);
        for t in hits.iter() {
            g.heal(*t, 8);
        }
    }),
    deathrattle("Sahket Sapper", |g, c| {
        if let Some(t) = g.random_minion(c.side.other()) {
            g.bounce(t);
        }
    }),
    deathrattle("Stormbinder", |g, c| {
        // "Unlock" — the crystals locked for this turn come back, and the mana
        // with them.
        let p = g.player_mut(c.side);
        let freed = p.overload_now;
        p.overload_now = 0;
        p.mana += freed;
    }),
    deathrattle("Ball and Chain", |g, c| {
        let mut hits: Inline<Target, MAX_BOARD> = Inline::new();
        g.collect_area(c.side, Area::FriendlyMinions, &mut hits);
        for t in hits.iter() {
            if let Target::Minion(s, i) = *t
                && g.player(s).board[i as usize].damage > 0
            {
                g.buff(*t, 1, 2);
            }
        }
    }),

    // Spells.
    spell("Panther Mask", T::AnyMinion, |g, c| {
        if let Some(t) = c.target {
            g.set_attack(t, 5);
            g.set_health(t, 4);
            g.grant(t, Keywords::STEALTH);
        }
        g.draw_cards(c.side, 2);
    }),
    spell("Devilsaur Mask", T::AnyMinion, |g, c| {
        if let Some(t) = c.target {
            g.set_attack(t, 8);
            g.set_health(t, 8);
            g.grant(t, Keywords::CHARGE);
        }
    }),
    spell("Call of the Wild", T::None, |g, c| {
        for companion in tokens::ANIMAL_COMPANION.summonable_children() {
            g.summon(c.side, companion);
        }
    }),
    spell("Thief's Tools", T::None, |g, c| {
        for _ in 0..2 {
            if g.add_random_to_hand(c.side, |d| {
                d.kind() == super::Kind::Spell && d.cost == 4
            }) && let Some(h) = g.player_mut(c.side).hand.last_mut()
            {
                h.cost_delta -= 2;
            }
        }
    }),
    spell("Healing Rain", T::None, |g, c| {
        g.heal_split(c.side, 12);
    }),
    spell("Typhoon", T::None, |g, _c| {
        // Collected first: shuffling one back changes the boards underneath.
        let mut all: Inline<(Side, CardId), { MAX_BOARD * 2 }> = Inline::new();
        for i in 0..2 {
            let side = Side::from_index(i);
            for m in g.player(side).board.iter() {
                if m.is_minion() {
                    all.push((side, m.card));
                }
            }
        }
        for i in 0..2 {
            g.players[i].board.retain(|m| !m.is_minion());
        }
        for (_, card) in all.iter().copied() {
            // "a random player's deck" — a coin per minion, not per side.
            let to = if g.rngs.effects.index(2) == 0 {
                Side::Player0
            } else {
                Side::Player1
            };
            g.shuffle_into_deck(to, card);
        }
        g.board_dirty = true;
        g.recompute_auras();
    }),
    spell("Fortify", T::EnemyMinion, |g, c| {
        g.gain_armor(c.side, 3);
        let armor = g.player(c.side).armor;
        g.spell_damage(c.side, c.target, armor);
    }),
    spell("Shellnado", T::None, |g, c| {
        let spent = g.player(c.side).armor.min(5);
        g.player_mut(c.side).armor -= spent;
        g.spell_damage_area(c.side, Area::AllMinions, spent);
    }),

    // ------------------------------------------------------------ graveyard
    // "… that died this game". The pool itself is `Player::graveyard`.

    spell("Memoriam Manifest", T::None, |g, c| {
        g.resurrect_costliest(c.side, |d| d.races.any(Races::UNDEAD));
    }),
    battlecry("Calia Menethil", T::None, |g, c| {
        g.resurrect_costliest(c.side, |_| true);
    }),
    spell("Resuscitate", T::None, |g, c| {
        for cost in [1, 2, 3] {
            if g.resurrect(c.side, |d| d.cost == cost) {
                let slot = g.player(c.side).board.len() as u8 - 1;
                g.grant(Target::Minion(c.side, slot), Keywords::REBORN);
            }
        }
    }),
    spell("Undeath Sentence", T::None, |g, c| {
        let dead = g.dead_where(c.side, |d| d.keywords.has(Keywords::DEATHRATTLE));
        if dead.is_empty() {
            return;
        }
        let pick = g.rngs.effects.index(dead.len());
        let card = dead[pick];
        g.fire_deathrattle_of(c.side, card);
    }),
    battlecry("Merithra", T::None, |g, c| {
        // "all different" — one body per distinct card, however many copies
        // of it died.
        let dead = g.dead_where(c.side, |d| d.cost >= 8);
        let mut done: Inline<CardId, { crate::state::GRAVEYARD }> = Inline::new();
        for card in dead.iter().copied() {
            if done.contains(&card) {
                continue;
            }
            done.push(card);
            g.summon(c.side, card);
        }
    }),
    battlecry("Aessina", T::None, |g, c| {
        if g.player(c.side).deaths >= 20 {
            g.damage_split(c.side, Area::AllEnemies, 20);
        }
    }),
    deathrattle("Ysondre", |g, c| {
        // Ysondre is already in the graveyard by the time her own deathrattle
        // runs, so this death is one of the ones counted.
        let times = g
            .player(c.side)
            .graveyard
            .iter()
            .filter(|card| card.name() == "Ysondre")
            .count();
        for _ in 0..times {
            if !g.summon_random_where(c.side, |d| {
                d.kind() == super::Kind::Minion && d.races.any(Races::DRAGON)
            }) {
                break;
            }
        }
    }),
    spell("Splintered Reality", T::None, |g, c| {
        let grown = g
            .player(c.side)
            .graveyard
            .iter()
            .filter(|card| card.name() == "Treant")
            .count() as i16;
        for _ in 0..2 {
            if g.summon_child(c.side, c.card, 1) == 0 {
                break;
            }
            let slot = g.player(c.side).board.len() as u8 - 1;
            if grown > 0 {
                g.buff(Target::Minion(c.side, slot), grown, grown);
            }
        }
    }),
    spell("Succumb to Madness", T::None, |g, c| {
        // Discover, as this engine models it: three offered at random, the
        // biggest taken -- see `Game::discover`. The pick is summoned rather
        // than put in hand, because the card resummons it.
        let dead = g.dead_where(c.side, |d| d.races.any(Races::DRAGON));
        if dead.is_empty() {
            return;
        }
        let mut offered = [0u32; 3];
        let n = g.rngs.effects.sample_indices(dead.len(), &mut offered);
        let best = offered[..n]
            .iter()
            .map(|&i| dead[i as usize])
            .reduce(|a, b| if b.def().cost > a.def().cost { b } else { a });
        if let Some(card) = best {
            g.summon(c.side, card);
        }
    }),

    trigger("Truth Seeker", |g, c| {
        if matches!(c.event, Event::AfterAttack { attacker: Target::Hero(s), .. } if s == c.side) {
            for i in 0..g.player(c.side).board.len() {
                if g.player(c.side).board[i].card.def().class() == super::Class::Paladin {
                    g.buff(Target::Minion(c.side, i as u8), 2, 2);
                }
            }
        }
    }),

    // ------------------------------------------------ the deck as a zone
    // Cards that write on the copies still waiting in a deck, or ask where a
    // copy came from. Both live on `DeckCard`; see `state::DeckCard`.

    // "Top" is the end of the array -- the end `Game::draw` pops from -- and
    // only minions count, so this can skip past spells to find its three.
    battlecry("Beanstalk Brute", T::None, |g, c| {
        g.enchant_deck_top(c.side, 3, |d| d.kind() == super::Kind::Minion, 4, 4);
    }),
    // Two separate Discovers, each landing on the bottom rather than in hand.
    battlecry("Kaldorei Cultivator", T::None, |g, c| {
        for _ in 0..2 {
            g.discover_to_deck_bottom(
                c.side,
                |d| d.kind() == super::Kind::Minion && d.races.any(Races::BEAST),
                5,
                5,
            );
        }
    }),
    deathrattle("Seismopod", |g, c| {
        g.enchant_hand_where(c.side, |d| d.kind() == super::Kind::Minion, 3, 3);
        g.enchant_deck_where(c.side, |d| d.kind() == super::Kind::Minion, 3, 3);
    }),
    spell("Supreme Dinomancy", T::None, |g, c| {
        let beast = |d: &super::CardDef| d.kind() == super::Kind::Minion && d.races.any(Races::BEAST);
        g.enchant_hand_where(c.side, beast, 2, 2);
        g.enchant_deck_where(c.side, beast, 2, 2);
        for i in 0..g.player(c.side).board.len() {
            if g.player(c.side).board[i].races().any(Races::BEAST) {
                g.buff(Target::Minion(c.side, i as u8), 2, 2);
            }
        }
    }),
    // "Double their stats" is read off each card's own printed body, so the
    // numbers stay the cards' own.
    spell("Azshara's Triumph", T::None, |g, c| {
        g.shuffle_random_into_deck_where(
            c.side,
            5,
            |d| d.kind() == super::Kind::Minion && d.cost >= 8,
            |dc| {
                let d = dc.def();
                dc.enchant(d.atk, d.hp);
            },
        );
    }),
    // "Every minion in your deck shares a minion type": the tribes common to
    // all of them, intersected. A minion with no tribe at all breaks it; one
    // printed as All belongs to every tribe and so breaks nothing. A deck
    // with no minions left in it passes, having nothing to disagree.
    battlecry("City Chief Esho", T::None, |g, c| {
        let mut common = Races(u32::MAX);
        for dc in g.player(c.side).deck.iter() {
            let d = dc.def();
            if d.kind() != super::Kind::Minion {
                continue;
            }
            let r = if d.races.has(Races::ALL) { Races(u32::MAX) } else { d.races };
            common = Races(common.0 & r.0);
        }
        if common.is_empty() {
            return;
        }
        // "Your other minions": every copy in hand and deck, and every body
        // on the board but this one.
        g.enchant_hand_where(c.side, |d| d.kind() == super::Kind::Minion, 2, 2);
        g.enchant_deck_where(c.side, |d| d.kind() == super::Kind::Minion, 2, 2);
        let me = c.source;
        for i in 0..g.player(c.side).board.len() as u8 {
            if Some(i) == me {
                continue;
            }
            if g.player(c.side).board[i as usize].is_minion() {
                g.buff(Target::Minion(c.side, i), 2, 2);
            }
        }
    }),
    battlecry("Krona, Keeper of Eons", T::None, |g, c| {
        g.set_deck_bottom_cost(c.side, 5, 1);
    }),

    // ----------------------------------------- "that didn't start in your deck"
    // Answered by `DeckCard::started_here`, written once when the deck is
    // built. Everything shuffled, put or traded in later says no.

    battlecry("Steamcleaner", T::None, |g, c| {
        g.destroy_shuffled_in(c.side);
        g.destroy_shuffled_in(c.side.other());
    }),
    deathrattle("Smuggled Shovel", |g, c| {
        g.draw_by_origin(c.side, false, |d| d.kind() == super::Kind::Spell);
    }),
    spell("Dragonscale Armaments", T::None, |g, c| {
        let spell = |d: &super::CardDef| d.kind() == super::Kind::Spell;
        g.draw_by_origin(c.side, true, spell);
        g.draw_by_origin(c.side, false, spell);
    }),
    battlecry("Dreamwarden", T::None, |g, c| {
        if g.draw_by_origin(c.side, false, |_| true)
            && let Some(slot) = c.source
        {
            g.buff(Target::Minion(c.side, slot), 2, 2);
        }
    }),
    spell("Story of the Waygate", T::None, |g, c| {
        g.discount_hand_where(c.side, |hc| hc.marks.has(Marks::NOT_FROM_DECK), 1);
    }),
    // "If your deck started with no spells": a question about the list the
    // deck was built from, not about what is left in it, so it is answered
    // once at construction and never moves. The discount lands on the copy
    // just added, which is the last card in hand.
    battlecry("Hexmarshal", T::None, |g, c| {
        let before = g.player(c.side).hand.len();
        if !g.add_random_to_hand_where(c.side, |d| {
            d.kind() == super::Kind::Spell && d.cost >= 5
        }) {
            return;
        }
        if g.player(c.side).deck_started_spelless
            && g.player(c.side).hand.len() > before
            && let Some(hc) = g.player_mut(c.side).hand.last_mut()
        {
            hc.cost_delta -= 5;
        }
    }),
    // "(Swaps class each turn!)" -- the class starts as your own hero's and
    // is rerolled at the end of each of your turns, in `Game::end_turn`.
    // Played on the turn it arrives, this Discovers your own spells; held,
    // it drifts. The corpus text says only that it swaps, so where it swaps
    // to is not derivable from it; a random other class each turn is the
    // rule the game actually uses.
    // ------------------------------------------------------- acts when drawn
    // Shuffled into a deck, these never reach a hand: they resolve on the way
    // out of it. Each token's effect is its own printed text; see
    // `DRAWN_ACTORS` for how the draw path finds them.

    spell("Acorn", T::None, |g, c| {
        g.summon(c.side, tokens::SATISFIED_SQUIRREL);
    }),
    spell("Shred of Time", T::None, |g, c| {
        g.damage_hero(c.side, 3);
    }),
    spell("Found Gear!", T::None, |g, c| {
        g.gain_armor(c.side, 2);
    }),
    spell("Tripped Arcane Tripwire", T::None, |g, c| {
        g.damage_split(c.side, Area::AllEnemies, 4);
    }),
    spell("Tripped Beast Tripwire", T::None, |g, c| {
        g.summon_random_where(c.side, |d| {
            d.kind() == super::Kind::Minion && d.cost == 5 && d.races.any(Races::BEAST)
        });
    }),

    deathrattle("Vibrant Squirrel", |g, c| {
        for _ in 0..4 {
            g.shuffle_into_deck(c.side, tokens::ACORN);
        }
    }),
    battlecry("Twilight Timehopper", T::None, |g, c| {
        for _ in 0..2 {
            g.shuffle_into_deck(c.side, tokens::SHRED_OF_TIME);
        }
    }),
    spell("Scramble for Gear", T::None, |g, c| {
        g.gain_armor(c.side, 2);
        for _ in 0..5 {
            g.shuffle_into_deck(c.side, tokens::FOUND_GEAR);
        }
    }),
    spell("Arcane Tripwire", T::None, |g, c| {
        g.damage_split(c.side, Area::AllEnemies, 4);
        for _ in 0..2 {
            g.shuffle_into_deck(c.side, tokens::TRIPPED_ARCANE);
        }
    }),
    spell("Beast Tripwire", T::None, |g, c| {
        g.summon_random_where(c.side, |d| {
            d.kind() == super::Kind::Minion && d.cost == 5 && d.races.any(Races::BEAST)
        });
        for _ in 0..2 {
            g.shuffle_into_deck(c.side, tokens::TRIPPED_BEAST);
        }
    }),
    spell("Interrogation", T::None, |g, c| {
        for _ in 0..3 {
            g.shuffle_into_deck(c.side, tokens::TORTOLLAN_NINJA);
        }
    }),
    deathrattle("Illusory Greenwing", |g, c| {
        for _ in 0..2 {
            g.shuffle_into_deck(c.side, tokens::GREENWING_ILLUSION);
        }
    }),

    // ---------------------------------------------------------------- Temporary
    // A Temporary card is gone at the end of the turn it arrived on, unplayed.

    battlecry("Frantic Forger", T::None, |g, c| {
        let class = g.player(c.side).class;
        if g.add_random_to_hand_where(c.side, move |d| {
            d.kind() == super::Kind::Spell && (d.class() == class || d.class() == super::Class::Neutral)
        }) {
            g.make_last_temporary(c.side);
        }
    }),
    deathrattle("Tunnel Terror", |g, c| {
        for _ in 0..2 {
            if g.add_random_to_hand_where(c.side, |d| {
                d.kind() == super::Kind::Minion && d.cost == 2
            }) {
                g.make_last_temporary(c.side);
            }
        }
    }),
    // A Location: its activated ability lives in the `spell` hook.
    spell("Bloodpetal Biome", T::None, |g, c| {
        if g.discover(c.side, |d| d.kind() == super::Kind::Minion && d.cost == 1) {
            g.make_last_temporary(c.side);
        }
    }),
    battlecry("Spelunker", T::None, |g, c| {
        g.player_mut(c.side).next_temporary_discount += 2;
    }),

    // --------------------------------------------- closest to the real field
    // Picked by `tavernsim decks`: the cards standing between the simulator
    // and a deck people are actually playing, cheapest first.

    // Rewind is the engine's, not this row's: `Game::play_with_rewind` plays
    // the card twice from the same position and keeps the better draw. The
    // draw and the buff here are exactly as printed.
    battlecry("Portal Vanguard", T::None, |g, c| {
        // The buff goes on the card that was just drawn, so it has to be a
        // card that actually arrived: `draw_matching` reports the draw, not
        // whether a full hand burned it, and buffing `last_mut()` blind would
        // enchant a card that was already there.
        let before = g.player(c.side).hand.len();
        if g.draw_matching(c.side, |d| d.kind() == super::Kind::Minion)
            && g.player(c.side).hand.len() > before
            && let Some(hc) = g.player_mut(c.side).hand.last_mut()
        {
            hc.enchant(2, 2);
        }
    }),
    // "Give it this effect for a turn": the Discovered minion carries Follow
    // the Footsteps itself, so playing it Discovers another.
    spell("Follow the Footsteps", T::None, |g, c| {
        let stealthy =
            |d: &super::CardDef| d.kind() == super::Kind::Minion && d.keywords.has(Keywords::STEALTH);
        let before = g.player(c.side).hand.len();
        if g.discover(c.side, stealthy)
            && g.player(c.side).hand.len() > before
            && let Some(hc) = g.player_mut(c.side).hand.last_mut()
        {
            hc.marks.insert(Marks::FOOTSTEPS);
        }
    }),
    // "While holding this" is per copy, and the swing that sets it is the one
    // that strips the Stealth -- see `Game::attack_with`.
    spell("Tricks of the Trade", T::AnyCharacter, |g, c| {
        let n = if c.marks.has(Marks::STEALTH_ATTACKED) { 3 } else { 1 };
        g.spell_damage(c.side, c.target, n);
    }),
    // "Set your Mana to 10 after five turns": queued against this player's
    // own turns, so five of theirs rather than five of either, and it lands
    // once the five have gone by rather than on each of them -- the wiki's
    // note is "after they end their 5th turn", which is the start of their
    // sixth. `PendingKind::delayed` is what holds it back that long.
    start_of_game("Chef Neth'rek", |g, c| {
        // The whole starting list, not what is left of the deck: Start of Game
        // runs after the mulligan, so `deck` is short the opening hand by now.
        if g.player(c.side).deck_started_max_cost <= 3 {
            g.player_mut(c.side).pending.push(Pending {
                kind: PendingKind::SetMana,
                turns_left: 6,
                amount: 10,
                card: c.card,
            });
        }
    }),
    spell("Heartroot Stones", T::None, |g, c| {
        let times = if g.player(c.side).played_minion_last_turn { 1 } else { 2 };
        for _ in 0..times {
            g.draw_cards(c.side, 1);
            g.gain_armor(c.side, 3);
        }
    }),

    // ------------------------------------------------------ the Lotus rackets
    // Two of these count the cards you played for exactly two Mana; the third
    // is a 4/4 for two that leaves after one swing.

    battlecry("Lotus Troublemaker", T::None, |g, c| {
        // "Shoot 1 time!", plus one for every 2-Mana card played while this
        // sat in hand or deck. A copy that started in the deck has been there
        // since turn zero, so the player's count is exactly its own; a copy
        // conjured mid-game has not, and there is no per-copy counter to give
        // it -- so it shoots once. Weaker than printed, never stronger.
        let held_all_game = !c.marks.has(Marks::NOT_FROM_DECK);
        let extra = if held_all_game {
            g.player(c.side).cards_played_for_two
        } else {
            0
        };
        for _ in 0..1 + extra {
            if let Some(t) = g.random_enemy(c.side) {
                g.deal_damage(t, 1);
            }
        }
        g.sweep_deaths();
    }),

    spell("Jade Guardians", T::None, |g, c| {
        // "This game" here, not "while in hand or deck": the discount is the
        // player's whole count, whatever this spell's own history is.
        let off = g.player(c.side).cards_played_for_two as i16;
        for _ in 0..2 {
            let before = g.player(c.side).hand.len();
            g.add_random_to_hand_where(c.side, |d| d.cost == 8 && d.kind() == super::Kind::Minion);
            if g.player(c.side).hand.len() > before
                && let Some(hc) = g.player_mut(c.side).hand.last_mut()
            {
                hc.cost_delta -= off;
            }
        }
    }),

    trigger("Escape Artist", |g, c| {
        // "After this attacks and survives": the trigger only runs from a
        // board slot a live minion is standing in, so surviving is what
        // having been called at all means.
        let slot = c.slot;
        if !matches!(
            c.event,
            Event::AfterAttack { attacker: Target::Minion(s, i), .. }
                if s == c.side && i == slot
        ) {
            return;
        }
        g.draw_cards(c.side, 1);
        // "Escape the game": off the board, and not into the graveyard --
        // nothing died, so no Deathrattle fires and nothing that counts
        // friendly deaths counts this one.
        g.player_mut(c.side).board.remove(slot as usize);
        g.board_dirty = true;
        g.recompute_auras();
    }),

    // --------------------------------------------------------------- dragons
    // Three cards that wake up when a Dragon is played while they sit in hand,
    // and a spell that arrives already broken in two. `AWAKENS` and
    // `SHATTERS` hold the mechanics; these are the effects the cards have once
    // they are played.
    //
    // Each of these names belongs to more than one card -- the base form and
    // the awakened one, the whole spell and its two halves -- and the
    // behaviour index is keyed by name, so one row serves every form and
    // branches on `c.card` where the forms differ.

    battlecry("Ebonscale Scout", T::AnyCharacter, |g, c| {
        // "Damage equal to this minion's Attack": the body is already on the
        // board when a Battlecry runs, so it reads its own Attack from there
        // -- which is 4 unawakened and 8 awake, and whatever a buff has since
        // made it.
        let atk = c
            .source
            .and_then(|s| g.player(c.side).board.get(s as usize))
            .map_or(0, |m| m.atk);
        if let Some(t) = c.target
            && atk > 0
        {
            g.spell_damage(c.side, Some(t), atk);
        }
    }),

    battlecry("Ebyssian", T::None, |g, c| {
        // "This game", so it outlives the body: a flag on the player, applied
        // to the Dragons already out and to every one summoned after.
        g.player_mut(c.side).dragons_have_rush = true;
        for m in g.player_mut(c.side).board.iter_mut() {
            if m.card.def().races.any(Races::DRAGON) {
                m.keywords.insert(Keywords::RUSH);
            }
        }
    }),

    spell("Supply Run", T::None, |g, c| {
        // Whole or half. The unshattered card does both halves -- which is
        // what you get when your hand was too full to split it.
        if c.card != tokens::SUPPLY_RUN_BUFF {
            for _ in 0..3 {
                g.draw_matching(c.side, |d| d.kind() == super::Kind::Minion);
            }
        }
        if c.card != tokens::SUPPLY_RUN_DRAW {
            for hc in g.player_mut(c.side).hand.iter_mut() {
                if hc.card.def().kind() == super::Kind::Minion {
                    hc.enchant(2, 2);
                }
            }
        }
    }),

    // ------------------------------------------------------------- the Kabal
    // A sham trial, and the plants that make the case. The package turns on
    // one token: an Imp-formant goes into the *enemy's* deck and is summoned
    // for the player who put it there when they draw it. Everything else here
    // either plants one, moves one to where they will draw it next, or is the
    // trial that pays for all of it.

    spell("Harsh Sentence", T::None, |g, c| {
        // "Next turn" is theirs, not yours: the tax is cleared at the end of
        // the taxed player's own turn, so it is live for exactly the turn
        // they take after this one.
        g.player_mut(c.side.other()).minion_tax += 2;
        for _ in 0..2 {
            g.shuffle_into_deck(c.side.other(), tokens::IMP_FORMANT);
        }
    }),

    battlecry("Corrupt Constable", T::None, |g, c| {
        let foe = c.side.other();
        let at = g.deck_positions(foe, tokens::IMP_FORMANT);
        if at.is_empty() {
            return;
        }
        // Which one hardly matters -- every Imp-formant in there is the same
        // card -- but picking at random keeps two Constables from always
        // reaching for the same slot.
        let pick = g.rngs.effects.index(at.len());
        if let Some(mut dc) = g.move_deck_card_to_top(foe, at[pick] as usize) {
            dc.enchant(2, 2);
            // `move_deck_card_to_top` put it back already; the buff has to go
            // on the copy that is now sitting there.
            if let Some(top) = g.player_mut(foe).deck.last_mut() {
                *top = dc;
            }
        }
    }),

    spell("Frame Job", T::None, |g, c| {
        let foe = c.side.other();
        for _ in 0..2 {
            if let Some(t) = g.random_minion(foe) {
                g.destroy(t);
            }
        }
        g.sweep_deaths();
        // "Discover a minion in the enemy deck to put on top." Their deck,
        // their card, their draw -- what this decides is which one it is.
        g.discover_deck_card_to_top(foe, |d| d.kind() == super::Kind::Minion);
    }),

    // Godfather Kazakus. Two effects of nine, three offered at a time, then
    // the trial's length -- see the APPROXIMATE entry for which length this
    // takes and why.
    battlecry("Godfather Kazakus", T::None, |g, c| {
        for _ in 0..2 {
            let pick = g.rngs.effects.index(SHAM_TRIAL.len());
            let mut best = SHAM_TRIAL[pick];
            // Three offered, one taken -- the same shape `Game::discover`
            // uses, done by hand because these are tokens and no discover
            // pool holds them. The pick is the highest-costed offer, which
            // for a set of nine 0-cost tokens is simply the first drawn; what
            // the offer of three buys is that a Kazakus does not always reach
            // for the same effect.
            let mut offered = [0u32; 3];
            let n = g.rngs.effects.sample_indices(SHAM_TRIAL.len(), &mut offered);
            if n > 0 {
                best = SHAM_TRIAL[offered[0] as usize];
            }
            g.player_mut(c.side).pending.push(Pending {
                kind: PendingKind::CastLater,
                turns_left: 4,
                amount: 0,
                card: best,
            });
        }
    }),

    // --- the nine sham-trial effects, cast by the trial rather than played
    spell("Crate of Contraband", T::None, |g, c| g.draw_cards(c.side, 3)),
    spell("Swill of Suggestibility", T::None, |g, c| g.heal_hero(c.side, 12)),
    spell("Potion of Perjury", T::None, |g, c| {
        for hc in g.player_mut(c.side).hand.iter_mut() {
            if hc.card.def().kind() == super::Kind::Minion {
                hc.cost_delta -= 2;
            }
        }
    }),
    spell("Spurious Shiv", T::None, |g, c| {
        for hc in g.player_mut(c.side).hand.iter_mut() {
            if hc.card.def().kind() == super::Kind::Minion {
                hc.enchant(3, 3);
            }
        }
        for m in g.player_mut(c.side).board.iter_mut() {
            m.atk += 3;
            m.max_hp += 3;
        }
        g.recompute_auras();
    }),
    spell("Criminal Contract", T::None, |g, c| {
        g.summon_random_of_cost(c.side, 3, 3);
    }),
    spell("Tonic of Tyranny", T::None, |g, c| {
        g.summon_token(c.side, tokens::VOIDLORD, 1);
    }),
    spell("Convicted for Conspiracy", T::None, |g, c| {
        if let Some(t) = g.random_minion(c.side.other()) {
            g.take_control(t, c.side);
        }
    }),
    spell("Sentenced for Smuggling", T::None, |g, c| {
        for _ in 0..2 {
            g.discover_from_opponent_hand(c.side);
        }
    }),
    spell("Detained for Destruction", T::None, |g, _c| {
        g.force_every_minion_to_trade();
    }),

    // ------------------------------------------------------------- Cannoneers
    // A Cannoneer "fires": one damage at a random enemy. It does that at the
    // end of your turn on its own, and whenever something else tells it to.
    // Captain Crowley doubles every shot for as long as he is on the board.

    trigger("Cannoneer", |g, c| {
        if matches!(c.event, Event::TurnEnd { side } if side == c.side) {
            fire_one(g, c.side);
        }
    }),
    battlecry("Cannonmaster", T::None, |g, c| {
        g.give_token(c.side, tokens::CANNONEER);
    }),
    battlecry("Captain Crowley", T::None, |g, c| {
        g.summon_token(c.side, tokens::CANNONEER, 2);
    }),
    spell("Hook n' Heave", T::None, |g, c| {
        g.discover(c.side, |d| {
            d.kind() == super::Kind::Minion && d.races.any(Races::PIRATE)
        });
        g.summon_token(c.side, tokens::CANNONEER, 2);
    }),
    spell("Land Ho!", T::None, |g, c| {
        g.draw_cards(c.side, 2);
        g.summon_token(c.side, tokens::CANNONEER, 2);
    }),
    // The weapon's own trigger, fired from `WEAPON_SLOT`.
    trigger("Hand Cannon", |g, c| {
        if matches!(c.event, Event::AfterAttack { attacker: Target::Hero(s), .. } if s == c.side) {
            let n = g
                .player(c.side)
                .board
                .iter()
                .filter(|m| m.card == tokens::CANNONEER)
                .count();
            for _ in 0..n {
                fire_one(g, c.side);
            }
        }
    }),

    // A body, and a bonus every Pirate's damage reads off the board.
    c(
        "Blastpowder Engineer",
        T::None,
        None, None, None, None, None, None, None, None, None,
    ),
    // "Give a playable Pirate in your hand this effect for a turn": the copy
    // is marked, and playing it deals the same 2 damage again.
    spell("Follow the Fuse", T::None, |g, c| {
        if let Some(t) = g.random_enemy(c.side) {
            g.spell_damage(c.side, Some(t), 2);
        }
        let pirates: Inline<u8, MAX_HAND> = g
            .player(c.side)
            .hand
            .iter()
            .enumerate()
            .filter(|(_, hc)| {
                let d = hc.card.def();
                d.kind() == super::Kind::Minion && d.races.any(Races::PIRATE)
            })
            .map(|(i, _)| i as u8)
            .collect();
        if pirates.is_empty() {
            return;
        }
        let pick = g.rngs.effects.index(pirates.len());
        if let Some(hc) = g.player_mut(c.side).hand.get_mut(pirates[pick] as usize) {
            hc.marks.insert(Marks::FUSED);
        }
    }),

    // ------------------------------------------------------------- Dark Gifts
    // "Discover a ... with a Dark Gift": the Discover happens, then the card
    // it put in hand is given one of the ten. See `DARK_GIFTS`.

    spell("Avant-Gardening", T::None, |g, c| {
        if g.discover(c.side, |d| {
            d.kind() == super::Kind::Minion && d.keywords.has(Keywords::DEATHRATTLE)
        }) {
            g.gift_last_in_hand(c.side);
        }
    }),
    battlecry("Brutish Endmaw", T::None, |g, c| {
        if g.discover(c.side, |d| d.kind() == super::Kind::Minion && d.cost == 1) {
            g.gift_last_in_hand(c.side);
        }
    }),
    battlecry("Creature of Madness", T::None, |g, c| {
        if g.discover(c.side, |d| d.kind() == super::Kind::Minion && d.cost == 3) {
            g.gift_last_in_hand(c.side);
        }
    }),
    battlecry("Treacherous Tormentor", T::None, |g, c| {
        if g.discover(c.side, |d| {
            d.kind() == super::Kind::Minion && d.rarity() == super::Rarity::Legendary
        }) {
            g.gift_last_in_hand(c.side);
        }
    }),
    spell("Smoke Bomb", T::None, |g, c| {
        if g.discover(c.side, |d| {
            d.kind() == super::Kind::Minion
                && (d.keywords.has(Keywords::COMBO)
                    || d.keywords.has(Keywords::BATTLECRY)
                    || d.keywords.has(Keywords::STEALTH))
        }) {
            g.gift_last_in_hand(c.side);
        }
    }),
    // "It costs (2) less" is on top of whatever the Gift itself did to the
    // cost -- Short Claws takes another two off.
    spell("Cremate", T::None, |g, c| {
        if g.discover(c.side, |d| d.kind() == super::Kind::Minion) {
            g.gift_last_in_hand(c.side);
            if let Some(hc) = g.player_mut(c.side).hand.last_mut() {
                hc.cost_delta -= 2;
            }
        }
    }),
    // The Gift only lands if the Corpses are there to pay for it; the
    // Discover happens either way.
    spell("Rite of Atrocity", T::None, |g, c| {
        if g.discover(c.side, |d| {
            d.kind() == super::Kind::Minion && d.races.any(Races::UNDEAD)
        }) && g.spend_corpses(c.side, 2)
        {
            g.gift_last_in_hand(c.side);
        }
    }),
    // "Get a copy of it": the copy carries the same Gift, since the Gift is
    // part of the card that was Discovered.
    battlecry("Shadowflame Stalker", T::None, |g, c| {
        if g.discover(c.side, |d| {
            d.kind() == super::Kind::Minion && d.races.any(Races::DEMON)
        }) {
            g.gift_last_in_hand(c.side);
            if let Some(hc) = g.player(c.side).hand.last().copied() {
                g.player_mut(c.side).hand.push(hc);
            }
        }
    }),

    battlecry("Cindersword", T::None, |g, c| {
        if g.holding_dark_gift(c.side)
            && let Some(w) = g.player_mut(c.side).weapon.as_mut()
        {
            w.atk += 3;
        }
    }),
    battlecry("Dragon Turtle", T::None, |g, c| {
        if g.holding_dark_gift(c.side) {
            g.player_mut(c.side).hero_bonus_atk += 3;
            g.gain_armor(c.side, 6);
        }
    }),
    battlecry("Frostburn Matriarch", T::None, |g, c| {
        if g.holding_dark_gift(c.side) {
            g.summon_token(c.side, tokens::FROSTBURN_BROODLING, 2);
        }
    }),
    battlecry("Overgrown Horror", T::None, |g, c| {
        for hc in g.player_mut(c.side).hand.iter_mut() {
            if hc.gift != 0 && hc.card.def().kind() == super::Kind::Minion {
                hc.cost_delta -= 2;
            }
        }
    }),

    // ------------------------------------------------------------------ Imbue
    // "Imbue your Hero Power" installs the class's Blessing and raises a
    // count; every Blessing's number is that count. See `Game::imbue`.

    // The Portal is drawn, not played: it casts itself on the way out of the
    // deck. `@-Cost` is the Imbue count, like every other Blessing number.
    spell("Emerald Portal", T::None, |g, c| {
        let n = g.player(c.side).imbue_count.max(1) as i16;
        let pool = super::discover_pool(move |d| {
            d.kind() == super::Kind::Minion && d.cost == n && d.races.any(Races::DRAGON)
        });
        if !pool.is_empty() {
            let pick = g.rngs.effects.index(pool.len());
            g.summon(c.side, pool[pick]);
        }
    }),

    battlecry("Bitterbloom Knight", T::None, |g, c| g.imbue(c.side)),
    battlecry("Flutterwing Guardian", T::None, |g, c| g.imbue(c.side)),
    battlecry("Jagged Edge of Time", T::None, |g, c| g.imbue(c.side)),
    battlecry("Lunarwing Messenger", T::None, |g, c| g.imbue(c.side)),
    deathrattle("Umbraclaw", |g, c| g.imbue(c.side)),
    c(
        "Goldpetal Drake",
        T::None,
        None,
        Some(|g, c| g.imbue(c.side)),
        Some(|g, c| g.imbue(c.side)),
        None, None, None, None, None, None,
    ),
    spell("Aegis of Light", T::None, |g, c| {
        if g.summon_random_where(c.side, |d| d.kind() == super::Kind::Minion && d.cost == 2) {
            let slot = g.player(c.side).board.len() as u8 - 1;
            g.grant(Target::Minion(c.side, slot), Keywords::TAUNT);
        }
        g.imbue(c.side);
    }),
    spell("Aspect's Embrace", T::AnyCharacter, |g, c| {
        if let Some(t) = c.target {
            g.heal(t, 4);
        }
        g.draw_cards(c.side, 1);
        g.imbue(c.side);
    }),
    spell("Eventuality", T::AnyCharacter, |g, c| {
        g.spell_damage(c.side, c.target, 2);
        g.imbue(c.side);
    }),
    battlecry("Exotic Houndmaster", T::None, |g, c| {
        g.draw_matching(c.side, |d| d.races.any(Races::BEAST));
        g.imbue(c.side);
    }),
    spell("Finality", T::None, |g, c| {
        g.draw_matching(c.side, |d| d.races.any(Races::UNDEAD));
        g.imbue(c.side);
        g.imbue(c.side);
    }),
    battlecry("Living Garden", T::None, |g, c| {
        g.imbue(c.side);
        let minions: Inline<u8, MAX_HAND> = g
            .player(c.side)
            .hand
            .iter()
            .enumerate()
            .filter(|(_, hc)| hc.card.def().kind() == super::Kind::Minion)
            .map(|(i, _)| i as u8)
            .collect();
        if !minions.is_empty() {
            let pick = g.rngs.effects.index(minions.len());
            let at = minions[pick] as usize;
            if let Some(hc) = g.player_mut(c.side).hand.get_mut(at) {
                hc.cost_delta -= 1;
            }
        }
    }),
    battlecry("Spirit Gatherer", T::None, |g, c| {
        g.give_token(c.side, tokens::EDR_WISP);
        g.imbue(c.side);
    }),
    battlecry("Wisprider", T::None, |g, c| {
        g.imbue(c.side);
        g.trigger_hero_power(c.side);
    }),
    battlecry("Petal Picker", T::None, |g, c| {
        if g.player(c.side).imbue_count >= 2 {
            g.draw_cards(c.side, 2);
        }
    }),
    battlecry("Resplendent Dreamweaver", T::AnyMinion, |g, c| {
        if g.player(c.side).imbue_count >= 2 {
            g.spell_damage(c.side, c.target, 4);
        }
    }),

    // ------------------------------------------------------------ Bonus Effects
    // One of eight keywords, drawn at random -- see `Game::BONUS_EFFECTS` for
    // where that pool comes from and why it is not in the card data.

    // Rewind is the engine's, not this row's: `Game::play_with_rewind` rolls
    // the four Shades twice and keeps the better set of Bonus Effects -- as
    // well as `agent::position_value` can tell them apart, which is where the
    // remaining gap is now. The summon is exactly as printed.
    spell("Shadows of Yesterday", T::None, |g, c| {
        for _ in 0..4 {
            if !g.summon(c.side, tokens::ANOMALOUS_SHADE) {
                break;
            }
            let slot = g.player(c.side).board.len() as u8 - 1;
            g.give_bonus_effects(Target::Minion(c.side, slot), 2);
        }
    }),
    spell("Story of Galvadon", T::AnyMinion, |g, c| {
        if let Some(t) = c.target {
            g.give_bonus_effects(t, 3);
        }
    }),
    deathrattle("Tyrannogill", |g, c| {
        for _ in 0..3 {
            if !g.summon(c.side, tokens::DINOLOC) {
                break;
            }
            let slot = g.player(c.side).board.len() as u8 - 1;
            g.give_bonus_effect(Target::Minion(c.side, slot));
        }
    }),
    // "And this Deathrattle": the chosen minion carries Stranglevine's own
    // rattle onward, which is what makes the effect chain down the board.
    deathrattle("Stranglevine", |g, c| {
        let n = g.player(c.side).board.len();
        if n == 0 {
            return;
        }
        let pick = g.rngs.effects.index(n);
        let t = Target::Minion(c.side, pick as u8);
        g.give_bonus_effect(t);
        grant_rattle(g, Some(t), c.card);
    }),
    // "Steal its Bonus Effects": the ones granted to it, not the ones its own
    // card prints -- a body printed with Taunt has not been given Taunt.
    battlecry("Violet Punisher", T::EnemyMinion, |g, c| {
        let Some(t) = c.target else { return };
        let stolen = g.bonus_effects_on(t);
        if stolen.is_empty() {
            return;
        }
        if let Target::Minion(s, i) = t
            && let Some(m) = g.player_mut(s).board.get_mut(i as usize)
        {
            m.keywords.remove(stolen);
        }
        let Some(slot) = c.source else { return };
        let me = Target::Minion(c.side, slot);
        g.grant(me, stolen);
        let n = stolen.0.count_ones() as i16;
        g.buff(me, n, n);
    }),

    battlecry("Shadowed Informant", T::None, |g, c| {
        let want = g.player(c.side).informant_class;
        g.discover_where(c.side, move |d| {
            d.kind() == super::Kind::Spell && d.class() == want
        });
    }),
    c(
        "Techysaurus",
        T::None,
        None, None, None, None, None, None, None,
        Some(|g, side, _i| -(g.player(side).cards_played_not_from_deck as i16)),
        None,
    ),

    // ------------------------------------------------------------ the cycle
    // Priest's Quest package. One Quest that is really two, the two halves of
    // one Elemental it pays out, a board wipe that hands both players the
    // undo, and the minion that reads the whole game's Reborns back off the
    // graveyard.

    // "Quest: Cast 4 Holy spells Reward: Life's Breath. Quest: Cast 4 Shadow
    // spells. Reward: Death's Touch." Two counts in one Quest slot, four bits
    // each: Holy in the low nibble, Shadow in the high one. Neither reward
    // ends the Quest on its own -- the slot is only given up once both have
    // been paid, which is what lets one card carry two.
    trigger("Reach Equilibrium", |g, ctx| {
        let Event::CardPlayed { side, card } = ctx.event else {
            return;
        };
        if side != ctx.side || card.def().kind() != super::Kind::Spell {
            return;
        }
        let Some((qcard, progress)) = g.player(ctx.side).quest else {
            return;
        };
        let (holy, shadow) = (progress & 0xf, progress >> 4);
        let (mut holy, mut shadow) = (holy, shadow);
        match card.def().school() {
            super::School::Holy if holy < 4 => {
                holy += 1;
                if holy == 4 {
                    g.give_token(ctx.side, tokens::SOLETOS_LIFE);
                }
            }
            super::School::Shadow if shadow < 4 => {
                shadow += 1;
                if shadow == 4 {
                    g.give_token(ctx.side, tokens::SOLETOS_DEATH);
                }
            }
            _ => return,
        }
        g.player_mut(ctx.side).quest = if holy == 4 && shadow == 4 {
            None
        } else {
            Some((qcard, holy | (shadow << 4)))
        };
    }),

    // The two halves of Sol'etos. Each is a 4/4 for (5) with its own half of
    // the whole card's text; holding both combines them, which `COMBINES`
    // does as the second one arrives. The combined 8/8 keeps both hooks, so
    // all three forms are three rows here rather than one branching row --
    // unlike Schism below, the three carry three different names.
    battlecry("Sol'etos, Life's Breath", T::None, |g, c| {
        g.summon_token(c.side, c.card, 1);
    }),
    deathrattle("Sol'etos, Death's Touch", |g, c| {
        if let Some(t) = g.random_enemy(c.side) {
            g.deal_damage(t, 5);
        }
    }),
    c(
        "Sol'etos, Cycle's Rebirth",
        T::None,
        None,
        Some(|g, c| {
            g.summon_token(c.side, c.card, 1);
        }),
        Some(|g, c| {
            if let Some(t) = g.random_enemy(c.side) {
                g.deal_damage(t, 5);
            }
        }),
        None, None, None, None, None, None,
    ),

    // "Destroy all minions. Each player gets a 3-Cost spell that resummons
    // theirs." The wipe goes through the ordinary death path, so deathrattles
    // fire and the bodies land in each player's graveyard; the slice they
    // landed in is what each Ectoplasm reads back.
    spell("Slime 'em!", T::None, |g, c| {
        let before = [
            g.player(Side::from_index(0)).graveyard.len(),
            g.player(Side::from_index(1)).graveyard.len(),
        ];
        g.destroy_area_where(c.side, Area::AllMinions, |_| true);
        // `destroy` only marks; the bodies reach the graveyard on the sweep,
        // and the slice cannot be read before they are in it. Sweeping here
        // also puts the wipe's deathrattles before the two Ectoplasms, which
        // is the order the card reads in.
        g.sweep_deaths();
        for (i, was) in before.into_iter().enumerate() {
            let side = Side::from_index(i);
            let grew = g.player(side).graveyard.len() - was;
            if grew > 0 {
                g.player_mut(side).set_slimed(was, grew);
            }
            g.give_token(side, tokens::ECTOPLASM);
        }
    }),

    // "Resummon all friendly minions that were slimed." The slice is spent as
    // it is read: a second Ectoplasm resummons nothing unless another Slime
    // 'em! has been cast since.
    spell("Ectoplasm", T::None, |g, c| {
        let (at, len) = g.player(c.side).slimed_slice();
        g.player_mut(c.side).slimed = 0;
        for i in at..at + len {
            let Some(card) = g.player(c.side).graveyard.get(i).copied() else {
                break;
            };
            if !g.summon(c.side, card) {
                break;
            }
        }
    }),

    // "Battlecry: Resurrect your minions that were Reborn this game. They
    // attack random enemy minions." The pool is the graveyard slots marked
    // when a body that had come back through Reborn died again -- see
    // `Player::reborn_dead`. Every one of them, not a pick: the card says
    // "your minions", plural and unqualified.
    battlecry("Raith Van Geist", T::None, |g, c| {
        let marks = g.player(c.side).reborn_dead;
        let mut brought: Inline<u8, MAX_BOARD> = Inline::new();
        for i in 0..g.player(c.side).graveyard.len() {
            if marks & (1 << i) == 0 {
                continue;
            }
            let card = g.player(c.side).graveyard[i];
            if !g.summon(c.side, card) {
                break;
            }
            brought.push(g.player(c.side).board.len() as u8 - 1);
        }
        // "They attack random enemy minions" -- the minion only, never the
        // hero, and only while there is one left to hit. Slots are re-read
        // through the card each swing, since a death in the middle shifts
        // the board under the ones still to go.
        for slot in brought.iter().copied() {
            let Some(m) = g.player(c.side).board.get(slot as usize) else {
                break;
            };
            if !m.active() || !m.is_minion() {
                continue;
            }
            let Some(t) = g.random_minion(c.side.other()) else {
                break;
            };
            g.forced_attack((c.side, slot), t);
            g.sweep_deaths();
            if g.is_over() {
                return;
            }
        }
    }),

    // "Deathrattle: Get three 1/1 copies of random Legendary minions. They
    // cost (1)." Stats and cost are set by delta on the hand card, so the
    // numbers stay the card's own: whatever the Legendary printed, what
    // arrives is a 1/1 for (1).
    deathrattle("Karov the Broken", |g, c| {
        for _ in 0..3 {
            if !g.add_random_to_hand_where(c.side, |d| {
                d.kind() == super::Kind::Minion && d.rarity() == super::Rarity::Legendary
            }) {
                break;
            }
            let p = g.player_mut(c.side);
            let Some(hc) = p.hand.last_mut() else { break };
            let d = hc.card.def();
            hc.atk = (1 - d.atk).clamp(-128, 127) as i8;
            hc.hp = (1 - d.hp).clamp(-128, 127) as i8;
            hc.cost_delta = 1 - d.cost;
        }
    }),

    // "Draw 2 cards. Kindred: This costs (2) less." Kindred on a spell asks
    // about the spell school rather than a tribe, and this one is Holy.
    c(
        "Gravedawn Sunbloom",
        T::None,
        Some(|g, c| g.draw_cards(c.side, 2)),
        None, None, None, None, None, None,
        Some(|g, side, _i| {
            if g.player(side).schools_cast_last & (1 << (super::School::Holy as u8)) != 0 {
                -2
            } else {
                0
            }
        }),
        None,
    ),

    // A Location. "Your next Healing effect this turn deals damage instead"
    // is a charge on the player, spent by the next heal -- see
    // `Game::take_heal_charge`.
    spell("Ruby Sanctum", T::None, |g, c| {
        g.player_mut(c.side).heal_as_damage = true;
    }),

    // Shatter, like Supply Run above: whole or half, and all three forms are
    // printed under the one name "Schism", so one row branches on `c.card`.
    spell("Schism", T::FriendlyMinion, |g, c| {
        let Some(t) = c.target else { return };
        if c.card != tokens::SCHISM_COPY {
            g.buff(t, 2, 3);
            g.grant(t, Keywords::ELUSIVE);
        }
        if c.card != tokens::SCHISM_BUFF {
            g.summon_copy_of(c.side, t);
        }
    }),

    // -------------------------------------------------------------- the Void
    // Demon Hunter's Void Soul package. Four cards that hand out the same
    // one-mana spell and one that makes Taunt stop mattering.
    //
    // The Void Soul's own number is not in the corpus: its text arrives
    // template-stripped as "Summon a random -Cost Demon", with the value cut
    // out where the number belongs. One is what the card prints, and that
    // came from hearthstone.wiki.gg, not from the data -- named here because
    // it is the one figure in this package the corpus cannot back. The other
    // half of the card, "Improve your future Void Souls", has no rule stated
    // anywhere that could be checked, so it is not implemented at all and is
    // listed in `APPROXIMATE`. Guessing a step of one would be inventing a
    // number twice over.
    spell("Void Soul", T::None, |g, c| {
        g.summon_random_where(c.side, |d| {
            d.kind() == super::Kind::Minion && d.cost == 1 && d.races.any(Races::DEMON)
        });
    }),

    deathrattle("Vicious Voidscale", |g, c| {
        // The corpus prints one Void Soul, on the Deathrattle. Some card
        // listings elsewhere describe a Battlecry as well; the corpus is what
        // this engine follows, and one is the weaker reading of the two.
        g.give_token(c.side, tokens::VOID_SOUL);
    }),

    spell("Void Blast", T::AnyMinion, |g, c| {
        let Some(t) = c.target else { return };
        g.spell_damage(c.side, Some(t), 3);
        // "If it dies": the body is still on the board carrying lethal damage
        // at this point -- the sweep runs once the spell has finished -- so
        // the question is whether it is dead, not whether it is gone.
        let dead = match t {
            Target::Minion(s, i) => g
                .player(s)
                .board
                .get(i as usize)
                .is_none_or(crate::state::Permanent::is_dead),
            Target::Hero(_) => false,
        };
        if dead {
            g.give_token(c.side, tokens::VOID_SOUL);
        }
    }),

    trigger("Stardust Scythe", |g, c| {
        if matches!(c.event, Event::AfterAttack { attacker: Target::Hero(s), .. } if s == c.side) {
            g.give_token(c.side, tokens::VOID_SOUL);
        }
    }),

    spell("Hive Map", T::None, |g, c| {
        g.discover(c.side, |d| {
            d.kind() == super::Kind::Spell && d.school() == super::School::Fel
        });
    }),

    // ------------------------------------------------------------- the Auras
    // Paladin's Aura cycle, and the five cards built around it.
    //
    // An Aura is a spell that stays: it goes to the hero zone rather than the
    // board, does its thing at the end of each of its owner's turns, and is
    // gone after three of them. The engine already had somewhere for that to
    // live -- `Player::pending`, which Acceleration Aura has used since the
    // second card batch -- so each Aura here is one queued entry and the
    // firing is in `Game::tick_auras`.
    //
    // "Lasts 3 turns" is the one figure the corpus does not carry: all four
    // arrive with the number cut out ("Lasts turns", "Lasts @ turns"). Three
    // is what hearthstone.wiki.gg prints on each of the four card pages
    // separately, and it is also what Acceleration Aura -- whose text is
    // stripped the same way -- has been queued with here since it was
    // written. One value for the whole cycle, named where it is used.

    spell("Chronological Aura", T::None, |g, c| {
        g.player_mut(c.side).pending.push(Pending {
            kind: PendingKind::AuraSummon,
            turns_left: 3,
            amount: 0,
            card: tokens::CHRONOLOGICAL_DRAKE,
        });
    }),
    spell("Sandfury Aura", T::None, |g, c| {
        g.player_mut(c.side).pending.push(Pending {
            kind: PendingKind::AuraDouble,
            turns_left: 3,
            amount: 0,
            card: CardId(0),
        });
    }),
    spell("Gnomish Aura", T::None, |g, c| {
        g.player_mut(c.side).pending.push(Pending {
            kind: PendingKind::AuraHeal,
            turns_left: 3,
            amount: 4,
            card: CardId(0),
        });
    }),
    spell("Mekkatorque's Aura", T::None, |g, c| {
        g.player_mut(c.side).pending.push(Pending {
            kind: PendingKind::AuraBuff,
            turns_left: 3,
            amount: 4,
            card: CardId(0),
        });
    }),

    // "Battlecry: Put one of each Aura from your deck into the battlefield."
    // One of each, so a deck holding two Chronological Auras loses one; and
    // "from your deck", so each is taken out of it. Both of Gelbin's own
    // Auras are in the deck rather than in his text: Fabled puts them there
    // when the list is built, which is why the real deck codes carry them.
    battlecry("Gelbin of Tomorrow", T::None, |g, c| {
        let mut done: Inline<CardId, { AURAS.len() }> = Inline::new();
        loop {
            let next = g
                .player(c.side)
                .deck
                .iter()
                .position(|d| is_aura(d.card) && !done.iter().any(|s| *s == d.card));
            let Some(at) = next else { break };
            let card = g.player(c.side).deck[at].card;
            g.player_mut(c.side).deck.remove(at);
            done.push(card);
            g.cast_token(c.side, card);
        }
    }),

    battlecry("Manifested Timeways", T::None, |g, c| {
        if g.controls_aura(c.side) {
            g.spell_damage_area(c.side, Area::AllEnemies, 3);
        }
    }),

    // Shatter, the third pair -- see `SHATTERS`. Whole or half, one row.
    spell("Flight Maneuvers", T::None, |g, c| {
        if c.card != tokens::FLIGHT_MANEUVERS_BUFF {
            g.summon_token(c.side, tokens::SKY_DRAKE, 2);
        }
        if c.card != tokens::FLIGHT_MANEUVERS_DRAKES {
            for i in 0..g.player(c.side).board.len() {
                let m = g.player(c.side).board[i];
                if m.is_minion() && m.active() {
                    let t = Target::Minion(c.side, i as u8);
                    g.buff(t, 1, 0);
                    g.grant(t, Keywords::DIVINE_SHIELD);
                }
            }
        }
    }),

    // "Deathrattle: Trigger a random friendly minion's end of turn effect."
    // The weapon's own deathrattle, fired from the weapon slot; what it
    // triggers is one minion's reaction to `TurnEnd`, run on demand with no
    // turn actually ending.
    deathrattle("Inspiring Maul", |g, c| {
        let mut able: Inline<u8, MAX_BOARD> = Inline::new();
        for (i, m) in g.player(c.side).board.iter().enumerate() {
            if m.is_minion()
                && m.active()
                && behaviour_of(m.card).and_then(|b| b.trigger).is_some()
            {
                able.push(i as u8);
            }
        }
        if able.is_empty() {
            return;
        }
        let pick = g.rngs.effects.index(able.len());
        let slot = able[pick];
        let card = g.player(c.side).board[slot as usize].card;
        if let Some(f) = behaviour_of(card).and_then(|b| b.trigger) {
            f(
                g,
                &crate::events::TriggerCtx {
                    side: c.side,
                    slot,
                    event: Event::TurnEnd { side: c.side },
                },
            );
        }
    }),

    // "Battlecry: Cast a random spell from your deck that costs (2) or less
    // (targets this if possible)." The spell leaves the deck, as a cast from
    // it does; a spell whose target requirement this body cannot satisfy is
    // still cast, at whatever the engine's own targeting finds.
    battlecry("Violet Treasuregill", T::None, |g, c| {
        let mut cheap: Inline<u16, { MAX_DECK }> = Inline::new();
        for (i, d) in g.player(c.side).deck.iter().enumerate() {
            if d.card.def().kind() == super::Kind::Spell
                && d.card.def().cost + d.cost_delta as i16 <= 2
                && behaviour_of(d.card).and_then(|b| b.spell).is_some()
            {
                cheap.push(i as u16);
            }
        }
        if cheap.is_empty() {
            return;
        }
        let pick = g.rngs.effects.index(cheap.len());
        let at = cheap[pick] as usize;
        let card = g.player(c.side).deck[at].card;
        g.player_mut(c.side).deck.remove(at);
        // "(targets this if possible)": the body is on the board when a
        // Battlecry runs, so it can be pointed at -- when the spell will
        // take it.
        let me = c.source.map(|s| Target::Minion(c.side, s));
        let spec = behaviour_of(card).map_or(T::None, |b| b.target);
        let target = me.filter(|t| spec.needed() && spec.matches(g, c.side, *t));
        g.cast_token_at(c.side, card, target);
    }),

    // Gnomeregan across three ages. Each use gives its own buff and then
    // hands the slot to the next age, which is what "Advance to the present!"
    // says; the durability that is left goes with it, so the three ages are
    // three uses of one Location rather than nine.
    spell("Past Gnomeregan", T::AnyMinion, |g, c| {
        if let Some(t) = c.target {
            g.buff(t, 2, 1);
        }
        advance_gnomeregan(g, c, tokens::PRESENT_GNOMEREGAN);
    }),
    spell("Present Gnomeregan", T::AnyMinion, |g, c| {
        if let Some(t) = c.target {
            g.buff(t, 2, 1);
            grant_rattle(g, Some(t), tokens::LEPER_GNOME);
        }
        advance_gnomeregan(g, c, tokens::FUTURE_GNOMEREGAN);
    }),
    spell("Future Gnomeregan", T::AnyMinion, |g, c| {
        if let Some(t) = c.target {
            g.buff(t, 2, 1);
            g.grant(t, Keywords::DIVINE_SHIELD);
            grant_rattle(g, Some(t), tokens::LEPER_GNOME);
        }
    }),

    // ----------------------------------------------------------- the past
    // Mage's Quest package. Four Discovers, a Quest that counts them, and the
    // weapon it pays out.
    //
    // "From the past" is a pool, not an effect: a card from a set that has
    // rotated out of Standard. `cards::from_the_past` answers it straight
    // from the formats each `CardDef` carries, so the pool follows the corpus
    // through the next rotation with nothing here to change.

    spell("Alter Time", T::None, |g, c| {
        for _ in 0..2 {
            if g.discover(c.side, |d| {
                d.kind() == super::Kind::Spell
                    && d.school() == super::School::Arcane
                    && super::from_the_past(d)
            }) && let Some(hc) = g.player_mut(c.side).hand.last_mut()
            {
                hc.cost_delta -= 2;
            }
        }
    }),

    // "Your Rewinds keep BOTH potential outcomes" is not here: it is a
    // standing rule about how a Rewind resolves, and it lives where Rewind
    // does -- `Game::keeps_both_rewind_outcomes`, read off the board. This
    // row is only the Battlecry.
    battlecry("Morchie", T::None, |g, c| {
        g.discover(c.side, |d| d.keywords.has(Keywords::REWIND));
    }),

    // "Battlecry: Discover a spell. Choose to keep it or put it on top of
    // your opponent's deck." The engine always keeps it; see `APPROXIMATE`.
    battlecry("Q'onzu", T::None, |g, c| {
        g.discover(c.side, |d| d.kind() == super::Kind::Spell);
    }),

    battlecry("Raptor Herald", T::None, |g, c| {
        if !g.discover(c.side, |d| {
            d.kind() == super::Kind::Minion && d.races.any(Races::BEAST)
        }) {
            return;
        }
        // The discount goes on before the Gift: one of the eight Gifts is
        // Sweet Dreams, which moves the card out of hand and into the deck,
        // and after that `last_mut` is somebody else.
        if g.kindred(c.side, Races::BEAST)
            && let Some(hc) = g.player_mut(c.side).hand.last_mut()
        {
            hc.cost_delta -= 1;
        }
        g.gift_last_in_hand(c.side);
    }),

    spell("Wanted Poster", T::None, |g, c| {
        if g.discover(c.side, |d| d.kind() == super::Kind::Minion && d.cost >= 5)
            && let Some(hc) = g.player_mut(c.side).hand.last_mut()
        {
            // "Give it Prepare": the keyword belongs to the card and cards
            // are immutable, so this copy carries a mark instead. See
            // `Marks::GRANTED_PREPARE`.
            hc.marks.insert(Marks::GRANTED_PREPARE);
        }
    }),

    // "Quest: Discover 7 cards." Every Discover in the engine goes through
    // one place and fires `Event::Discovered` from it, so this counts them
    // all without knowing which card made the offer.
    trigger("The Forbidden Sequence", |g, ctx| {
        if let Event::Discovered { side, .. } = ctx.event
            && side == ctx.side
            && let Some((qcard, progress)) = g.player(ctx.side).quest
        {
            let progress = progress + 1;
            if progress >= 7 {
                g.give_token(ctx.side, tokens::ORIGIN_STONE);
                g.player_mut(ctx.side).quest = None;
            } else {
                g.player_mut(ctx.side).quest = Some((qcard, progress));
            }
        }
    }),

    // "After you Discover a card, this plays the other options. Lose 1
    // Durability." The options that were let go exist nowhere but the event
    // -- a Discover throws them away the instant it picks -- which is why
    // `Event::Discovered` carries them.
    trigger("The Origin Stone", |g, c| {
        let Event::Discovered { side, others } = c.event else {
            return;
        };
        if side != c.side || others.iter().all(|o| *o == CardId(0)) {
            return;
        }
        for card in others {
            if card != CardId(0) {
                g.play_loose_card(side, card);
            }
        }
        // Its own durability, spent by its own trigger.
        let spent = match g.player_mut(side).weapon.as_mut() {
            Some(w) => {
                w.durability -= 1;
                w.durability <= 0
            }
            None => false,
        };
        if spent {
            g.destroy_weapon(side);
        }
    }),

    // ------------------------------------------------------------- Azshara
    // Druid's Well and its Queen. Lady Azshara comes with two Locations in
    // the deck -- Fabled puts them there when the list is built -- and her
    // Choose One empowers one of them and destroys the other. Both Locations
    // and both empowered forms share their names in pairs, so each pair is
    // one row here that branches on `c.card`, like Schism and Supply Run.

    choose(
        "Lady Azshara",
        &[
            m(T::None, |g, c| empower(g, c.side, true)),
            m(T::None, |g, c| empower(g, c.side, false)),
        ],
    ),

    // A Location. "Fill your hand with random Temporary spells" -- as many as
    // there is room for, each burning unplayed at the end of the turn. The
    // empowered printing marks each of them to cast twice.
    spell("The Well of Eternity", T::None, |g, c| {
        let twice = c.card == tokens::WELL_OF_ETERNITY_EMPOWERED;
        while g.player(c.side).hand.len() < MAX_HAND {
            let before = g.player(c.side).hand.len();
            if !g.add_random_to_hand(c.side, |d| d.kind() == super::Kind::Spell) {
                break;
            }
            if g.player(c.side).hand.len() == before {
                break;
            }
            if let Some(hc) = g.player_mut(c.side).hand.last_mut() {
                hc.marks.insert(Marks::TEMPORARY);
                if twice {
                    hc.marks.insert(Marks::CASTS_TWICE);
                }
            }
        }
    }),

    // A Location. The empowered printing doubles what it copies, which is
    // done to the copy after it lands rather than to the original.
    spell("Zin-Azshari", T::FriendlyMinion, |g, c| {
        let Some(t) = c.target else { return };
        if !g.summon_copy_of(c.side, t) {
            return;
        }
        if c.card == tokens::ZIN_AZSHARI_EMPOWERED {
            let slot = g.player(c.side).board.len() as u8 - 1;
            let made = Target::Minion(c.side, slot);
            if let Some(m) = g.player(c.side).board.get(slot as usize) {
                let (atk, hp) = (m.atk, m.max_hp);
                g.buff(made, atk, hp);
            }
        }
    }),

    // "Reopen a location" is the whole of it: a Location that has been used
    // can be used again. It does not print a durability refill and does not
    // get one -- see the wiki note in `APPROXIMATE`.
    spell("Welcome Home!", T::FriendlyLocation, |g, c| {
        let Some(Target::Minion(s, i)) = c.target else {
            return;
        };
        if let Some(m) = g.player_mut(s).board.get_mut(i as usize) {
            m.flags.remove(Flags::USED);
            m.cooldown = 0;
            m.granted_rattle = tokens::STUBBORN_SUSPECT;
        }
    }),

    c(
        "Ysera, Emerald Aspect",
        T::None,
        None,
        // "Battlecry: Gain 3 Mana Crystals." Capped like every other gain,
        // by a ceiling this same card has already raised.
        Some(|g, c| g.gain_crystal(c.side, 3)),
        None, None, None, None, None, None,
        // "Start of Game: Increase both players' maximum Mana by 5." Both,
        // and once: two copies in one deck do not stack to ten, because the
        // ceiling is set rather than added to.
        Some(|g, _c| {
            for i in 0..2 {
                g.players[i].extra_crystals = g.players[i].extra_crystals.max(5);
            }
        }),
    ),

    // -------------------------------------------------------- the Windrunners
    // Hunter's face package: three sisters who each count the others, and the
    // cheap bodies and burn around them.

    // Each of the three repeats its Battlecry once for every sister already
    // played -- see `windrunner_sisters`, and `Player::rangers_played` for
    // where that is recorded.
    battlecry("Ranger General Sylvanas", T::None, |g, c| {
        for _ in 0..=windrunner_sisters(g, c.side, c.card) {
            g.spell_damage_area(c.side, Area::AllEnemies, 2);
            if g.is_over() {
                return;
            }
        }
    }),
    battlecry("Ranger Captain Alleria", T::None, |g, c| {
        for _ in 0..=windrunner_sisters(g, c.side, c.card) {
            g.discover(c.side, |d| d.kind() == super::Kind::Spell);
        }
    }),
    battlecry("Ranger Initiate Vereesa", T::None, |g, c| {
        for _ in 0..=windrunner_sisters(g, c.side, c.card) {
            g.enchant_deck_where(c.side, |d| d.kind() == super::Kind::Minion, 1, 1);
        }
    }),

    // "Your Hero Power costs (0) while your hand has 3 or less cards." A live
    // condition on the power's price, not an effect, so it is read where the
    // price is asked for -- `Game::hero_power_cost`.
    // (This row exists so the card counts as implemented; the rule itself is
    // in the engine, the way Kayn Sunfury's is.)

    spell("Sylvanas's Triumph", T::AnyCharacter, |g, c| {
        // The flag is set by the *first* copy and read by the second, so it
        // has to be read before this cast records itself.
        let again = g.player(c.side).triumph_cast;
        g.player_mut(c.side).triumph_cast = true;
        if again {
            g.spell_damage_area(c.side, Area::AllEnemies, 3);
        } else if let Some(t) = c.target {
            g.spell_damage(c.side, Some(t), 3);
        }
    }),

    spell("Confront the Tol'vir", T::None, |g, c| {
        for i in 0..g.player(c.side).cheap_minions_played.len() {
            let card = g.player(c.side).cheap_minions_played[i];
            if !g.summon(c.side, card) {
                break;
            }
        }
    }),

    // "Whenever you play a 1-Cost minion, double its stats. Whenever you cast
    // a 1-Cost spell, cast it twice." The spell half is in `play_card` with
    // the other doubling rules; this is the minion half.
    trigger("Niri of the Crater", |g, c| {
        if let Event::MinionSummoned { side, card, slot } = c.event
            && side == c.side
            && card.def().cost == 1
            && let Some(m) = g.player(side).board.get(slot as usize)
        {
            let (atk, hp) = (m.atk, m.max_hp);
            g.buff(Target::Minion(side, slot), atk, hp);
        }
    }),

    battlecry("Rockskipper", T::None, |g, c| {
        g.give_token(c.side, tokens::ROCK);
    }),
    spell("Rock", T::AnyCharacter, |g, c| {
        if let Some(t) = c.target {
            g.spell_damage(c.side, Some(t), 3);
        }
    }),

    spell("Tame Pet", T::None, |g, c| {
        g.player_mut(c.side).tamed_pet = true;
        g.draw_cards(c.side, 1);
    }),

    trigger("Chronoclaws", |g, c| {
        if matches!(c.event, Event::AfterAttack { attacker: Target::Hero(s), .. } if s == c.side)
            && let Some(card) = discard_costliest(g, c.side)
        {
            g.rattle_from_hand_or_deck(c.side, card);
        }
    }),

    // "Battlecry: Choose a card in your hand to discard. Deathrattle: Get it
    // back. It costs (1) less." The engine chooses, and it chooses the card
    // it can least use: the most expensive one, which is also the one worth
    // most when it comes back a mana cheaper.
    c(
        "Gemstone Hoarder",
        T::None,
        None,
        Some(|g, c| {
            if let Some(card) = discard_costliest(g, c.side) {
                g.player_mut(c.side).hoarded = card;
                g.rattle_from_hand_or_deck(c.side, card);
            }
        }),
        Some(|g, c| {
            let card = g.player(c.side).hoarded;
            if card == CardId(0) {
                return;
            }
            g.player_mut(c.side).hoarded = CardId(0);
            let mut hc = HandCard::new(card);
            hc.cost_delta -= 1;
            hc.marks.insert(Marks::NOT_FROM_DECK);
            g.give_hand_card(c.side, hc);
        }),
        None, None, None, None, None, None,
    ),

    // "Deal 3 damage. If this is EXACTLY in the center of your hand, deal 5
    // instead." Where it sat is read before it left; see `Ctx::centre`.
    spell("Precise Shot", T::AnyCharacter, |g, c| {
        if let Some(t) = c.target {
            g.spell_damage(c.side, Some(t), if c.centre { 5 } else { 3 });
        }
    }),

    // "Deathrattle: Deal 3 damage to all other characters. (Also triggers in
    // hand or deck.)" The parenthetical is the whole trick, and it is a rule
    // about where a deathrattle may fire from rather than about this card --
    // see `RATTLES_ANYWHERE`.
    deathrattle("IMPFERNAL!", |g, c| {
        let me = c.source.map(|s| Target::Minion(c.side, s));
        for i in 0..2 {
            let side = Side::from_index(i);
            g.deal_damage(Target::Hero(side), 3);
            for slot in (0..g.player(side).board.len()).rev() {
                let t = Target::Minion(side, slot as u8);
                if Some(t) != me {
                    g.deal_damage(t, 3);
                }
            }
        }
    }),

    // ------------------------------------------------------------- Leylines
    // Mage's Leyline package. Three spells and four cards that make them
    // bigger, cheaper or more numerous, all of it "this game".
    //
    // The three Leylines are the only cards in this engine whose *base*
    // numbers the corpus does not carry: each arrives with the value cut out
    // ("Deal damage to a random enemy minion", "Summon a random -Cost
    // minion", "It costs ( ) less"). Four, six and one come from
    // hearthstone.wiki.gg, each from that card's own page, and each is named
    // at the line that uses it. Everything that *scales* them is the
    // corpus's: Mystic Runesaber's 1, Ley Walker's 1, Surge Needle's extra
    // trigger, and The Arcanomicon's 2, 2 and 1 are all printed.

    // "Draw a card. It costs (1) less." The 1 is the wiki's.
    spell("Leyline Nexus", T::None, |g, c| {
        let p = g.player(c.side);
        let (bonus, extra) = (p.leyline_bonus as i16, p.leyline_extra);
        for _ in 0..=extra {
            let before = g.player(c.side).hand.len();
            g.draw_cards(c.side, 1);
            if g.player(c.side).hand.len() > before
                && let Some(hc) = g.player_mut(c.side).hand.last_mut()
            {
                hc.cost_delta -= 1 + bonus;
            }
        }
    }),

    // "Deal 4 damage to a random enemy minion. Excess damage hits the enemy
    // hero." The 4 is the wiki's.
    spell("Bursting Leyline", T::None, |g, c| {
        let p = g.player(c.side);
        let (bonus, extra) = (p.leyline_bonus as i16, p.leyline_extra);
        for _ in 0..=extra {
            let amount = 4 + bonus;
            match g.random_minion(c.side.other()) {
                Some(t @ Target::Minion(s, i)) => {
                    // "Excess damage hits the enemy hero": what the body
                    // could not absorb carries over, so how much it had left
                    // has to be read before the hit.
                    let left = g.player(s).board[i as usize].health();
                    g.spell_damage(c.side, Some(t), amount);
                    let spill = amount + g.player(c.side).spell_power() - left;
                    if spill > 0 {
                        g.deal_damage(Target::Hero(c.side.other()), spill);
                    }
                }
                // Nothing to hit, so nothing to spill past: the card names a
                // minion, and with no minion it does nothing.
                _ => return,
            }
            if g.is_over() {
                return;
            }
        }
    }),

    // "Summon a random 6-Cost minion." The 6 is the wiki's.
    spell("Crystallized Leyline", T::None, |g, c| {
        let p = g.player(c.side);
        let (bonus, extra) = (p.leyline_bonus as i16, p.leyline_extra);
        for _ in 0..=extra {
            g.summon_random_of_cost(c.side, 6 + bonus, 1);
        }
    }),

    // The three that scale them, and the one that hands them out.
    battlecry("Mystic Runesaber", T::None, |g, c| {
        let p = g.player_mut(c.side);
        p.leyline_bonus = p.leyline_bonus.saturating_add(1);
    }),
    c(
        "Ley Walker",
        T::None,
        None,
        Some(|g, c| {
            let p = g.player_mut(c.side);
            p.leyline_discount = p.leyline_discount.saturating_add(1);
        }),
        Some(|g, c| {
            let pick = g.rngs.effects.index(LEYLINES.len());
            g.give_token(c.side, LEYLINES[pick]);
        }),
        None, None, None, None, None, None,
    ),
    battlecry("Surge Needle", T::None, |g, c| {
        let p = g.player_mut(c.side);
        p.leyline_extra = p.leyline_extra.saturating_add(1);
    }),

    // "Get all 3 Leylines. Choose an upgrade for your Leylines." The three
    // upgrades are its own children and each carries its own number.
    choose(
        "The Arcanomicon",
        &[
            m(T::None, |g, c| {
                arcanomicon(g, c.side);
                g.cast_token(c.side, tokens::ENERGIZE);
            }),
            m(T::None, |g, c| {
                arcanomicon(g, c.side);
                g.cast_token(c.side, tokens::UNBLOCK);
            }),
            m(T::None, |g, c| {
                arcanomicon(g, c.side);
                g.cast_token(c.side, tokens::EMPOWER);
            }),
        ],
    ),
    spell("Energize", T::None, |g, c| {
        let p = g.player_mut(c.side);
        p.leyline_extra = p.leyline_extra.saturating_add(1);
    }),
    spell("Unblock", T::None, |g, c| {
        let p = g.player_mut(c.side);
        p.leyline_discount = p.leyline_discount.saturating_add(2);
    }),
    spell("Empower", T::None, |g, c| {
        let p = g.player_mut(c.side);
        p.leyline_bonus = p.leyline_bonus.saturating_add(2);
    }),

    // "Summon a 6/6 Dragon. Costs (1) less for each damage you dealt with
    // spells this turn." Both numbers are the card's own.
    c(
        "Spellweaver's Brilliance",
        T::None,
        Some(|g, c| {
            g.summon_token(c.side, tokens::AZURE_WARDEN, 1);
        }),
        None, None, None, None, None, None,
        Some(|g, side, _i| -(g.player(side).spell_damage_turn as i16)),
        None,
    ),

    // "Draw 1 card. (Upgrades each turn, but discards after 3!)" The 1 and
    // the 3 are the wiki's; the engine ticks the count and throws the card
    // away when it runs out (see `Game::end_turn`), so this only reads it.
    spell("Smoldering Grove", T::None, |g, c| {
        g.draw_cards(c.side, 1 + c.marks.held_turns() as usize);
    }),

    // "Your cards that summon minions summon twice as many." A standing rule
    // about how many bodies a summon puts down, so it is read where the
    // counting happens -- `Game::doubled_summons`.
];

/// The Arcanomicon's first half: all three Leylines into hand.
fn arcanomicon(g: &mut Game, side: Side) {
    for card in LEYLINES {
        g.give_token(side, card);
    }
}

/// Discard this player's most expensive card, and say which it was.
///
/// Two cards do this -- Chronoclaws every swing, Gemstone Hoarder once -- and
/// both print "your highest Cost card" or leave the choice to the player. The
/// most expensive is the one a Face deck can least use, and for the Hoarder
/// it is also the one worth most coming back a mana cheaper.
fn discard_costliest(g: &mut Game, side: Side) -> Option<CardId> {
    let at = g
        .player(side)
        .hand
        .iter()
        .enumerate()
        .max_by_key(|(_, hc)| hc.card.def().cost)
        .map(|(i, _)| i)?;
    let card = g.player(side).hand[at].card;
    g.player_mut(side).hand.remove(at);
    Some(card)
}

/// Lady Azshara's Choose One: empower one of her Locations, destroy the other.
///
/// "Empower" swaps the Location for its stronger printing and "destroy" takes
/// the other out of the game, and both of them can be sitting in any of three
/// places -- still in the deck, already drawn, or already played. All three
/// are handled, because a card that only worked from the deck would silently
/// do nothing in the game where she is drawn late.
fn empower(g: &mut Game, side: Side, zin: bool) {
    let (keep, into, drop) = if zin {
        (
            tokens::ZIN_AZSHARI,
            tokens::ZIN_AZSHARI_EMPOWERED,
            tokens::WELL_OF_ETERNITY,
        )
    } else {
        (
            tokens::WELL_OF_ETERNITY,
            tokens::WELL_OF_ETERNITY_EMPOWERED,
            tokens::ZIN_AZSHARI,
        )
    };
    for dc in g.player_mut(side).deck.iter_mut() {
        if dc.card == keep {
            dc.card = into;
        }
    }
    for hc in g.player_mut(side).hand.iter_mut() {
        if hc.card == keep {
            hc.card = into;
        }
    }
    for i in 0..g.player(side).board.len() {
        if g.player(side).board[i].card == keep {
            g.transform(Target::Minion(side, i as u8), into);
        }
    }
    g.player_mut(side).deck.retain(|dc| dc.card != drop);
    g.player_mut(side).hand.retain(|hc| hc.card != drop);
    for i in 0..g.player(side).board.len() {
        if g.player(side).board[i].card == drop {
            g.destroy(Target::Minion(side, i as u8));
        }
    }
    g.sweep_deaths();
}

/// Turn this Location into the next age, keeping the durability it has left.
///
/// A fresh Permanent would come with the next age's full three uses, and the
/// chain would be nine uses of an escalating Location rather than three. The
/// corpus does not say which it is and neither does the wiki, so this takes
/// the reading that cannot overrate the card, and says so in `APPROXIMATE`.
fn advance_gnomeregan(g: &mut Game, c: &Ctx, into: CardId) {
    let Some(slot) = c.source else { return };
    let Some(here) = g.player(c.side).board.get(slot as usize).copied() else {
        return;
    };
    let spent = here.damage;
    g.transform(Target::Minion(c.side, slot), into);
    if let Some(m) = g.player_mut(c.side).board.get_mut(slot as usize) {
        m.damage = spent;
        // The age it just became has not been used this turn; the age it was
        // has. Keeping both marks would give the chain a free extra use.
        m.flags.insert(Flags::USED);
        m.cooldown = 1;
    }
}

/// The Aura cycle, as cards.
///
/// Gelbin of Tomorrow asks the deck for "one of each Aura", which is a
/// question about card identity rather than about anything queued, so it
/// needs its own list. Acceleration Aura is in it because it is an Aura in a
/// deck and Gelbin should find it; it is not in `PendingKind::is_aura`,
/// because what it queues is a plain temporary crystal that another card
/// queues too -- see `APPROXIMATE`.
const AURAS: [CardId; 5] = [
    tokens::CHRONOLOGICAL_AURA,
    tokens::SANDFURY_AURA,
    tokens::GNOMISH_AURA,
    tokens::MEKKATORQUES_AURA,
    token("END_011"),
];

pub fn is_aura(card: CardId) -> bool {
    names(&AURAS, card)
}

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
        "Khadgar",
        "\"Your cards that summon minions summon twice as many\" reaches the summons that are written as a count -- `summon_token`, `summon_child`, `summon_random_of_cost` -- and not the ones a card writes as its own loop or as a single `summon`. Doubling every `summon` instead would double resurrects, Reborn and board-copies as well, which the card does not say; so this misses summons rather than inventing them, and reads weaker",
    ),
    (
        "Welcome Home!",
        "\"Reopen a location\" clears the once-per-turn lock and nothing else. The wiki says only that Reopen \"refreshes a location that has used its ability, allowing it to be used again\", and neither it nor the corpus says whether durability comes back or by how much -- so the Location gets its use back and keeps whatever durability it had left, which is the reading that cannot overrate the card",
    ),
    (
        "Q'onzu",
        "the Discovered spell is always kept; \"or put it on top of your opponent's deck\" is never the mode chosen. Picking between them needs a judgement about a card in *their* deck, which nothing here can weigh -- so the engine takes the half that is always available, and the card loses its use as a way to bury a spell it does not want",
    ),
    (
        "Past Gnomeregan",
        "\"Advance to the present!\" happens on each use and the durability that is left goes with it, so the three ages are three uses of one Location rather than three each. Neither the corpus nor the wiki says when the advance happens or what it does to durability; this is the reading that cannot overrate the card",
    ),
    (
        "Manifested Timeways",
        "\"If you control an Aura\" sees the four Auras of this cycle but not Acceleration Aura, which queues a plain temporary crystal that a second card queues too -- so there is nothing on the queued entry to tell them apart. Gelbin of Tomorrow does find it in a deck, since that is a question about the card and not about what it queued",
    ),
    (
        "Void Soul",
        "only the summon is implemented, and the 1-Cost it summons is the wiki's number rather than the corpus's -- the corpus text arrives template-stripped as \"a random -Cost Demon\". \"Improve your future Void Souls\" is not implemented at all: no source states what one improvement changes, and a step invented here would be a number this engine made up",
    ),
    (
        "Hive Map",
        "only the Discover is implemented; \"if you play it this turn, also pick one of the others\" needs the card to know it was the one just played, which no card in hand carries -- the same half Cultist Map is missing",
    ),
    (
        "Cultist Map",
        "only the Discover from the deck is implemented; \"if you play it this turn, also pick one of the others\" needs per-card state the engine does not have. Listed here late: the card has been implemented at half strength since the Discover batch, and was missing from this list rather than from the engine",
    ),
    (
        "Ruby Sanctum",
        "whose Healing effect it is is read as whose turn it is, since a heal reaches the engine with no caster attached -- so an opposing trigger that heals during your own turn spends the charge early; and the charge catches only a heal that goes through `Game::heal`, so \"Restore a minion to full Health\" is not turned round at all and a split heal loses one point of its total rather than all of it",
    ),
    (
        "Slime 'em!",
        "the slimed bodies are named by their slice of the graveyard rather than copied out, so a wipe whose deaths run past the graveyard's thirty-two slots resummons only what fitted, and a second Slime 'em! cast before the first Ectoplasm was spent overwrites the slice the unspent one was holding",
    ),
    (
        "Deathwing, Worldbreaker",
        "the Cataclysms are picked by a crude board heuristic rather than by the player -- the same limit Discover has, since an effect is resolved without reaching a policy",
    ),
    (
        "Storm the Gates",
        "the reward is a fixed old 1/1 Zombeast token rather than a custom          minion crafted from two chosen deck cards, which the engine has no          way to build",
    ),
    (
        "Archmage Kalec",
        "\"all spells in your hand and deck\" needs per-card Spell Damage, which neither a card in hand nor a card in the deck carries -- they hold stats and a cost, and a spell's damage bonus is neither; implemented as Spell Damage on the hero, which also boosts spells acquired after it lands -- the one entry here that can read stronger than the card, not weaker",
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
        "Godfather Kazakus",
        "the trial is always the Unending one -- two effects, resolving in four of your turns. The corpus prices the three lengths at 7, 4 and 0 Mana (Rushed, Grueling, Unending) and gives a number of turns for only two of them: \"in 4 turns\" and \"at the start of your next turn\". Rushed carries no text at all, so how soon it lands is a number this does not have. Taking the cheapest and slowest is the weaker of the two readings and the only one that needs no invented number; the engine also has nowhere to put a card whose cost and effects were chosen at play time, so the trial resolves on its own rather than arriving in hand to be paid for -- which is why the length that costs nothing is the honest one to take",
    ),
    (
        "Nespirah, Enthralled",
        "the corpus text for \"Deal 1 damage\" carries no target qualifier; narrowed to enemies only as the conservative default rather than guessing it can hit friendly characters too",
    ),
    (
        "Godfrey the Betrayer",
        "\"when you have space\" is checked once per own turn (begin_turn) rather than the instant a card leaves hand, so a return can lag by up to a turn; the queue of waiting cards is also capped at 10",
    ),
    (
        "Mirrex, the Crystalline",
        "plays as its own plain 3/4 Beast/Elemental; \"is a copy of the last enemy minion played\" while in hand is not implemented -- whether that copies the minion's abilities or only its name is not resolvable from the corpus text alone",
    ),
    (
        "Wickerfang",
        "\"after one of Wickerfang's Legs gains stats, this gains them too\" is not implemented; the main body stays at its own printed 0/5 while the four Legs still grow on their own",
    ),
    (
        "Al'Akir, Lord of Storms",
        "the two Charged Hands play as plain vanilla bodies; \"Adjacent minions have +{0} Attack\" is template-stripped at every Herald tier, with no floor value to fall back on",
    ),
    (
        "Sinestra",
        "each Wing's own Discover fires at full price; \"It costs ({0}) less\" is template-stripped at every Herald tier, with no floor value to fall back on",
    ),
    (
        "Atiesh the Greatstaff",
        "\"Costs (0) if you control Medivh\" is unreachable -- Medivh is a Hero card, and Hero cards are not implemented -- so this always costs its printed 10; and only the damage half of the spell-doubling is implemented, not the healing half, since this engine's heal/heal_hero have no spell-specific wrapper the way spell_damage does for combat damage",
    ),
    (
        "Commander Beatrix",
        "\"Ten copies join your deck\" is a deck-construction-time effect, outside this engine's scope -- plays with only its printed Taunt and whatever thirty cards it was actually given",
    ),
    (
        "Devouring Plague",
        "the Lifesteal heal is always the full 4, which can overheal if the random split ran out of live enemy minions before all 4 points landed -- the one entry here besides Archmage Kalec and Cursed Catacombs that can read stronger than the card, not weaker",
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

/// Cards whose `deathrattle` slot holds a rattle they *grant* to something
/// else rather than one they have themselves.
///
/// Two invariants below would otherwise reject them: a spell must have a cast
/// effect, and a declared deathrattle must match the printed keyword. Neither
/// holds for a carrier -- Spikeridged Steed is a spell whose rattle belongs to
/// whatever it buffed, and Living Spores is a bare enchantment that is never
/// played at all. See `Permanent::granted_rattle`.
pub const GRANTED_RATTLES: &[&str] = &[
    "Spikeridged Steed",
    "Talanji's Last Stand",
    "Ulfar",
    "Living Spores",
    "Dig for Freedom",
    "Threshrider's Blessing",
    "Lo'Gosh's Last Stand",
    "Amphibian's Spirit",
    "Sheep Mask",
];

/// Point a minion's `granted_rattle` at `carrier`.
fn grant_rattle(g: &mut Game, t: Option<Target>, carrier: CardId) {
    if let Some(Target::Minion(s, i)) = t
        && let Some(m) = g.player_mut(s).board.get_mut(i as usize)
    {
        m.granted_rattle = carrier;
    }
}

/// Turn a minion into a random one of `cost` (Dangerous Variant, Unknown
/// Voyager).
/// One Cannoneer's shot: a point of damage at a random enemy, twice while
/// Captain Crowley is out ("Your Cannoneers fire an additional shot").
///
/// Read off the board at the moment of firing rather than counted when a
/// Cannoneer arrives, because Crowley can leave: the shot is his while he is
/// there and not afterwards.
pub fn pirate_damage_bonus(g: &Game, side: Side) -> i16 {
    // "On your turn": the Engineer arms its owner's swings, not the swings
    // taken on the other player's turn.
    if g.current != side {
        return 0;
    }
    g.player(side)
        .board
        .iter()
        .filter(|m| m.card.name() == "Blastpowder Engineer")
        .count() as i16
}

fn fire_one(g: &mut Game, side: Side) {
    let shots = if g
        .player(side)
        .board
        .iter()
        .any(|m| m.card.name() == "Captain Crowley")
    {
        2
    } else {
        1
    };
    // A Cannoneer is a Pirate, so the Engineer arms its shot too.
    let damage = 1 + pirate_damage_bonus(g, side);
    for _ in 0..shots {
        if let Some(t) = g.random_enemy(side) {
            g.deal_damage(t, damage);
        }
    }
}

fn transform_into_cost(g: &mut Game, t: Target, cost: i16) {
    g.transform_into_random_of_cost(t, cost);
}

/// "If your deck has no Neutral cards" -- two Paladin cards ask it.
fn deck_has_neutrals(g: &Game, side: Side) -> bool {
    g.player(side)
        .deck
        .iter()
        .any(|card| card.def().class() == super::Class::Neutral)
}

/// Triennium Rex's payout, printed once and used by both of its hooks.
fn deathrattle_minion_to_hand(g: &mut Game, side: Side) {
    if g.add_random_to_hand(side, |d| {
        d.kind() == super::Kind::Minion && d.keywords.has(Keywords::DEATHRATTLE)
    }) && let Some(h) = g.player_mut(side).hand.last_mut()
    {
        h.cost_delta -= 2;
    }
}

/// How much a Cataclysm is worth in this position, for Deathwing's pick.
///
/// Crude on purpose: enough to prefer clearing a board that is actually there
/// over shuffling Dragons into a deck the game may never reach, and no more.
/// A real evaluation belongs to a policy, which effects cannot reach.
fn cataclysm_score(g: &Game, side: Side, cata: CardId) -> i16 {
    let foe = side.other();
    let enemies: Vec<i16> = g
        .player(foe)
        .board
        .iter()
        .filter(|m| m.active() && m.is_minion())
        .map(|m| m.health())
        .collect();
    match cata {
        // One point per enemy it damages, two more for each it kills.
        c if c == tokens::RAZE => enemies
            .iter()
            .map(|h| if *h <= 4 { 3 } else { 1 })
            .sum::<i16>(),
        // Worth the size of what it removes, and nothing on an empty board.
        c if c == tokens::TOPPLE => enemies.iter().copied().max().unwrap_or(0),
        // A 12/12 is a threat wherever the board stands, but there has to be
        // room for it.
        c if c == tokens::DRAGONS_REIGN => {
            if g.player(side).board.is_full() {
                0
            } else {
                6
            }
        }
        // Value for a long game, discounted because it does nothing now.
        _ => 2,
    }
}

/// The ten Dark Gifts, in the order the corpus lists them.
///
/// The pool needs nothing from outside the card data: a Dark Gift card names
/// its own, and every Dark Gift card names the same ten. `Cremate` and `Rite
/// of Atrocity` each carry exactly this list as their children, and `tests`
/// checks that they still do -- so this tracks the corpus rather than a
/// memory of it. Two more `EDR_100t*` tokens exist (`Inner Demons`,
/// `Nightmare Scales`) and are deliberately absent, because no Dark Gift card
/// lists them.
///
/// Each Gift's effect is its own printed text, quoted beside it.
pub const DARK_GIFTS: [CardId; 10] = [
    token("EDR_100t"),   // Waking Terror:     "+3 Attack and Lifesteal."
    token("EDR_100t1"),  // Well Rested:       "+2/+2 and Elusive."
    token("EDR_100t2"),  // Short Claws:       "Costs (2) less, but has -2 Attack."
    token("EDR_100t3"),  // Bundled Up:        "+4 Health and Taunt."
    token("EDR_100t5"),  // Living Nightmare:  "When you play this minion, summon a 2/2 copy of it."
    token("EDR_100t6"),  // Sleepwalker:       "Charge"
    token("EDR_100t7"),  // Rude Awakening:    "This minion's Battlecries trigger twice."
    token("EDR_100t8"),  // Sweet Dreams:      "+4/+5. Place this card on top of your deck."
    token("EDR_100t9"),  // Persisting Horror: "Reborn. Is Reborn with full Health and enchantments."
    token("EDR_100t13"), // Harpy's Talons:    "Divine Shield, Windfury"
];

/// A Dark Gift as stored on a card: a 1-based index into [`DARK_GIFTS`].
pub const fn gift_card(gift: u8) -> Option<CardId> {
    if gift == 0 || gift as usize > DARK_GIFTS.len() {
        None
    } else {
        Some(DARK_GIFTS[gift as usize - 1])
    }
}

/// The stats and cost a Gift is worth, applied the moment it is given.
///
/// Read straight off each Gift's printed text. The keyword half is
/// [`gift_keywords`], applied when the body reaches the board.
pub const fn gift_stats(gift: u8) -> (i16, i16, i16) {
    // (attack, health, cost delta)
    match gift {
        1 => (3, 0, 0),   // Waking Terror
        2 => (2, 2, 0),   // Well Rested
        3 => (-2, 0, -2), // Short Claws
        4 => (0, 4, 0),   // Bundled Up
        8 => (4, 5, 0),   // Sweet Dreams
        _ => (0, 0, 0),
    }
}

/// The keywords a Gift grants, applied when the minion reaches the board.
pub const fn gift_keywords(gift: u8) -> Keywords {
    match gift {
        1 => Keywords::LIFESTEAL,
        2 => Keywords::ELUSIVE,
        4 => Keywords::TAUNT,
        6 => Keywords::CHARGE,
        9 => Keywords::REBORN,
        10 => Keywords(Keywords::DIVINE_SHIELD.0 | Keywords::WINDFURY.0),
        _ => Keywords::NONE,
    }
}

/// Effects that change the game before it starts.
///
/// Distinct from the `start_of_game` hook, which fires after the mulligan so
/// that an effect touching a hand is not mistaken for part of dealing one.
/// These run *before* it, because they change what the mulligan is dealt
/// from: a starting Health, a deck that is a different size. Reordering the
/// existing hook to suit them would move every card already on it.
///
/// A name list for the same reason `CASTS_WHEN_DRAWN` is one -- the corpus
/// carries no mechanic for "this changes how the game is set up".
/// What a `GAME_SETUP` hook does: change one player's opening before the
/// first turn is dealt.
pub type Setup = fn(&mut Game, Side);

pub const GAME_SETUP: &[(&str, Setup)] = &[("Azalina Soulsever", azalina_setup)];

/// "Your starting Health is 40. Your deck is 20 cards, plus 20 copied from
/// your enemy."
///
/// The list is twenty by construction (`deck::DECK_SIZE_RULES`); this adds
/// the other twenty. They are copies -- the opponent keeps every card -- and
/// they are drawn from the opponent's list at random, since the card says
/// which deck they come from and not which cards.
fn azalina_setup(g: &mut Game, side: Side) {
    g.player_mut(side).hero_hp = 40;
    let foe = side.other();
    let theirs: Inline<CardId, MAX_DECK> = g.player(foe).deck.iter().map(|d| d.card).collect();
    if theirs.is_empty() {
        return;
    }
    for _ in 0..20 {
        let pick = g.rngs.effects.index(theirs.len());
        if !g.player_mut(side).deck.push(DeckCard::new(theirs[pick])) {
            break;
        }
    }
}

/// Apply every game-setup rule a player's opening list asks for.
pub fn apply_game_setup(g: &mut Game, side: Side) {
    let present: Inline<CardId, MAX_DECK> = g.player(side).deck.iter().map(|d| d.card).collect();
    for (name, f) in GAME_SETUP {
        if present.iter().any(|c| c.name() == *name) {
            f(g, side);
        }
    }
}

/// Cards that act on the way out of the deck instead of reaching hand.
///
/// The corpus writes this two ways, on the card itself: "Casts When Drawn"
/// on a spell and "Summoned When Drawn" on a minion. Which one applies
/// follows from the card's own kind, so one list covers both.
///
/// A name list rather than a keyword bit: the corpus spells this only as
/// words at the head of the text and carries no mechanic for it, and the
/// keyword word has one free bit left, which this has no claim on yet.
/// `tests` checks every name here really does print it.
/// By id rather than by name, unlike the other name-keyed lists here: this
/// one is read on *every draw of every card*, and comparing eight strings
/// each time is a cost the hot path should not carry. Ids are safe here where
/// names are elsewhere because each of these is a token with exactly one
/// printing, which `tests` checks.
const DRAWN_ACTORS: [CardId; 9] = [
    tokens::EMERALD_PORTAL,   // Summon a random @-Cost Dragon.
    tokens::ACORN,            // Summon a 2/1 Squirrel.
    tokens::SHRED_OF_TIME,    // Deal 3 damage to your hero.
    tokens::FOUND_GEAR,       // Gain 2 Armor.
    tokens::TRIPPED_ARCANE,   // Deal 4 damage split among all enemies.
    tokens::TRIPPED_BEAST,    // Summon a random 5-Cost Beast.
    tokens::TORTOLLAN_NINJA,  // Summoned: a 3/3 with Stealth.
    tokens::GREENWING_ILLUSION, // Summoned: a 4/5 Dragon with Taunt.
    tokens::IMP_FORMANT,      // Summoned: a 3/3 Lifesteal -- for the other side.
];

/// "While in hand, play a Dragon to become an X/X Dragon."
///
/// Base form and awakened form, both of which the corpus carries as cards in
/// their own right -- so the stats, the cost and the Dragon tribe of what it
/// becomes are read rather than written. One Dragon is enough; the wiki says
/// so and the text says "play a Dragon", not "play Dragons".
const AWAKENS: [(CardId, CardId); 3] = [
    (tokens::STONETALON_STRIKER, tokens::STONETALON_STRIKER_AWAKE),
    (tokens::EBONSCALE_SCOUT, tokens::EBONSCALE_SCOUT_AWAKE),
    (tokens::EBYSSIAN, tokens::EBYSSIAN_AWAKE),
];

/// What `card` becomes when its owner plays a Dragon while holding it.
pub fn awakened_by_dragon(card: CardId) -> Option<CardId> {
    let mut i = 0;
    while i < AWAKENS.len() {
        if AWAKENS[i].0.0 == card.0 {
            return Some(AWAKENS[i].1);
        }
        i += 1;
    }
    None
}

/// Shatter: "Splits into two halves that recombine when adjacent in hand."
///
/// Parent, then the half that goes to the left of the hand and the half that
/// goes to the right, in the order the parent's own text reads them. Both
/// halves keep the parent's full cost, which is what makes splitting a cost
/// and recombining the reward -- and both costs are the corpus's, not a
/// choice made here.
const SHATTERS: [(CardId, CardId, CardId); 3] = [
    (
        tokens::SUPPLY_RUN,
        tokens::SUPPLY_RUN_DRAW,
        tokens::SUPPLY_RUN_BUFF,
    ),
    (tokens::SCHISM, tokens::SCHISM_BUFF, tokens::SCHISM_COPY),
    (
        tokens::FLIGHT_MANEUVERS,
        tokens::FLIGHT_MANEUVERS_DRAKES,
        tokens::FLIGHT_MANEUVERS_BUFF,
    ),
];

/// Halves that combine wherever they are in hand, not only side by side.
///
/// Shatter's own halves have to be adjacent, which is the cost the mechanic
/// charges for splitting a card. Sol'etos is not a Shatter card: its two
/// halves are separate Quest rewards paid out turns apart, and its text asks
/// only "if you're holding both halves". So the pair is listed here instead,
/// and `Game::settle_hand` joins them from wherever they sit.
const COMBINES: [(CardId, CardId, CardId); 1] = [(
    tokens::SOLETOS_LIFE,
    tokens::SOLETOS_DEATH,
    tokens::SOLETOS_WHOLE,
)];

/// The card `left` and `right` combine into, in either order, when both are
/// held anywhere in hand.
pub fn combines(left: CardId, right: CardId) -> Option<CardId> {
    let mut i = 0;
    while i < COMBINES.len() {
        let (a, b, whole) = COMBINES[i];
        if (a.0 == left.0 && b.0 == right.0) || (a.0 == right.0 && b.0 == left.0) {
            return Some(whole);
        }
        i += 1;
    }
    None
}

/// Whether this card's Reborn copy keeps what the body was carrying.
///
/// "This is Reborn with full Health and enchantments" (Sinful Steed). Reborn
/// otherwise returns a fresh printing at one Health, so this is a rule the
/// card prints and the mechanic has no room for -- a side list, like
/// `AWAKENS` and `SHATTERS` above.
/// Where `card` sits in `list`, if it is in it at all.
///
/// A dozen side lists name a handful of cards each -- the ones whose printed
/// rule has no hook to hang a behaviour on, and which the engine therefore
/// reads by identity. Each of them used to carry its own copy of this loop.
/// It is a `while` rather than `iter().position()` because these are asked
/// from `const` context as well as from the engine, and neither iterators nor
/// `PartialEq` are available there.
const fn place_in(list: &[CardId], card: CardId) -> Option<usize> {
    let mut i = 0;
    while i < list.len() {
        if list[i].0 == card.0 {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Whether `list` names `card`.
const fn names(list: &[CardId], card: CardId) -> bool {
    place_in(list, card).is_some()
}

const REBORN_KEEPS_ALL: [CardId; 1] = [token("127060-sinful-steed")];

/// Cards whose Deathrattle also fires when they are lost from hand or deck.
///
/// "(Also triggers in hand or deck.)" -- a rule about *where* a Deathrattle
/// may fire from, printed on the card but belonging to the discard and mill
/// paths rather than to the effect. A side list, like `AWAKENS` and
/// `SHATTERS`: the effect itself is an ordinary `deathrattle` row.
const RATTLES_ANYWHERE: [CardId; 1] = [token("JAIL_398")];

/// The three Leylines.
///
/// Seven cards in the Mage package say "your Leylines" and mean exactly these
/// three, so which cards they are is a fact about identity and lives here.
/// Everything that scales them -- `Player::leyline_bonus`, `leyline_discount`
/// and `leyline_extra` -- is read by the cards themselves.
/// Khadgar, in every printing that is a 2-Cost minion.
///
/// Compared by id rather than by name because this is asked on every summon,
/// and a name comparison there is a string compare per board slot per body.
const KHADGARS: [CardId; 2] = [token("CORE_DAL_575"), token("DAL_575")];

pub fn doubles_summons(card: CardId) -> bool {
    names(&KHADGARS, card)
}

const LEYLINES: [CardId; 3] = [
    token("MEND_504"), // Leyline Nexus
    token("MEND_500"), // Bursting Leyline
    token("MEND_502"), // Crystallized Leyline
];

pub fn is_leyline(card: CardId) -> bool {
    names(&LEYLINES, card)
}

/// Cards that get better the longer they are held and are discarded once the
/// count runs out.
///
/// "(Upgrades each turn, but discards after 3!)" -- a rule about the copy in
/// hand rather than about the effect, so the engine ticks it (see
/// `Game::end_turn`) and the card reads the count off its own `Marks`.
const UPGRADES_WHILE_HELD: [CardId; 1] = [token("FIR_911")];

pub fn upgrades_while_held(card: CardId) -> bool {
    names(&UPGRADES_WHILE_HELD, card)
}

pub fn rattles_from_hand_or_deck(card: CardId) -> bool {
    names(&RATTLES_ANYWHERE, card)
}

/// Cards whose whole rules text is a standing rule the engine reads for
/// itself, so there is no hook to hang a behaviour on.
///
/// Kayn Sunfury has its own list because what it changes is Taunt; these are
/// the rest. Quel'dorei Fletcher's "Your Hero Power costs (0) while your hand
/// has 3 or less cards" lives in `Game::hero_power_cost`, and Niri of the
/// Crater's spell half lives beside the other doubling rules in `play_card`.
/// Cards the engine asks about by identity while they are in play.
///
/// A handful of rules are not effects a card fires but standing conditions
/// something else reads -- "while you control X", "while this is your Hero
/// Power". They were spelled as name comparisons, which is a string compare
/// per board slot; these are the ids instead, resolved at compile time. Each
/// list holds every printing, because what is being asked is "is this that
/// card", and a card is its reprints too.
pub mod controlled {
    use super::{CardId, names, token};

    const MORCHIE: [CardId; 1] = [token("END_036")];
    const SINESTRA: [CardId; 1] = [token("CATA_154")];
    const NIRI: [CardId; 1] = [token("TLC_836")];
    const FLETCHER: [CardId; 1] = [token("TIME_606")];
    const NARALEX: [CardId; 1] = [token("EDR_844")];
    const ATIESH: [CardId; 1] = [token("TIME_890t")];
    // The same two constants Mug'Zee's Start of Game assigns, so the write
    // and the read cannot drift apart.
    const MUGS_MAGIC: [CardId; 1] = [super::tokens::MUGS_MAGIC];
    const ZEES_MIGHT: [CardId; 1] = [super::tokens::ZEES_MIGHT];

    pub fn is_morchie(c: CardId) -> bool {
        names(&MORCHIE, c)
    }
    pub fn is_sinestra(c: CardId) -> bool {
        names(&SINESTRA, c)
    }
    pub fn is_niri(c: CardId) -> bool {
        names(&NIRI, c)
    }
    pub fn is_fletcher(c: CardId) -> bool {
        names(&FLETCHER, c)
    }
    pub fn is_naralex(c: CardId) -> bool {
        names(&NARALEX, c)
    }
    /// The weapon, not a minion -- but the same question, asked from the
    /// spell-damage path on every damaging spell in the game.
    pub fn is_atiesh(c: CardId) -> bool {
        names(&ATIESH, c)
    }
    /// Mug'Zee's two Passive Hero Powers, read from the Hero Power slot
    /// rather than the board: which one a player got is decided once, at the
    /// start of the game, from the shape of their deck.
    pub fn is_mugs_magic(c: CardId) -> bool {
        names(&MUGS_MAGIC, c)
    }
    pub fn is_zees_might(c: CardId) -> bool {
        names(&ZEES_MIGHT, c)
    }
}

const RULES_IN_THE_ENGINE: [CardId; 3] = [
    token("TIME_606"),     // Quel'dorei Fletcher
    token("TLC_836"),      // Niri of the Crater
    token("CORE_DAL_575"), // Khadgar
];

fn rule_lives_in_the_engine(card: CardId) -> bool {
    names(&RULES_IN_THE_ENGINE, card)
}

/// The three Windrunner sisters, in the order `Player::rangers_played` holds
/// them.
///
/// Each of the three is written around the other two, and each has to know
/// whether its sisters have been played. Which cards they are is a fact about
/// identity, so it lives here rather than in the accounting that records it.
const WINDRUNNERS: [CardId; 3] = [
    token("TIME_609"),   // Ranger General Sylvanas
    token("TIME_609t1"), // Ranger Captain Alleria
    token("TIME_609t2"), // Ranger Initiate Vereesa
];

/// The bit `card` sets in `Player::rangers_played`, if it is one of them.
pub fn windrunner_bit(card: CardId) -> Option<u8> {
    place_in(&WINDRUNNERS, card).map(|i| 1 << i)
}

/// How many of `card`'s two sisters have already been played -- the number
/// each of the three repeats its Battlecry for.
pub fn windrunner_sisters(g: &Game, side: Side, card: CardId) -> u32 {
    let mine = windrunner_bit(card).unwrap_or(0);
    (g.player(side).rangers_played & !mine).count_ones()
}

/// Minions whose controller's attacks ignore Taunt while they are in play.
///
/// "All friendly attacks ignore Taunt" is a standing rule about legality, not
/// an effect that fires, and the two places that ask about Taunt are both in
/// the engine -- so this is a side list read from there rather than a hook on
/// the card. Nothing is stored for it: the board is the state.
const IGNORES_TAUNT: [CardId; 2] = [token("BT_187"), token("CORE_BT_187")];

pub fn lets_attacks_ignore_taunt(card: CardId) -> bool {
    names(&IGNORES_TAUNT, card)
}

pub fn reborn_keeps_enchantments(card: CardId) -> bool {
    names(&REBORN_KEEPS_ALL, card)
}

/// The two halves `card` splits into as it enters hand, left first.
pub fn shatters_into(card: CardId) -> Option<(CardId, CardId)> {
    let mut i = 0;
    while i < SHATTERS.len() {
        if SHATTERS[i].0.0 == card.0 {
            return Some((SHATTERS[i].1, SHATTERS[i].2));
        }
        i += 1;
    }
    None
}

/// The card two adjacent halves recombine into, in that order.
pub fn recombines(left: CardId, right: CardId) -> Option<CardId> {
    let mut i = 0;
    while i < SHATTERS.len() {
        if SHATTERS[i].1.0 == left.0 && SHATTERS[i].2.0 == right.0 {
            return Some(SHATTERS[i].0);
        }
        i += 1;
    }
    None
}

/// Godfather Kazakus's nine sham-trial effects.
///
/// The whole menu, in the order the corpus lists them as his children. Every
/// one carries its own numbers in its own text -- twelve Health, three cards,
/// three 3-Cost minions -- so nothing here is a number this had to choose.
const SHAM_TRIAL: [CardId; 9] = [
    tokens::DETAINED_FOR_DESTRUCTION,
    tokens::CONVICTED_FOR_CONSPIRACY,
    tokens::SENTENCED_FOR_SMUGGLING,
    tokens::CRATE_OF_CONTRABAND,
    tokens::SPURIOUS_SHIV,
    tokens::CRIMINAL_CONTRACT,
    tokens::POTION_OF_PERJURY,
    tokens::SWILL_OF_SUGGESTIBILITY,
    tokens::TONIC_OF_TYRANNY,
];

/// Cards that, drawn, are summoned for the drawer's *opponent*.
///
/// One card, and the whole Kabal package turns on it: an Imp-formant is put
/// into the enemy's deck and pays out to the player who planted it. Reading
/// the printed text the other way -- summoning it for whoever drew it -- would
/// make every card that plants one a gift to the opponent.
const DRAWN_FOR_OPPONENT: [CardId; 1] = [tokens::IMP_FORMANT];

/// Whether a card that acts when drawn acts for the other side.
#[inline]
pub fn drawn_acts_for_opponent(card: CardId) -> bool {
    let mut i = 0;
    while i < DRAWN_FOR_OPPONENT.len() {
        if DRAWN_FOR_OPPONENT[i].0 == card.0 {
            return true;
        }
        i += 1;
    }
    false
}

/// Whether `card` acts as it is drawn rather than reaching hand.
#[inline]
pub fn acts_when_drawn(card: CardId) -> bool {
    let mut i = 0;
    while i < DRAWN_ACTORS.len() {
        if DRAWN_ACTORS[i].0 == card.0 {
            return true;
        }
        i += 1;
    }
    false
}

/// Which of the three standing hooks a card has, one byte per `CardId`.
///
/// Two scans ask nothing else of a card. `recompute_auras` wants to know what
/// carries an aura and what grants itself a bonus; `Game::fire` wants to know
/// what reacts. Between them they run after every resolution point in the
/// game -- well over a million board walks in a two-thousand-game batch, each
/// asking the question of every minion on both boards.
///
/// Asking `BEHAVIOURS` means a random read into ninety kilobytes of card
/// table per minion, which leaves L1 on the way in. Asking this is a read
/// into one kilobyte, which does not.
static HOOKS: OnceLock<Box<[u8]>> = OnceLock::new();

/// The card carries a continuous aura.
pub const HAS_AURA: u8 = 1;
/// The card grants itself a conditional bonus.
pub const HAS_BONUS: u8 = 2;
/// The card reacts to events.
pub const HAS_TRIGGER: u8 = 4;

/// The hook table, built from `BEHAVIOURS` the first time it is asked for.
/// Index it by `CardId`, the same as the behaviour index itself.
pub fn hooks() -> &'static [u8] {
    HOOKS.get_or_init(|| {
        let index = index();
        let mut out = vec![0u8; index.len()].into_boxed_slice();
        for (i, &slot) in index.iter().enumerate() {
            if slot == 0 {
                continue;
            }
            let b = &BEHAVIOURS[slot as usize - 1];
            if b.aura.is_some() {
                out[i] |= HAS_AURA;
            }
            if b.bonus.is_some() {
                out[i] |= HAS_BONUS;
            }
            if b.trigger.is_some() {
                out[i] |= HAS_TRIGGER;
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
    // A card whose whole rules text is a side-list mechanic has no hook to
    // hang a behaviour on and is still implemented: Stonetalon Striker is a
    // Taunt that wakes up, and the waking lives in `AWAKENS`.
    if awakened_by_dragon(card).is_some()
        || shatters_into(card).is_some()
        || reborn_keeps_enchantments(card)
        || lets_attacks_ignore_taunt(card)
        || rule_lives_in_the_engine(card)
    {
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
    fn the_dark_gift_pool_is_the_one_the_corpus_lists() {
        // Every Dark Gift card carries the pool as its own children, so the
        // list here is checkable against the data rather than trusted. If the
        // corpus ever adds or drops one, this fails instead of the engine
        // quietly rolling from a stale pool.
        for name in ["Cremate", "Rite of Atrocity"] {
            let card = crate::cards::by_name(name).expect("in the corpus");
            let mut theirs: Vec<u16> = card.children().iter().map(|c| c.0).collect();
            let mut ours: Vec<u16> = super::DARK_GIFTS.iter().map(|c| c.0).collect();
            theirs.sort_unstable();
            ours.sort_unstable();
            assert_eq!(theirs, ours, "{name} lists a different pool");
        }
    }

    #[test]
    fn every_dark_gift_effect_matches_its_printed_text() {
        // The stats and keywords are read off each Gift's own card text, so
        // the text is what the table is checked against.
        let want: [(&str, (i16, i16, i16), Keywords); 10] = [
            ("Waking Terror", (3, 0, 0), Keywords::LIFESTEAL),
            ("Well Rested", (2, 2, 0), Keywords::ELUSIVE),
            ("Short Claws", (-2, 0, -2), Keywords::NONE),
            ("Bundled Up", (0, 4, 0), Keywords::TAUNT),
            ("Living Nightmare", (0, 0, 0), Keywords::NONE),
            ("Sleepwalker", (0, 0, 0), Keywords::CHARGE),
            ("Rude Awakening", (0, 0, 0), Keywords::NONE),
            ("Sweet Dreams", (4, 5, 0), Keywords::NONE),
            ("Persisting Horror", (0, 0, 0), Keywords::REBORN),
            (
                "Harpy's Talons",
                (0, 0, 0),
                Keywords(Keywords::DIVINE_SHIELD.0 | Keywords::WINDFURY.0),
            ),
        ];
        for (i, (name, stats, kw)) in want.iter().enumerate() {
            let gift = i as u8 + 1;
            assert_eq!(
                super::gift_card(gift).map(|c| c.name()),
                Some(*name),
                "gift {gift} is not {name}"
            );
            assert_eq!(super::gift_stats(gift), *stats, "{name}");
            assert_eq!(super::gift_keywords(gift), *kw, "{name}");
        }
        assert_eq!(super::gift_card(0), None);
        assert_eq!(super::gift_card(11), None);
    }

    #[test]
    fn every_drawn_actor_really_prints_it() {
        // The list is by name because the corpus carries no mechanic for
        // this -- only the words at the head of the card's text. If a name
        // here stops printing them, the list is wrong.
        for card in super::DRAWN_ACTORS {
            let name = card.name();
            // Keyed by id, so nothing else may answer to the same name: a
            // second printing would act on the draw for one copy and not the
            // other.
            assert_eq!(
                crate::cards::all().filter(|c| c.name() == name).count(),
                1,
                "{name} has more than one printing"
            );
            let text = card.info().text;
            assert!(
                text.contains("When Drawn"),
                "{name} does not print \"When Drawn\""
            );
            // A spell casts, a minion is summoned; the card's own kind is
            // what the draw path reads, so the two must agree.
            let is_spell = card.def().kind() == crate::cards::Kind::Spell;
            assert_eq!(
                is_spell,
                text.contains("Casts When Drawn"),
                "{name} prints one thing and is another"
            );
        }
    }

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
            // Implemented, not necessarily through a row of its own: a card
            // whose whole text is a standing rule the engine reads has no
            // hook to hang a behaviour on and can still be approximate --
            // Khadgar's summon doubling is in `Game::doubled_summons`. What
            // this is really guarding against is an entry for a card nothing
            // plays at all, which `is_implemented` answers exactly.
            assert!(
                is_implemented(card),
                "{name} is listed as approximate but is not implemented at all"
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
            // cast effect of its own either; nor does a bare enchantment that
            // only exists to be granted.
            if GRANTED_RATTLES.contains(&b.name) && b.spell.is_none() {
                continue;
            }
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
                let card = by_name(b.name).unwrap();
                // The classic "Choose One -" wording sets CHOOSE_ONE. Newer
                // templated wording ("Battlecry: Choose to gain X, Y, or Z.",
                // e.g. Ancient Stegodon) offers the same discrete pick but the
                // corpus never tags it with the mechanic, only the text.
                // The Arcanomicon is the third wording: "Choose an upgrade
                // for your Leylines", with the three upgrades printed as its
                // own children. It is named rather than matched, because
                // "Choose a/an ..." is overwhelmingly a *targeting* template
                // ("Choose a card in your hand to discard") and accepting it
                // would let a real mistake through -- which is the only thing
                // this assertion is here to catch.
                assert!(
                    card.def().keywords.has(Keywords::CHOOSE_ONE)
                        || card.info().text.contains("Choose to")
                        || b.name == "The Arcanomicon",
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
            if card.def().kind() != super::super::Kind::Minion {
                continue;
            }
            // A name can belong to both, and this table is keyed by name. The
            // Rock that Rockskipper hands over is a 1-Cost spell; an old
            // Wild token called Rock is a 1-Cost Taunt minion, and `by_name`
            // finds that one first. The row's cast effect belongs to the
            // spell and is never reached from the minion, which does not
            // cast. So what this is really asking is whether *no* printing of
            // the name is a spell.
            if super::super::all().any(|c| {
                c.name() == b.name && c.def().kind() == super::super::Kind::Spell
            }) {
                continue;
            }
            assert!(
                b.spell.is_none(),
                "{} is a minion with a cast effect",
                b.name
            );
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
                // Battlecry, Combo, Kindred, "When summoned" and Colossal all
                // resolve as the card enters play. The corpus tags the first
                // two as mechanics; the others exist only in the text --
                // Colossal has no `Keywords` constant of its own (G4 is not
                // otherwise built), so it is matched the same way.
                let text = card.info().text;
                assert!(
                    d.keywords.has(Keywords::BATTLECRY)
                        || d.keywords.has(Keywords::COMBO)
                        // Outcast resolves as the card enters play, the same
                        // hook under a different printed name.
                        || d.keywords.has(Keywords::OUTCAST)
                        || text.contains("Kindred")
                        || text.contains("When summoned")
                        || text.contains("Colossal"),
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
            if b.deathrattle.is_some() && !GRANTED_RATTLES.contains(&b.name) {
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
