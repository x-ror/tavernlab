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
/// How many of a player's dead minions are remembered. See `Player::graveyard`.
pub const GRAVEYARD: usize = 32;
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
    /// Set on a minion from the moment it lands until its own `CardPlayed`
    /// event has been delivered.
    ///
    /// "Whenever you play a card" does not fire for the card being played,
    /// but by the time that event goes out the minion is already in play --
    /// Wild Pyromancer depends on exactly that ordering. `Game::fire` skips
    /// a reactor carrying this flag for `CardPlayed` alone, so Questing
    /// Adventurer does not grow off its own arrival. A flag rather than a
    /// remembered slot, because a battlecry can reorder the board underneath
    /// it and a flag travels with the permanent.
    pub const BEING_PLAYED: Flags = Flags(1 << 9);
    /// This body did not come from the deck its owner built -- it was
    /// summoned as a token, transformed, resurrected, copied, or played from
    /// a card that was itself generated. Read only when the minion goes back
    /// to hand, so a bounced minion keeps knowing where it came from.
    ///
    /// Set by default on every summon and cleared in `Game::play_card` for a
    /// card played out of hand that came from the deck. That leaves one gap:
    /// a minion pulled straight out of the deck onto the board keeps the
    /// flag, so bouncing *that* minion marks it as generated. Nothing on the
    /// board carries a deck slot to consult, and the chain that would notice
    /// -- summon from deck, bounce, then trade or count it -- is narrow
    /// enough to name here rather than pay for everywhere.
    pub const NOT_FROM_DECK: Flags = Flags(1 << 10);
    /// This body comes back from Reborn at full Health rather than at one
    /// (the Persisting Horror Dark Gift).
    ///
    /// A flag rather than carrying the Gift's identity on every permanent:
    /// Persisting Horror is the only Dark Gift whose rule outlives the play,
    /// and a byte on `Permanent` costs a `Game` far more than a spare bit in
    /// a word that already exists.
    pub const REBORN_FULL: Flags = Flags(1 << 11);
    /// Queued to swing by "Force each minion to attack another minion".
    ///
    /// A marker, not a state: it is set on every minion on both boards before
    /// any of them attacks, and each one clears its own as it goes. That is
    /// what makes the card exact without a per-minion identity to track --
    /// a minion that dies part way through takes its mark with it, and one
    /// summoned by a Deathrattle in the middle never had one, so neither
    /// swings when it should not.
    pub const FORCED_TO_ATTACK: Flags = Flags(1 << 12);
    /// This body is the copy Reborn brought back, rather than the one that
    /// was played (Raith Van Geist, Sinful Steed).
    ///
    /// Reborn already strips the keyword from the returning copy so it cannot
    /// come back twice, which leaves nothing on the body to say it ever did.
    /// This says it, and it is what `Player::reborn_dead` reads when that copy
    /// dies in turn.
    pub const CAME_BACK: Flags = Flags(1 << 13);

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
    /// Set while this permanent is under borrowed control (Cursed Chains),
    /// to the side it should return to. Travels with the permanent itself
    /// through any board reordering, so returning it needs no search by
    /// name or id -- which could not tell two copies of the same card
    /// apart -- only a scan for this flag.
    pub stolen_from: Option<Side>,
    /// A deathrattle granted to this minion on top of its own, as the card
    /// that carries it -- "Give your minions \"Deathrattle: ...\"". `CardId(0)`
    /// means none, the same sentinel a vacated inline slot holds.
    pub granted_rattle: CardId,
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
            // Summoned, not played from a deck card, until something says
            // otherwise; see `Flags::NOT_FROM_DECK`.
            flags: Flags(Flags::JUST_SUMMONED.0 | Flags::NOT_FROM_DECK.0),
            attacks_done: 0,
            dormant: d.dormant as u8,
            cooldown: 0,
            spell_damage: d.spell_damage,
            growth: 0,
            stolen_from: None,
            granted_rattle: CardId(0),
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
        self.granted_rattle = CardId(0);
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
pub struct Marks(pub u16);

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
    /// This copy reached hand from somewhere other than the deck it was
    /// built from — generated, Discovered, or drawn after being shuffled in.
    /// Set by [`Game::give_card`] and cleared on the draw path, which copies
    /// [`DeckCard::started_here`] instead.
    ///
    /// It exists so a card put *back* into the deck (Tradeable) keeps its
    /// provenance. It is exact for every card that has only ever been drawn
    /// or generated; the one case it overstates is a minion bounced off the
    /// board back to hand, which the board carries no provenance for, and
    /// which only matters if that same card is then traded away again.
    pub const NOT_FROM_DECK: Marks = Marks(1 << 4);
    /// This copy burns at the end of the turn it was given, unplayed
    /// ("Temporary"). Per-copy, because the same card can sit in hand as a
    /// permanent copy and a temporary one at the same time.
    pub const TEMPORARY: Marks = Marks(1 << 5);
    /// This copy carries Follow the Fuse's effect for the turn: playing it
    /// also deals 2 damage to a random enemy. Cleared at the end of the turn
    /// it was given, like the spell says.
    pub const FUSED: Marks = Marks(1 << 6);
    /// A Stealthed minion attacked while this card sat in hand (Tricks of
    /// the Trade). Set on every card in hand at the moment of the swing, the
    /// same way `PLAYED_MINION` is set when a minion lands.
    pub const STEALTH_ATTACKED: Marks = Marks(1 << 7);
    /// This copy carries Follow the Footsteps' effect for the turn: playing
    /// it also Discovers a Stealth minion. Cleared with `FUSED` at the end of
    /// the turn it was given.
    pub const FOOTSTEPS: Marks = Marks(1 << 8);
    /// Prepared this turn, and so unplayable for the rest of it -- otherwise
    /// the discount could be banked into and cashed on the same turn.
    ///
    /// A mark rather than a turn number: the number would only ever be
    /// compared against the current turn, and a `u16` on every `HandCard`
    /// costs eighty bytes a `Game`. Cleared with `FUSED` at the owner's turn
    /// end, which is exactly "the rest of it".
    pub const PREPARED: Marks = Marks(1 << 9);
    /// Prepare granted to this copy rather than printed on the card (Wanted
    /// Poster). A mark rather than a keyword because keywords live on the
    /// immutable `CardDef` and this belongs to one card in one hand -- and
    /// `Marks` had a spare bit, so the whole grant costs a `Game` nothing.
    pub const GRANTED_PREPARE: Marks = Marks(1 << 10);
    /// This copy casts twice when it is played (the empowered Well of
    /// Eternity). A mark rather than anything on the card, for the same
    /// reason `GRANTED_PREPARE` is one: it belongs to one copy in one hand.
    pub const CASTS_TWICE: Marks = Marks(1 << 11);
    /// Two bits of counter for a card that upgrades while it is held and is
    /// discarded once it runs out (Smoldering Grove). Which cards do that is
    /// a fact about the card, not about the copy, so it lives in a side list
    /// and only the count is here -- and `Marks` had the bits spare.
    pub const HELD_ONE: Marks = Marks(1 << 12);
    pub const HELD_TWO: Marks = Marks(1 << 13);

    #[inline]
    pub const fn has(self, m: Marks) -> bool {
        self.0 & m.0 != 0
    }
    #[inline]
    pub fn insert(&mut self, m: Marks) {
        self.0 |= m.0;
    }
    #[inline]
    pub fn remove(&mut self, m: Marks) {
        self.0 &= !m.0;
    }

    /// How many of its owner's turns an upgrading card has been held for,
    /// 0 to 3, out of the two bits reserved for the count.
    #[inline]
    pub const fn held_turns(self) -> u8 {
        ((self.0 >> 12) & 0b11) as u8
    }

    /// Set that count, saturating at the three the two bits can hold.
    #[inline]
    pub fn set_held_turns(&mut self, n: u8) {
        let n = if n > 3 { 3 } else { n } as u16;
        self.0 = (self.0 & !(0b11 << 12)) | (n << 12);
    }
}

/// A card in hand, with the per-copy state that makes two copies differ.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HandCard {
    pub card: CardId,
    /// Cost modification applied to this copy only.
    pub cost_delta: i16,
    pub marks: Marks,
    /// Mana spent on anything else -- a card, a Hero Power -- while this one
    /// has been sitting in hand, since it arrived (Merithra of the Dream).
    /// Unlike `Marks`, this is a running sum rather than a flag, so it lives
    /// as its own field.
    pub mana_spent_while_held: i16,
    /// Stats granted to this copy while it is in hand -- "Give all minions in
    /// your hand +1/+1". Folded into the body the moment it is played, and
    /// into the weapon for a weapon. `i8` because these are small numbers and
    /// a hand is ten cards on two sides: the whole feature costs forty bytes
    /// of a `Game`.
    pub atk: i8,
    pub hp: i8,
    /// Which Dark Gift this copy carries, as a 1-based index into
    /// `cards::DARK_GIFTS`, or 0 for none.
    ///
    /// An index rather than a `CardId` because a `Game` has ten cards in
    /// hand and seven on board on both sides, and two bytes each would push
    /// it past the size the whole design rests on. The stat and cost halves
    /// of a Gift fold into `atk`/`hp`/`cost_delta` the moment it is given;
    /// this is what is left -- which Gift it was, for the keywords and the
    /// rules that only apply once the body is in play.
    pub gift: u8,
}

impl HandCard {
    pub fn new(card: CardId) -> Self {
        Self {
            card,
            cost_delta: 0,
            marks: Marks::NONE,
            mana_spent_while_held: 0,
            atk: 0,
            hp: 0,
            gift: 0,
        }
    }

    /// Add stats to this copy while it is in hand, saturating rather than
    /// wrapping: a card buffed past 127 is not a card any deck plays.
    pub fn enchant(&mut self, atk: i16, hp: i16) {
        self.atk = self.atk.saturating_add(atk.clamp(-128, 127) as i8);
        self.hp = self.hp.saturating_add(hp.clamp(-128, 127) as i8);
    }
}

// ----------------------------------------------------------- deck cards

/// A card in the deck, with the per-copy state that makes two copies of the
/// same card differ.
///
/// A deck is not a bag of identities: "Give +4/+4 to the top 3 minions in
/// your deck" buffs three specific copies and leaves the fourth alone, and
/// "cards that didn't start in your deck" asks each copy where it came from.
/// Both questions need somewhere to write the answer, and this is it. The
/// stats and the cost fold into the [`HandCard`] the moment the card is
/// drawn, exactly as a hand enchantment folds into the body when played.
///
/// Six bytes, of which one is padding the `CardId`'s alignment would cost
/// anyway — so `cost_delta` rides along free.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeckCard {
    pub card: CardId,
    /// Stats granted to this copy while it waits in the deck.
    pub atk: i8,
    pub hp: i8,
    /// Cost modification applied to this copy only, carried into hand on the
    /// draw. "Set the Cost to (1)" is stored as the delta that gets there,
    /// because a delta is what the hand already knows how to apply.
    pub cost_delta: i8,
    /// Whether this copy was in the list the deck was built from. Written
    /// once in [`Player::new`], and preserved across a Trade by
    /// [`Marks::NOT_FROM_DECK`]; everything shuffled in later is `false`.
    pub started_here: bool,
}

impl DeckCard {
    /// A copy that arrived after the game started — shuffled in, put back,
    /// created. Not part of the opening list.
    pub const fn new(card: CardId) -> Self {
        Self { card, atk: 0, hp: 0, cost_delta: 0, started_here: false }
    }

    /// A copy from the list the deck was built from.
    pub const fn started(card: CardId) -> Self {
        Self { card, atk: 0, hp: 0, cost_delta: 0, started_here: true }
    }

    #[inline]
    pub fn def(&self) -> &'static crate::cards::CardDef {
        self.card.def()
    }

    #[inline]
    pub fn name(&self) -> &'static str {
        self.card.name()
    }

    /// Add stats to this copy while it waits in the deck, saturating rather
    /// than wrapping, for the same reason [`HandCard::enchant`] does.
    pub fn enchant(&mut self, atk: i16, hp: i16) {
        self.atk = self.atk.saturating_add(atk.clamp(-128, 127) as i8);
        self.hp = self.hp.saturating_add(hp.clamp(-128, 127) as i8);
    }

    /// Set what this copy costs, whatever it was printed at. Stored as the
    /// delta that lands on `cost`, clamped so an absurd printed cost cannot
    /// wrap the byte.
    pub fn set_cost(&mut self, cost: i16) {
        let delta = cost - self.def().cost;
        self.cost_delta = delta.clamp(-128, 127) as i8;
    }

    /// The hand card this becomes when drawn, carrying everything the deck
    /// wrote on it.
    pub fn to_hand(self) -> HandCard {
        let mut hc = HandCard::new(self.card);
        hc.atk = self.atk;
        hc.hp = self.hp;
        hc.cost_delta = self.cost_delta as i16;
        if !self.started_here {
            hc.marks.insert(Marks::NOT_FROM_DECK);
        }
        hc
    }
}

impl From<CardId> for DeckCard {
    fn from(card: CardId) -> Self {
        Self::new(card)
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
    /// Deal `amount` damage split at random among the owner's enemies.
    SplitDamage = 4,
    /// Give the owner's hero `amount` Attack for that turn.
    HeroAttack = 5,
    /// Set the owner's Mana to `amount`, crystals and all (Chef Neth'rek).
    SetMana = 6,
    /// Cast `card`'s spell for the owner, as though they had played it.
    ///
    /// What a sham trial is: an effect chosen now and resolved several turns
    /// later, by the card it was chosen from.
    CastLater = 7,
    /// An Aura: summon `card` at the end of each of the owner's turns.
    AuraSummon = 8,
    /// An Aura: restore `amount` Health to all the owner's characters at the
    /// end of each of their turns.
    AuraHeal = 9,
    /// An Aura: give a random friendly minion `+amount/+amount` and Divine
    /// Shield at the end of each of the owner's turns.
    AuraBuff = 10,
    /// An Aura that is a modifier rather than an effect: while it is live,
    /// the owner's minions' end of turn effects trigger twice. Nothing fires
    /// for it -- `Game::fire` reads it -- but it still ticks down like the
    /// rest, which is why it is queued the same way.
    AuraDouble = 11,
}

impl PendingKind {
    /// Whether this fires once, when the count runs out, instead of on each
    /// of the turns counted.
    ///
    /// Most pending effects repeat: "for the next five turns" is five
    /// firings, one per turn, and `turns_left` is how many are left. A few
    /// read the other way -- "after five turns" is one firing, once they have
    /// passed -- and those wait.
    pub const fn delayed(self) -> bool {
        matches!(self, PendingKind::SetMana | PendingKind::CastLater)
    }

    /// Whether this fires at the end of its owner's turn rather than the
    /// start of it.
    ///
    /// The Aura cycle is printed "At the end of your turn", and one of them --
    /// Acceleration Aura -- is printed "At the start" and is the exception
    /// rather than the rule. Both ticks walk the same queue; this is what
    /// decides which one owns an entry.
    pub const fn at_end_of_turn(self) -> bool {
        matches!(
            self,
            PendingKind::AuraSummon
                | PendingKind::AuraHeal
                | PendingKind::AuraBuff
                | PendingKind::AuraDouble
        )
    }

    /// Whether an entry of this kind is an Aura, for the cards that ask
    /// whether you control one.
    ///
    /// The same four as [`at_end_of_turn`](Self::at_end_of_turn) today, and
    /// deliberately a separate question: Acceleration Aura is an Aura that
    /// ticks at the start of a turn, and an Aura the engine adds later need
    /// not be one at all.
    pub const fn is_aura(self) -> bool {
        self.at_end_of_turn()
    }
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
    /// Extra damage this player's Hero Power deals, for the one power that
    /// grows: Soul Immolation raises Collapsing Star by 1 every time it is
    /// cast after the first. Kept on the player rather than on the power
    /// because the power is a `CardId` in a shared, immutable table.
    pub hero_power_bonus: i16,
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
    /// Whether this player's *hero* has taken damage this turn, and whether
    /// its Health has moved at all (damage or healing). Two cards ask the two
    /// questions separately, and `friendly_damaged_turn` answers neither: it
    /// counts minions too. Both reset at this player's own turn start.
    pub hero_damaged_turn: bool,
    pub hero_health_moved_turn: bool,
    /// Whether this player has restored Health this turn, and whether they
    /// have Discovered. Both are printed conditions with no other home.
    pub hero_healed_turn: bool,
    pub discovered_turn: bool,
    /// How long the graveyard was when this player's turn began, so "that
    /// died this turn" is the slice past it.
    pub graveyard_at_turn_start: u8,
    /// Friendly minions that have died this turn.
    pub friendly_deaths_turn: u8,
    /// Cards this player has played this game that did not start in the deck
    /// they built -- generated, Discovered, shuffled in (Techysaurus).
    /// Saturating, and never reset: the card counts the whole game.
    pub cards_played_not_from_deck: u8,
    /// Whether the list this deck was built from held no spells at all
    /// (Hexmarshal). Read from the opening list once, in [`Player::new`],
    /// because "started with" is a question about that list and not about
    /// whatever is left in the deck by the time the card is played.
    pub deck_started_spelless: bool,
    /// The highest cost in the deck the player started with, capped at 255.
    ///
    /// Read by "if your deck only has cards that cost (3) or less"
    /// (Chef Neth'rek). Taken at construction for the same reason
    /// `deck_started_spelless` is: Start of Game fires after the mulligan, so
    /// by then `deck` is missing the three or four cards in the opening hand
    /// and a check made there would pass for a deck whose one expensive card
    /// happened to be dealt.
    pub deck_started_max_cost: u8,
    /// Waiting discount for the next Temporary card this player is given
    /// (Spelunker). Spent by the first one that arrives, whether it comes
    /// from a Discover, a battlecry or a Hero Power.
    pub next_temporary_discount: i16,
    /// How many times this player has Imbued their Hero Power.
    ///
    /// Every Blessing scales off this one number: the corpus writes each of
    /// them with `@` where the value belongs (`Summon a @/@ Plant Golem`),
    /// and the value is the Imbue count, with no ceiling. The printed tokens
    /// confirm the reading -- the Plant Golem is a 1/1, which is the count
    /// after the first Imbue. See `Game::imbue`.
    pub imbue_count: u8,
    /// The class Shadowed Informant is pointing at right now. It starts as
    /// this player's own class and swaps to a random other one at the end of
    /// each of their turns, so a copy played on the turn it arrives offers
    /// your own spells and a copy held offers someone else's.
    ///
    /// One value per player rather than per copy: two copies in hand show
    /// the same class as each other.
    pub informant_class: Class,
    /// The active Quest, as (card, progress). A Quest's own printed number
    /// is not stored here, since its `trigger` already knows it -- this is
    /// only ever read back by the same card that wrote it.
    pub quest: Option<(CardId, u8)>,
    /// The active Sidequest, in its own slot: Hearthstone allows one Quest
    /// and one Sidequest active at the same time, and Quest Hunter runs one
    /// of each.
    pub sidequest: Option<(CardId, u8)>,
    /// Spell schools cast on this player's previous turn, as a bitmask over
    /// [`crate::cards::School`] -- the same shape as `schools_cast_turn`,
    /// rolled over at end of turn.
    ///
    /// Kindred on a *spell* asks about the spell school rather than a tribe:
    /// the keyword reads "you played a card with the same minion type or
    /// spell school as this last turn", and a spell has no tribe to ask
    /// about. `played_races_last` answers the minion half; this answers the
    /// other one.
    pub schools_cast_last: u8,
    /// Which of this player's `graveyard` entries died *after* coming back
    /// through Reborn, as a bitmask over graveyard slots (Raith Van Geist).
    ///
    /// A bitmask rather than a second list because a `Game` is copied by
    /// value on every search node and the graveyard is already thirty-two
    /// slots wide; four bytes answer the question exactly for every slot the
    /// graveyard itself records. A death that fell past the graveyard's cap
    /// is not marked here either, so this can only ever read short, never
    /// long.
    pub reborn_dead: u32,
    /// The slice of `graveyard` that Slime 'em! put there: the start index in
    /// the low five bits, the length in the top three.
    ///
    /// Its Ectoplasm resummons "all friendly minions that were slimed", and
    /// naming them by graveyard position rather than by a copied list keeps
    /// the whole feature to one byte a player. Thirty-two graveyard slots
    /// need five bits and a board of seven needs three, so the two halves fit
    /// a byte exactly. The graveyard is
    /// append-only, so the slice keeps pointing at the same bodies for as
    /// long as it is held, and minions that died after the wipe sit past it
    /// and are not resummoned.
    pub slimed: u8,
    /// Set by Ruby Sanctum: the next Healing effect this turn deals its
    /// damage instead of healing, and spends this.
    pub heal_as_damage: bool,
    /// Which of the three Windrunner sisters this player has played this
    /// game, as three bits (Sylvanas, Alleria, Vereesa in that order).
    ///
    /// Each of the three asks about the other two -- "if you've played
    /// Alleria or Vereesa, repeat for each" -- so what has to be remembered
    /// is which, not how many, and three bits say it exactly.
    pub rangers_played: u8,
    /// Whether Sylvanas's Triumph has been cast this game ("if you've played
    /// another copy of this").
    pub triumph_cast: bool,
    /// Whether Tame Pet has replaced this player's future Animal Companions.
    pub tamed_pet: bool,
    /// The card Gemstone Hoarder made this player discard, for its own
    /// Deathrattle to hand back. `CardId(0)` for none, the same sentinel a
    /// vacated inline slot holds.
    pub hoarded: CardId,
    /// The 1-Cost minions this player has played this game, for Confront the
    /// Tol'vir.
    ///
    /// Seven slots because seven is a whole board: the card summons each of
    /// them and nothing can put an eighth body down, so a longer list could
    /// never be read to the end. A game that plays more than seven records
    /// the first seven, which is the way this can only ever read short.
    pub cheap_minions_played: Inline<CardId, MAX_BOARD>,
    /// How much stronger this player's Leylines are, how much cheaper, and
    /// how many extra times each one fires.
    ///
    /// Three separate cards raise each of the three -- Mystic Runesaber, Ley
    /// Walker and Surge Needle -- and The Arcanomicon offers a bigger version
    /// of any one of them. All of it is "this game", so none of it resets.
    /// The numbers each card adds come from the corpus; the Leylines' base
    /// values do not, and are attributed where they are used.
    pub leyline_bonus: u8,
    pub leyline_discount: u8,
    pub leyline_extra: u8,
    /// Damage this player has dealt with spells this turn, for the one card
    /// that is priced by it (Spellweaver's Brilliance). Saturating, and reset
    /// at the start of each of this player's turns.
    pub spell_damage_turn: u8,
    /// Mana crystals this player may hold above the usual ten.
    ///
    /// "Increase both players' maximum Mana by 5" (Ysera, Emerald Aspect) is
    /// the only thing that moves it, and it moves it for both sides at once.
    /// A ceiling rather than a grant: it raises what the natural ramp and
    /// every "gain a Mana Crystal" stop at, and hands out nothing itself.
    pub extra_crystals: u8,
    pub weapon: Option<Weapon>,
    pub hand: Inline<HandCard, MAX_HAND>,
    pub deck: Inline<DeckCard, MAX_DECK>,
    pub board: Inline<Permanent, MAX_BOARD>,
    pub secrets: Inline<CardId, MAX_SECRETS>,
    /// The hand this player kept after mulligan, captured once before Start
    /// of Game runs (The Fins Beyond Time). Plain card identities, not
    /// `HandCard`s: nothing has a cost delta, a Prepare lock or a mark yet
    /// at the moment this is taken.
    pub starting_hand: Inline<CardId, MAX_HAND>,
    /// The real hand, stashed while it is temporarily replaced by fresh
    /// copies of `starting_hand` (The Fins Beyond Time). `Some` only for the
    /// rest of the turn the swap happened on; restored and cleared at that
    /// same turn's end.
    pub swapped_hand: Option<Inline<HandCard, MAX_HAND>>,
    /// Minions played from hand, total across the whole game, never reset
    /// (Zee's Might: every fifth one triggers its own Battlecry twice).
    pub minions_played_total: u8,
    /// Whether this player's first minion this turn has already had Mug's
    /// Magic's discount applied. Reset every turn; the card itself is what
    /// gates *whether* the discount exists at all (Passive Hero Power, Turn
    /// 3+), so this only needs to say "not yet this turn".
    pub first_minion_discounted_turn: bool,
    /// Set for the rest of the game once Godfrey the Betrayer's Start of
    /// Game fires (it need never be played, only kept). While set,
    /// `Game::give_card` queues an overdraw here instead of burning it.
    pub godfrey_active: bool,
    /// Whether this player played a minion on their previous turn. Read by
    /// "if you didn't play a minion last turn" (Heartroot Stones); rolled
    /// forward from `minions_played_turn` at this player's own turn end.
    /// Extra Mana this player's minions cost, until their next turn ends.
    ///
    /// Set on the *opponent* by "enemy minions cost (2) more next turn"
    /// (Harsh Sentence). Cleared where every other per-turn discount is, at
    /// the taxed player's own turn end, so "next turn" is their turn and not
    /// the caster's.
    pub minion_tax: i16,
    /// Ebyssian: "Your Dragons have Rush this game." A flag on the player
    /// rather than an aura, because it outlives the body that granted it.
    pub dragons_have_rush: bool,
    /// How many cards this player has played for exactly two Mana this game.
    ///
    /// What was actually paid, not what was printed: a three-drop discounted
    /// to two was played for two. Two cards read it -- one to make eight-drops
    /// cheaper, one to shoot more often -- and both say "for 2 Mana".
    pub cards_played_for_two: u16,
    pub played_minion_last_turn: bool,
    pub minions_played_turn: bool,
    /// Whether this player's first Dragon this turn has already had Naralex,
    /// Herald of the Flights' discount applied. Reset every turn; whether
    /// the discount exists at all is a live board check (is Naralex still
    /// out?) in `Game::card_cost`, not a flag, since it can leave play.
    pub dragon_discounted_turn: bool,
    /// Cards Godfrey caught on their way to being burned, waiting for hand
    /// space; returned one at a time, discounted by 1, at this player's own
    /// `begin_turn`. Capped rather than unbounded: a game overdrawing more
    /// than this many cards past a full hand is not one Godfrey saves either
    /// way, and the cap keeps this a fixed handful of bytes on every game
    /// regardless of whether Godfrey is anywhere in play.
    pub overdrawn: Inline<CardId, 10>,
    /// Cards sent here by Irida Sinseeker's battlecry, drawn from two at a
    /// time at the start of this player's own turns. A third zone, not
    /// folded into `deck`, because a card here is not shuffled, not subject
    /// to fatigue, and not visible to anything that reads the deck (a
    /// Discover, an opponent's "look at their deck" effect).
    pub void: Inline<CardId, MAX_DECK>,
    /// Damage dealt to the opposing hero at the end of each of this player's
    /// turns, for the rest of the game (Alexandros Mograine). A field rather
    /// than a queued `Pending`, because `Pending` fires at the *start* of a
    /// turn and this is printed for the end of one.
    pub end_turn_burn: i16,
    /// Every friendly minion that has died this game, oldest first — the pool
    /// "Resurrect a minion that died this game" draws from, and the record
    /// "for each friendly minion that died this game" counts.
    ///
    /// Capped, like `overdrawn`, so a game stays a fixed handful of bytes.
    /// Thirty-two is more than a game of ordinary length reaches;
    /// `tests/graveyard.rs` holds the cap to that. Past the cap the pool
    /// stops growing while
    /// `deaths` keeps counting, so a card that counts stays right even where
    /// a card that resurrects would run out of pool.
    pub graveyard: Inline<CardId, GRAVEYARD>,
    /// How many friendly minions have died this game, including any the
    /// capped `graveyard` could not record. Saturating, so a card that counts
    /// deaths stays right even when the pool has stopped growing.
    pub deaths: u8,
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
            hero_power_bonus: 0,
            second_hero_power: None,
            second_hero_power_uses: 0,
            spell_tax_active: 0,
            spell_tax_pending: 0,
            pending: Inline::new(),
            friendly_damaged_turn: 0,
            hero_damaged_turn: false,
            hero_health_moved_turn: false,
            hero_healed_turn: false,
            discovered_turn: false,
            graveyard_at_turn_start: 0,
            friendly_deaths_turn: 0,
            cards_played_not_from_deck: 0,
            deck_started_spelless: deck
                .iter()
                .all(|c| c.def().kind() != Kind::Spell),
            deck_started_max_cost: deck
                .iter()
                .map(|c| c.def().cost.clamp(0, 255) as u8)
                .max()
                .unwrap_or(0),
            next_temporary_discount: 0,
            imbue_count: 0,
            informant_class: class,
            quest: None,
            sidequest: None,
            schools_cast_last: 0,
            reborn_dead: 0,
            slimed: 0,
            heal_as_damage: false,
            rangers_played: 0,
            triumph_cast: false,
            tamed_pet: false,
            hoarded: CardId(0),
            cheap_minions_played: Inline::new(),
            leyline_bonus: 0,
            leyline_discount: 0,
            leyline_extra: 0,
            spell_damage_turn: 0,
            extra_crystals: 0,
            weapon: None,
            hand: Inline::new(),
            deck: deck.iter().map(|&c| DeckCard::started(c)).collect(),
            board: Inline::new(),
            secrets: Inline::new(),
            starting_hand: Inline::new(),
            swapped_hand: None,
            minions_played_total: 0,
            first_minion_discounted_turn: false,
            godfrey_active: false,
            overdrawn: Inline::new(),
            graveyard: Inline::new(),
            end_turn_burn: 0,
            deaths: 0,
            dragon_discounted_turn: false,
            minion_tax: 0,
            dragons_have_rush: false,
            cards_played_for_two: 0,
            played_minion_last_turn: false,
            minions_played_turn: false,
            void: Inline::new(),
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

    /// The most crystals this player may hold, ten unless something raised it.
    #[inline]
    pub fn crystal_cap(&self) -> i16 {
        MAX_MANA + self.extra_crystals as i16
    }

    /// The graveyard slice Slime 'em! recorded, as `(start, length)`.
    ///
    /// `(0, 0)` when nothing is waiting, which is also what an Ectoplasm that
    /// has already been spent leaves behind.
    #[inline]
    pub fn slimed_slice(&self) -> (usize, usize) {
        ((self.slimed & 0x1f) as usize, (self.slimed >> 5) as usize)
    }

    /// Record a graveyard slice as slimed. `len` is clamped to the three bits
    /// it has, and a `start` past the graveyard's own width records nothing --
    /// there would be no slot to point at.
    #[inline]
    pub fn set_slimed(&mut self, start: usize, len: usize) {
        if start >= GRAVEYARD || len == 0 {
            self.slimed = 0;
            return;
        }
        self.slimed = (start as u8) | ((len.min(7) as u8) << 5);
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
    /// How many minions in play carry a [`Bonus`](crate::cards::Bonus) -- a
    /// continuous effect on themselves. Maintained by `recompute_auras`, and
    /// read on the damage and healing paths so a board with none of them pays
    /// one comparison rather than a recomputation.
    pub conditional: u8,
    /// Set while a Rewind is being resolved, so the replay cannot rewind
    /// itself. See `Game::apply`.
    pub rewinding: bool,
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
                marks: Marks::NONE,
                mana_spent_while_held: 0,
                atk: 0,
                hp: 0,
                gift: 0,
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
        // The number that justifies the whole design: a `Game` is copied
        // per search node, so if this grows past a few kilobytes the search
        // stops being cheap.
        //
        // The limit is deliberately close to the current size. Anything that
        // wants another byte per deck or hand card has to take its own
        // throughput measurement first -- `tools/ab-bench.sh` describes how.
        let n = size_of::<Game>();
        assert!(
            n < 2624,
            "Game is {n} bytes; it is meant to stay well under 3 KB"
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
