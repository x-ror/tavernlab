//! The game state: one flat value.
//!
//! Everything here is `Copy` and fixed-size. There is no allocation, no
//! indirection and nothing shared, so a position clones with a `memcpy` and a
//! whole game fits comfortably in L1. That is the property the rest of the
//! engine is built to preserve — see the crate docs for why.
//!
//! The layout follows the rules' own limits rather than a general-purpose
//! container: seven board slots, ten cards in hand, five secrets. Where the
//! rules cap something, the type caps it too, and "the board is full" becomes
//! a value the code has to handle rather than a condition it might forget.

use crate::cards::{CardId, Class, Keywords, Kind, Races};
use crate::inline::Inline;
use crate::rng::Rngs;

pub const MAX_BOARD: usize = 7;
pub const MAX_HAND: usize = 10;
/// Decks start at 30 but cards get shuffled in; this is the hard ceiling.
pub const MAX_DECK: usize = 60;
pub const MAX_SECRETS: usize = 5;
pub const MAX_MANA: i16 = 10;
pub const START_HP: i16 = 30;
/// A game this long is a draw. Real games never approach it; fatigue ends
/// them long before.
pub const TURN_LIMIT: u16 = 89;

// ---------------------------------------------------------------- flags

/// Per-permanent state that is not a printed keyword.
///
/// Kept separate from [`Keywords`] because silence clears keywords but must
/// not un-freeze a minion or forget that it already attacked.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Flags(pub u16);

impl Flags {
    pub const NONE: Flags = Flags(0);
    pub const FROZEN: Flags = Flags(1 << 0);
    /// Frozen during the current turn, so it does not thaw at end of turn.
    pub const FROZE_THIS_TURN: Flags = Flags(1 << 1);
    pub const SILENCED: Flags = Flags(1 << 2);
    /// Summoned this turn — summoning sickness, unless Charge or Rush.
    pub const JUST_SUMMONED: Flags = Flags(1 << 3);
    /// Marked for death; removed by the next death sweep.
    pub const PENDING_DESTROY: Flags = Flags(1 << 4);
    /// Dormant minions are on the board but not in play.
    pub const DORMANT: Flags = Flags(1 << 5);
    /// Already used this turn (locations).
    pub const USED: Flags = Flags(1 << 6);
    /// Attacked at least once this turn — Rush cannot go face afterwards
    /// either, so this is tracked separately from the attack counter.
    pub const ATTACKED: Flags = Flags(1 << 7);
    /// Dies at the end of its controller's current turn (Soulrest Ceremony),
    /// independent of any keyword — silencing away the Rush it was granted
    /// alongside this must not save it, so this is not modelled as one.
    pub const DOOMED: Flags = Flags(1 << 8);

    #[inline]
    pub const fn has(self, f: Flags) -> bool {
        self.0 & f.0 != 0
    }
    #[inline]
    pub fn insert(&mut self, f: Flags) {
        self.0 |= f.0;
    }
    #[inline]
    pub fn remove(&mut self, f: Flags) {
        self.0 &= !f.0;
    }
    #[inline]
    pub fn set(&mut self, f: Flags, on: bool) {
        if on { self.insert(f) } else { self.remove(f) }
    }
}

// ----------------------------------------------------------- permanents

/// A minion or a location on the board.
///
/// One type for both because the seven-slot limit counts them together; the
/// card's own [`Kind`] says which it is, so no discriminant is stored.
#[derive(Clone, Copy, Debug, Default)]
pub struct Permanent {
    pub card: CardId,
    /// Current attack, after buffs and auras.
    pub atk: i16,
    /// Current maximum health, after buffs.
    pub max_hp: i16,
    /// Damage taken. Health is `max_hp - damage`.
    pub damage: i16,
    /// Attack granted for this turn only, already included in `atk`. Kept
    /// separately so end of turn can take back exactly what it gave, which a
    /// plain "recompute from the card" cannot do once permanent buffs exist.
    pub temp_atk: i16,
    /// Attack currently granted by other minions' auras, already folded into
    /// `atk`. Held separately so recomputation can take back exactly what it
    /// gave when the source leaves play.
    pub aura_atk: i16,
    /// Health currently granted by auras, already folded into `max_hp`.
    pub aura_hp: i16,
    /// Live keywords. Starts from the card and is cleared by silence, so a
    /// silenced Taunt stops taunting without the card changing.
    pub keywords: Keywords,
    pub flags: Flags,
    pub attacks_done: u8,
    /// Turns left dormant.
    pub dormant: u8,
    /// Turns until a location can be used again.
    pub cooldown: u8,
    pub spell_damage: i8,
    /// A generic per-instance counter for cards that need to remember
    /// something about themselves while they are on the board -- how many of
    /// their controller's turns they have lived through, so a deathrattle can
    /// scale by it. Zero for every card that does not use it.
    pub growth: u8,
}

impl Default for CardId {
    /// Slot 0 of the card table, used only to fill vacated inline slots. No
    /// live permanent ever holds it.
    fn default() -> Self {
        CardId(0)
    }
}

impl Permanent {
    /// A freshly summoned minion or placed location.
    pub fn summon(card: CardId) -> Self {
        let d = card.def();
        let mut p = Self {
            card,
            atk: d.atk,
            temp_atk: 0,
            aura_atk: 0,
            aura_hp: 0,
            max_hp: if d.kind() == Kind::Location {
                d.dur.max(d.hp)
            } else {
                d.hp
            },
            damage: 0,
            keywords: d.keywords,
            flags: Flags::JUST_SUMMONED,
            attacks_done: 0,
            dormant: d.dormant as u8,
            cooldown: 0,
            spell_damage: d.spell_damage,
            growth: 0,
        };
        if p.dormant > 0 {
            p.flags.insert(Flags::DORMANT);
        }
        p
    }

    #[inline]
    pub fn health(&self) -> i16 {
        self.max_hp - self.damage
    }

    #[inline]
    pub fn is_dead(&self) -> bool {
        self.health() <= 0 || self.flags.has(Flags::PENDING_DESTROY)
    }

    #[inline]
    pub fn kind(&self) -> Kind {
        self.card.def().kind()
    }

    #[inline]
    pub fn is_minion(&self) -> bool {
        self.kind() == Kind::Minion
    }

    #[inline]
    pub fn races(&self) -> Races {
        self.card.def().races
    }

    /// In play and able to be interacted with. Dormant minions are on the
    /// board but not targetable and cannot attack or be attacked.
    #[inline]
    pub fn active(&self) -> bool {
        !self.flags.has(Flags::DORMANT) && !self.is_dead()
    }

    #[inline]
    pub fn has(&self, k: Keywords) -> bool {
        self.keywords.has(k)
    }

    /// How many times this may attack per turn.
    #[inline]
    pub fn max_attacks(&self) -> u8 {
        if self.has(Keywords::WINDFURY) { 2 } else { 1 }
    }

    /// Whether it can attack something right now, ignoring what the target is.
    pub fn can_attack(&self) -> bool {
        if !self.active() || !self.is_minion() {
            return false;
        }
        if self.atk <= 0
            || self.flags.has(Flags::FROZEN)
            || self.has(Keywords::CANT_ATTACK)
            || self.attacks_done >= self.max_attacks()
        {
            return false;
        }
        // Summoning sickness, waived by Charge and Rush.
        !self.flags.has(Flags::JUST_SUMMONED)
            || self.has(Keywords::CHARGE)
            || self.has(Keywords::RUSH)
    }

    /// Whether it may attack the enemy hero specifically.
    ///
    /// Rush is the whole reason this is separate: a rushed minion can trade on
    /// the turn it lands but cannot go face until the turn after.
    pub fn can_attack_face(&self) -> bool {
        if !self.can_attack() {
            return false;
        }
        if !self.flags.has(Flags::JUST_SUMMONED) {
            return true;
        }
        self.has(Keywords::CHARGE)
    }

    /// Strip keywords and buffs the way Silence does.
    ///
    /// Damage already taken stays, and health returns to the printed value —
    /// a silenced minion that was buffed to 5/5 and took 2 becomes a 1/2 at
    /// full health only if the printed health exceeds the damage.
    pub fn silence(&mut self) {
        if self.has(Keywords::CANT_BE_SILENCED) {
            return;
        }
        let d = self.card.def();
        self.keywords = Keywords::NONE;
        self.atk = d.atk;
        self.temp_atk = 0;
        // Aura bookkeeping resets too; the next recomputation will re-apply
        // whatever still legitimately affects this minion.
        self.aura_atk = 0;
        self.aura_hp = 0;
        self.max_hp = d.hp;
        self.spell_damage = 0;
        self.flags.insert(Flags::SILENCED);
        self.flags.remove(Flags::FROZEN);
        if self.damage > self.max_hp {
            self.damage = self.max_hp;
        }
    }
}

// -------------------------------------------------------------- weapons

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Weapon {
    pub card: CardId,
    pub atk: i16,
    pub durability: i16,
}

impl Weapon {
    pub fn equip(card: CardId) -> Self {
        let d = card.def();
        Self {
            card,
            atk: d.atk,
            durability: d.dur,
        }
    }
}

// ----------------------------------------------------------- hand cards

/// What has happened while one specific card sat in hand -- "while holding
/// this" text, which two copies of the same card can answer differently.
/// Cleared along with the rest of [`HandCard`] the moment it leaves hand.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Marks(pub u8);

impl Marks {
    pub const NONE: Marks = Marks(0);
    /// A minion was played while this card sat in hand (Ebb and Flow).
    pub const PLAYED_MINION: Marks = Marks(1 << 0);
    /// A card of the opponent's class was played while this card sat in
    /// hand. Only possible by having picked one up somehow -- a deck cannot
    /// contain them otherwise -- so "played a copy of an opponent's card"
    /// reduces to "played a card that is not Neutral and not mine" (Mind
    /// Sweeper, Unshackle Soul).
    pub const PLAYED_OPPONENT_CARD: Marks = Marks(1 << 1);
    /// This exact card was drawn by Platysaur's battlecry, so its
    /// deathrattle knows which card in hand to discard.
    pub const DRAWN_BY_PLATYSAUR: Marks = Marks(1 << 2);
    /// A card costing more than this one's own printed cost was played
    /// while this card sat in hand (Shaladrassil).
    pub const PLAYED_HIGHER_COST: Marks = Marks(1 << 3);

    #[inline]
    pub const fn has(self, m: Marks) -> bool {
        self.0 & m.0 != 0
    }
    #[inline]
    pub fn insert(&mut self, m: Marks) {
        self.0 |= m.0;
    }
}

/// A card in hand, with the per-copy state that makes two copies differ.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HandCard {
    pub card: CardId,
    /// Cost modification applied to this copy only.
    pub cost_delta: i16,
    /// Turn number on which this card became unplayable (Prepare), or
    /// `u16::MAX` for never.
    pub locked_turn: u16,
    pub marks: Marks,
}

impl HandCard {
    pub fn new(card: CardId) -> Self {
        Self {
            card,
            cost_delta: 0,
            locked_turn: u16::MAX,
            marks: Marks::NONE,
        }
    }
}

// -------------------------------------------------------------- pending

/// What a queued [`Pending`] effect does when it fires.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum PendingKind {
    #[default]
    None = 0,
    /// Gain a temporary mana crystal.
    TempCrystal = 1,
    /// Summon `card`.
    SummonToken = 2,
    /// Damage the owner's own hero for `amount`.
    HeroDamage = 3,
}

/// An effect queued against a future one of its owner's own turns —
/// "at the start of your turn" with a duration, or "at the start of your
/// next turn" once. Ticks down and fires in [`Game::begin_turn`], once per
/// owning player's own turn: casting on someone else's turn does not make
/// it fire sooner, because it is only ever read from that player's side of
/// [`Player::pending`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pending {
    pub kind: PendingKind,
    /// How many more of the owner's own turns this fires on, counting the
    /// next one. Removed once it reaches zero after firing.
    pub turns_left: u8,
    pub amount: i16,
    pub card: CardId,
}

// -------------------------------------------------------------- players

#[derive(Clone, Copy, Debug)]
pub struct Player {
    pub class: Class,
    pub hero_hp: i16,
    pub armor: i16,
    /// Attack granted this turn from buffs, on top of any weapon.
    pub hero_bonus_atk: i16,
    pub hero_attacks_done: u8,
    /// A frozen hero cannot attack. Freeze applies to characters, not just
    /// minions, so the hero carries the same pair of flags a minion does.
    pub hero_frozen: bool,
    /// Frozen during its own controller's turn, so it does not thaw at the
    /// end of it.
    pub hero_froze_this_turn: bool,
    /// Absorbs the next instance of damage to this hero, the same way a
    /// minion's Divine Shield does (Hardlight Protector).
    pub hero_divine_shield: bool,
    pub mana: i16,
    /// Mana crystals owned, capped at [`MAX_MANA`].
    pub crystals: i16,
    pub overload_now: i16,
    pub overload_next: i16,
    /// Fatigue counter: damage the next empty draw will deal.
    pub fatigue: i16,
    pub corpses: i16,
    /// How many times this player has Heralded. The count scales the Soldier
    /// each one summons: 1, then 2 from the second, then 4 from the fourth.
    pub herald: i16,
    /// A discount waiting to be spent on the next spell cast this turn
    /// (Preparation). Consumed by the cast, cleared at end of turn.
    pub next_spell_discount: i16,
    /// A discount waiting to be spent on the next Beast minion played this
    /// turn (Cower in Fear). Same shape as `next_spell_discount`, one tribe
    /// over.
    pub next_beast_discount: i16,
    /// Spell schools cast this turn, as bits indexed by `School`'s own
    /// discriminant -- "if you've cast a Fire spell this turn" reads bit 3.
    pub schools_cast_turn: u8,
    pub cards_played_turn: u8,
    /// Spells cast this turn. Separate from `cards_played_turn` because a
    /// family of cards asks specifically about spells ("if you've cast a
    /// spell this turn"), and counting minions towards that reads the card
    /// wrong in exactly the decks that play it.
    pub spells_cast_turn: u8,
    /// Spell Damage the player carries independently of the board, from
    /// cards that hand it to the hero rather than to a minion.
    pub spell_power_bonus: i16,
    /// Tribes of the minions played this turn, and last turn. Kindred asks
    /// whether you played something of the same tribe on your previous turn,
    /// so the two have to be kept separately and rolled over at end of turn.
    pub played_races_turn: Races,
    pub played_races_last: Races,
    pub hero_power: CardId,
    pub hero_power_uses: u8,
    /// A second, independent Hero Power granted on top of the class one
    /// (Blood Doctor Thal'ena) -- both are usable the same turn, each once,
    /// tracked separately.
    pub second_hero_power: Option<CardId>,
    pub second_hero_power_uses: u8,
    /// Extra cost on this player's own spells, active for the rest of this
    /// turn only (Cult Neophyte, set on the *target*). Promoted from
    /// `spell_tax_pending` at the start of the turn it applies to, and
    /// implicitly cleared the same way at the turn after: the promotion
    /// always overwrites it, pending or not.
    pub spell_tax_active: i16,
    /// A spell tax queued for the start of this player's own next turn.
    pub spell_tax_pending: i16,
    /// Effects queued against this player's own future turns; see
    /// [`Pending`].
    pub pending: Inline<Pending, 4>,
    /// Friendly characters damaged so far this turn (Warptooth), counting
    /// instances, not distinct characters -- the same one hit twice counts
    /// twice.
    pub friendly_damaged_turn: u8,
    /// The active Quest, as (card, progress). A Quest's own printed number
    /// is not stored here, since its `trigger` already knows it -- this is
    /// only ever read back by the same card that wrote it.
    pub quest: Option<(CardId, u8)>,
    /// The active Sidequest, in its own slot: Hearthstone allows one Quest
    /// and one Sidequest active at the same time, and Quest Hunter runs one
    /// of each.
    pub sidequest: Option<(CardId, u8)>,
    pub weapon: Option<Weapon>,
    pub hand: Inline<HandCard, MAX_HAND>,
    pub deck: Inline<CardId, MAX_DECK>,
    pub board: Inline<Permanent, MAX_BOARD>,
    pub secrets: Inline<CardId, MAX_SECRETS>,
}

impl Player {
    pub fn new(class: Class, hero_power: CardId, deck: &[CardId]) -> Self {
        Self {
            class,
            hero_hp: START_HP,
            armor: 0,
            hero_bonus_atk: 0,
            hero_attacks_done: 0,
            hero_frozen: false,
            hero_froze_this_turn: false,
            hero_divine_shield: false,
            mana: 0,
            crystals: 0,
            overload_now: 0,
            overload_next: 0,
            fatigue: 0,
            corpses: 0,
            herald: 0,
            next_spell_discount: 0,
            next_beast_discount: 0,
            schools_cast_turn: 0,
            cards_played_turn: 0,
            spells_cast_turn: 0,
            spell_power_bonus: 0,
            played_races_turn: Races::NONE,
            played_races_last: Races::NONE,
            hero_power,
            hero_power_uses: 0,
            second_hero_power: None,
            second_hero_power_uses: 0,
            spell_tax_active: 0,
            spell_tax_pending: 0,
            pending: Inline::new(),
            friendly_damaged_turn: 0,
            quest: None,
            sidequest: None,
            weapon: None,
            hand: Inline::new(),
            deck: deck.iter().copied().collect(),
            board: Inline::new(),
            secrets: Inline::new(),
        }
    }

    /// Total attack the hero swings for: weapon plus temporary buffs.
    #[inline]
    pub fn hero_attack(&self) -> i16 {
        self.weapon.map_or(0, |w| w.atk) + self.hero_bonus_atk
    }

    #[inline]
    pub fn hero_can_attack(&self) -> bool {
        self.hero_attack() > 0 && self.hero_attacks_done == 0 && !self.hero_frozen
    }

    /// Sum of Spell Damage on the board.
    #[inline]
    pub fn spell_power(&self) -> i16 {
        self.spell_power_bonus
            + self
                .board
                .iter()
                .filter(|p| p.active())
                .map(|p| p.spell_damage as i16)
                .sum::<i16>()
    }

    /// Cost to play this card now, floored at zero.
    #[inline]
    pub fn effective_cost(&self, c: &HandCard) -> i16 {
        (c.card.def().cost + c.cost_delta).max(0)
    }

    /// Minions in play, skipping dormant and dead ones.
    #[inline]
    pub fn minions(&self) -> impl Iterator<Item = &Permanent> {
        self.board.iter().filter(|p| p.is_minion() && p.active())
    }

    /// Whether any live minion is taunting. Stealthed taunts do not compel an
    /// attack, which is why this is not just "any taunt".
    pub fn has_taunt(&self) -> bool {
        self.board.iter().any(|p| {
            p.active() && p.is_minion() && p.has(Keywords::TAUNT) && !p.has(Keywords::STEALTH)
        })
    }

    #[inline]
    pub fn is_dead(&self) -> bool {
        self.hero_hp <= 0
    }
}

// ----------------------------------------------------------------- game

/// Which side an effect points at.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Side {
    #[default]
    Player0 = 0,
    Player1 = 1,
}

impl Side {
    #[inline]
    pub fn other(self) -> Side {
        match self {
            Side::Player0 => Side::Player1,
            Side::Player1 => Side::Player0,
        }
    }
    #[inline]
    pub fn index(self) -> usize {
        self as usize
    }
    #[inline]
    pub fn from_index(i: usize) -> Side {
        if i == 0 { Side::Player0 } else { Side::Player1 }
    }
}

/// A character an effect can point at: a hero, or a minion in a board slot.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Target {
    Hero(Side),
    Minion(Side, u8),
}

impl Default for Target {
    /// Only ever used to fill vacated slots in an [`Inline`] buffer; the
    /// all-zero bit pattern has to agree with it, which
    /// `state::tests::target_default_is_the_zero_pattern` checks.
    fn default() -> Self {
        Target::Hero(Side::Player0)
    }
}

/// How a finished game ended.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    Win(Side),
    /// Both heroes died in the same resolution, or the turn limit was reached.
    Draw,
}

/// A complete game.
///
/// One value, no allocation, no shared state. Cloning is a `memcpy`, which is
/// what makes lookahead search affordable.
#[derive(Clone, Copy, Debug)]
pub struct Game {
    pub players: [Player; 2],
    pub current: Side,
    pub turn: u16,
    pub outcome: Option<Outcome>,
    pub rngs: Rngs,
    /// Set whenever the board changes, so the agent's lethal check can be
    /// skipped when nothing moved. In the Python engine that search ran 4.3
    /// times per turn for no reason; here the invalidation is explicit.
    pub board_dirty: bool,
    /// Minions that have died this turn, either side. Reset at each turn
    /// boundary; read by cards that scale with the carnage.
    pub deaths_this_turn: u8,
    /// How many triggers deep the current resolution is. Two triggers that
    /// feed each other would otherwise never finish.
    pub trigger_depth: u8,
    /// Set by Counterspell while a spell is being cast; the cast is abandoned
    /// and the flag cleared by the same call that raised it.
    pub countered: bool,
}

impl Game {
    #[inline]
    pub fn me(&self) -> &Player {
        &self.players[self.current.index()]
    }

    #[inline]
    pub fn me_mut(&mut self) -> &mut Player {
        &mut self.players[self.current.index()]
    }

    #[inline]
    pub fn them(&self) -> &Player {
        &self.players[self.current.other().index()]
    }

    #[inline]
    pub fn them_mut(&mut self) -> &mut Player {
        &mut self.players[self.current.other().index()]
    }

    #[inline]
    pub fn player(&self, s: Side) -> &Player {
        &self.players[s.index()]
    }

    #[inline]
    pub fn player_mut(&mut self, s: Side) -> &mut Player {
        &mut self.players[s.index()]
    }

    #[inline]
    pub fn is_over(&self) -> bool {
        self.outcome.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cards::by_name;

    #[test]
    fn inline_element_types_have_sane_defaults() {
        // Vacated slots hold these, so they must be inert rather than a
        // half-built entity that later code could mistake for real.
        assert_eq!(CardId::default().0, 0);
        assert_eq!(
            HandCard::default(),
            HandCard {
                card: CardId(0),
                cost_delta: 0,
                locked_turn: 0,
                marks: Marks::NONE,
            }
        );
        assert_eq!(
            Weapon::default(),
            Weapon {
                card: CardId(0),
                atk: 0,
                durability: 0
            }
        );
        let p = Permanent::default();
        assert_eq!((p.atk, p.max_hp, p.damage), (0, 0, 0));
        assert_eq!(p.flags, Flags::NONE);
    }

    #[test]
    fn a_game_is_small_enough_to_copy_freely() {
        // The number that justifies the whole design. If this grows past a few
        // kilobytes, search stops being cheap and the reason for Rust is gone.
        let n = size_of::<Game>();
        assert!(
            n < 2048,
            "Game is {n} bytes; it is meant to stay under 2 KB"
        );
    }

    #[test]
    fn a_summoned_minion_starts_sick() {
        let m = Permanent::summon(by_name("Bloodfen Raptor").unwrap());
        assert_eq!((m.atk, m.max_hp), (3, 2));
        assert!(m.flags.has(Flags::JUST_SUMMONED));
        assert!(!m.can_attack(), "summoning sickness");
    }

    #[test]
    fn charge_attacks_immediately_rush_only_trades() {
        let charger = Permanent::summon(by_name("Wolfrider").unwrap());
        assert!(charger.has(Keywords::CHARGE));
        assert!(charger.can_attack());
        assert!(charger.can_attack_face(), "Charge may go face on turn one");

        let rusher = crate::cards::all()
            .map(Permanent::summon)
            .find(|m| m.has(Keywords::RUSH) && !m.has(Keywords::CHARGE) && m.atk > 0)
            .expect("the corpus has a Rush minion");
        assert!(rusher.can_attack(), "Rush may attack minions at once");
        assert!(
            !rusher.can_attack_face(),
            "Rush may not go face on the turn it lands"
        );
    }

    #[test]
    fn windfury_doubles_the_attack_allowance() {
        let mut m = Permanent::summon(by_name("Bloodfen Raptor").unwrap());
        m.flags.remove(Flags::JUST_SUMMONED);
        assert_eq!(m.max_attacks(), 1);
        m.keywords.insert(Keywords::WINDFURY);
        assert_eq!(m.max_attacks(), 2);
        m.attacks_done = 1;
        assert!(m.can_attack());
        m.attacks_done = 2;
        assert!(!m.can_attack());
    }

    #[test]
    fn frozen_and_cant_attack_both_stop_an_attack() {
        let mut m = Permanent::summon(by_name("Bloodfen Raptor").unwrap());
        m.flags.remove(Flags::JUST_SUMMONED);
        assert!(m.can_attack());
        m.flags.insert(Flags::FROZEN);
        assert!(!m.can_attack());
        m.flags.remove(Flags::FROZEN);
        m.keywords.insert(Keywords::CANT_ATTACK);
        assert!(!m.can_attack());
    }

    #[test]
    fn zero_attack_minions_cannot_attack() {
        let m = crate::cards::all()
            .map(Permanent::summon)
            .find(|m| m.atk == 0 && m.is_minion())
            .expect("the corpus has a 0-attack minion");
        assert!(!m.can_attack());
    }

    #[test]
    fn silence_strips_keywords_and_buffs_but_keeps_damage() {
        let mut m = Permanent::summon(by_name("Goldshire Footman").unwrap()); // 1/2 Taunt
        m.atk += 4;
        m.max_hp += 4;
        m.damage = 3;
        assert!(m.has(Keywords::TAUNT));
        m.silence();
        assert!(!m.has(Keywords::TAUNT));
        assert_eq!((m.atk, m.max_hp), (1, 2));
        // Damage exceeded the printed health, so it is clamped and the minion
        // is dead rather than resurrected at negative health.
        assert_eq!(m.damage, 2);
        assert!(m.is_dead());
    }

    #[test]
    fn dormant_minions_are_not_active() {
        let mut m = Permanent::summon(by_name("Bloodfen Raptor").unwrap());
        m.flags.insert(Flags::DORMANT);
        assert!(!m.active());
        assert!(!m.can_attack());
    }

    #[test]
    fn health_tracks_damage() {
        let mut m = Permanent::summon(by_name("Bloodfen Raptor").unwrap());
        assert_eq!(m.health(), 2);
        m.damage = 1;
        assert_eq!(m.health(), 1);
        assert!(!m.is_dead());
        m.damage = 2;
        assert!(m.is_dead());
    }

    #[test]
    fn stealthed_taunt_does_not_compel_an_attack() {
        let mut p = Player::new(Class::Neutral, by_name("Fireblast").unwrap(), &[]);
        let mut m = Permanent::summon(by_name("Goldshire Footman").unwrap());
        m.keywords.insert(Keywords::STEALTH);
        p.board.push(m);
        assert!(
            !p.has_taunt(),
            "a stealthed taunt cannot be attacked, so it compels nothing"
        );
        p.board[0].keywords.remove(Keywords::STEALTH);
        assert!(p.has_taunt());
    }

    #[test]
    fn hero_attack_combines_weapon_and_buff() {
        let mut p = Player::new(Class::Warrior, by_name("Armor Up!").unwrap(), &[]);
        assert!(!p.hero_can_attack());
        p.weapon = Some(Weapon::equip(by_name("Fiery War Axe").unwrap()));
        assert_eq!(p.hero_attack(), 3);
        p.hero_bonus_atk = 2;
        assert_eq!(p.hero_attack(), 5);
        assert!(p.hero_can_attack());
        p.hero_attacks_done = 1;
        assert!(!p.hero_can_attack());
    }

    #[test]
    fn target_has_a_default_for_inline_buffers() {
        // `Inline` fills vacated slots with `T::default()`, so `Target` needs
        // one. Its value is arbitrary — nothing reads a vacated slot — but it
        // must exist and be stable.
        assert_eq!(Target::default(), Target::Hero(Side::Player0));
        assert_eq!(Side::default(), Side::Player0);
    }

    #[test]
    fn sides_are_symmetric() {
        assert_eq!(Side::Player0.other(), Side::Player1);
        assert_eq!(Side::Player1.other().other(), Side::Player1);
        assert_eq!(Side::from_index(1), Side::Player1);
    }
}
