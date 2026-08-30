//! What the log adds up to: one game, as far as it can be seen.
//!
//! Deliberately only what the log states outright. A tracker that guesses at
//! what it cannot see would hand back confident advice about a board that is
//! not there, which is the one failure this project is built to avoid — so
//! everything here is either read from a line or left unknown, and the
//! command prints the picture it built beside the advice it gives.

use super::log::{EntityTag, Event, ZoneMove};
use tavernlab_core::cards::{CardId, Class, Keywords, by_id};

/// A minion the log has put on a board.
#[derive(Clone, Debug)]
pub struct Body {
    pub entity: u32,
    pub card: CardId,
    /// The turn counter when it entered this zone.
    ///
    /// A minion that landed this turn is still summoning sick, and one with
    /// Rush may trade but not go face. Advice drawn without it would swing
    /// with the minion that was only just put down.
    pub turn: u16,
    /// Attack and maximum Health as the log last stated them, or `None` while
    /// it has said nothing and the printed numbers still stand.
    ///
    /// `TAG_CHANGE` is written when a value *changes*, so silence means the
    /// card is what the corpus says it is -- and a buffed minion says so.
    pub atk: Option<i16>,
    pub hp: Option<i16>,
    pub damage: i16,
    /// Live keywords: the printed set, then whatever the log granted or took
    /// away on top of it. Started from the card so a body the log says
    /// nothing about is still its printed self.
    pub keywords: Keywords,
    pub frozen: bool,
    pub attacks: u8,
    /// What this copy costs right now, when the log has said. Discounts and
    /// taxes are already in that number; the printed cost is not.
    pub cost: Option<i16>,
    /// Spent for this turn. On a minion this is what `attacks` already says;
    /// on a Hero Power or a Location it is the whole of it.
    pub exhausted: bool,
    /// Uses left, for the entities that count them rather than health --
    /// Locations. `None` while the log has said nothing and the printed
    /// number stands.
    pub durability: Option<i16>,
}

impl Body {
    fn new(entity: u32, card: CardId, turn: u16) -> Body {
        Body {
            entity,
            card,
            turn,
            atk: None,
            hp: None,
            damage: 0,
            keywords: card.def().keywords,
            frozen: false,
            attacks: 0,
            cost: None,
            exhausted: false,
            durability: None,
        }
    }

    /// The card behind a face-down entity, once the log has shown it.
    ///
    /// The printed keywords come with it: a body that was `UNKNOWN ENTITY`
    /// started from an empty card and would otherwise keep an empty keyword
    /// set for the rest of the game.
    fn reveal(&mut self, card: CardId) {
        let granted = self.keywords;
        self.card = card;
        self.keywords = card.def().keywords;
        // Anything the log granted while it was face down still holds.
        self.keywords.insert(granted);
    }

    /// Attack and health right now, falling back to the printed numbers.
    pub fn stats(&self) -> (i16, i16) {
        let d = self.card.def();
        (
            self.atk.unwrap_or(d.atk),
            self.hp.unwrap_or(d.hp) - self.damage,
        )
    }

    fn apply(&mut self, what: EntityTag) {
        match what {
            EntityTag::Atk(n) => self.atk = Some(n),
            EntityTag::Health(n) => self.hp = Some(n),
            EntityTag::Damage(n) => self.damage = n,
            EntityTag::Attacks(n) => self.attacks = n,
            EntityTag::Cost(n) => self.cost = Some(n),
            EntityTag::Exhausted(on) => self.exhausted = on,
            EntityTag::Durability(n) => self.durability = Some(n),
            // Armor is a hero's. Dropped rather than guessed at: an armour
            // tag on a body is not a fact about that body.
            EntityTag::Armor(_) => {}
            EntityTag::Keyword("FROZEN", on) => self.frozen = on,
            EntityTag::Keyword(name, on) => {
                let Some(k) = keyword_of(name) else { return };
                if on {
                    self.keywords.insert(k);
                } else {
                    self.keywords.remove(k);
                }
            }
        }
    }
}

/// The log's name for a keyword, as this engine spells it.
fn keyword_of(tag: &str) -> Option<Keywords> {
    Some(match tag {
        "TAUNT" => Keywords::TAUNT,
        "DIVINE_SHIELD" => Keywords::DIVINE_SHIELD,
        "STEALTH" => Keywords::STEALTH,
        "CHARGE" => Keywords::CHARGE,
        "RUSH" => Keywords::RUSH,
        "WINDFURY" => Keywords::WINDFURY,
        "LIFESTEAL" => Keywords::LIFESTEAL,
        "POISONOUS" => Keywords::POISONOUS,
        "REBORN" => Keywords::REBORN,
        // The game's name for Elusive.
        "CANT_BE_TARGETED_BY_SPELLS" => Keywords::ELUSIVE,
        "CANT_ATTACK" => Keywords::CANT_ATTACK,
        "IMMUNE" => Keywords::IMMUNE,
        _ => return None,
    })
}

/// A weapon in play, as far as the log states it.
///
/// Separate from [`Body`] because a weapon is not on a board and has
/// durability where a minion has health -- and because the position needs to
/// find "the weapon" rather than search a list for it.
#[derive(Clone, Copy, Debug)]
pub struct Weapon {
    pub entity: u32,
    pub card: CardId,
    /// Attack and durability as the log last stated them, or `None` while it
    /// has said nothing and the printed numbers still stand. The same rule
    /// [`Body`] follows: `TAG_CHANGE` is written when a value *changes*.
    pub atk: Option<i16>,
    pub durability: Option<i16>,
}

impl Weapon {
    /// Attack and durability right now, falling back to the printed card.
    ///
    /// Falling back rather than assuming: a weapon the log has said nothing
    /// about is its printed self, and one it has spoken about is what it
    /// said. Neither case invents a number.
    pub fn stats(&self) -> (i16, i16) {
        let d = self.card.def();
        (self.atk.unwrap_or(d.atk), self.durability.unwrap_or(d.dur))
    }

    fn apply(&mut self, what: EntityTag) {
        match what {
            EntityTag::Atk(n) => self.atk = Some(n),
            // A weapon's durability arrives as `DURABILITY` on the cards that
            // print it that way and as `HEALTH` on the ones that do not.
            // Both mean the swings it has left, so both are read as that.
            EntityTag::Durability(n) | EntityTag::Health(n) => self.durability = Some(n),
            _ => {}
        }
    }
}

/// A secret in its own zone.
///
/// `card` is `None` for the opponent's, and that is the honest state rather
/// than a gap to fill: a secret is set face down, so the log names yours and
/// says only that theirs exists. What is done with each is different -- see
/// the plan.
#[derive(Clone, Copy, Debug)]
pub struct Secret {
    pub entity: u32,
    pub card: Option<CardId>,
}

/// A hero, as far as the log states it.
#[derive(Clone, Copy, Debug)]
pub struct Hero {
    pub entity: Option<u32>,
    /// Maximum Health. Thirty until a line says otherwise, which is what
    /// every hero starts at and what a hero card changes.
    pub max_hp: i16,
    pub damage: i16,
    pub armor: i16,
    /// Swings taken this turn. A hero that has already attacked cannot
    /// attack again, and a plan that offers the swing twice is offering one
    /// that does not exist.
    pub attacks: u8,
}

impl Default for Hero {
    fn default() -> Hero {
        Hero {
            entity: None,
            max_hp: 30,
            damage: 0,
            armor: 0,
            attacks: 0,
        }
    }
}

impl Hero {
    /// Health, floored at zero.
    ///
    /// The killing blow's `DAMAGE` tag is the whole hit, not the part that
    /// fit, so a dead hero's raw number goes below zero. Zero is what "dead"
    /// means here; the game being over is said separately.
    pub fn health(&self) -> i16 {
        (self.max_hp - self.damage).max(0)
    }

    fn apply(&mut self, what: EntityTag) {
        match what {
            EntityTag::Health(n) => self.max_hp = n,
            EntityTag::Damage(n) => self.damage = n,
            EntityTag::Armor(n) => self.armor = n,
            EntityTag::Attacks(n) => self.attacks = n,
            _ => {}
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Tracker {
    /// The player number whose client wrote this log, once a FRIENDLY line
    /// has named it.
    pub me: Option<u8>,
    /// My battletag, as the `TAG_CHANGE` lines spell it. The log never says
    /// which of the two names is the one holding it, so this is supplied
    /// (`--me`, `HS_ME`) when it cannot be learned. Without it the mana and
    /// whose-turn-it-is lines cannot be attributed and are left unknown
    /// rather than guessed.
    pub me_name: Option<String>,
    /// Whether `me_name` was read off the log rather than supplied. Printed,
    /// because a name the watcher worked out for itself is a claim, and a
    /// reader should be able to see which name it settled on.
    pub me_learned: bool,
    pub classes: [Option<Class>; 2],
    /// My opening hand, in the order the log dealt it.
    pub opening: Vec<CardId>,
    /// My hand right now.
    pub hand: Vec<Body>,
    pub board: [Vec<Body>; 2],
    /// Both heroes: health, armour, and the entity the log names them by.
    pub heroes: [Hero; 2],
    /// The weapon each hero has equipped, when the log has shown one.
    pub weapons: [Option<Weapon>; 2],
    /// Secrets in play, yours named and theirs not.
    pub secrets: [Vec<Secret>; 2],
    /// Each side's Hero Power entity, once a zone line has named it. Carried
    /// for the one tag that matters: whether it has been used this turn.
    pub hero_powers: [Option<Body>; 2],
    /// Every player name the log has used on a line that needs one, in the
    /// order they first appeared. Printed when `me_name` matched none of
    /// them, because the fix is to pass one of these and the user cannot
    /// guess which spelling the client chose.
    pub names: Vec<String>,
    /// Cards each player has put into play or cast, oldest first. What the
    /// opponent read is built from.
    pub played: [Vec<CardId>; 2],
    pub turn: u16,
    pub my_turn: bool,
    /// Corpses banked, for me. `None` until a `CORPSES` line could be
    /// attributed, which is not the same as zero: a Death Knight plan drawn
    /// at zero when the log said three spends nothing it actually has.
    pub corpses: Option<i16>,
    /// Mana crystals and mana spent, for me. `None` until a `RESOURCES` line
    /// could be attributed -- see `me_name`.
    pub crystals: Option<i16>,
    pub spent: i16,
    pub over: bool,
    /// Whether I won, once a `PLAYSTATE` line said so about a player the
    /// battletag could be matched to. `None` on a game the log never
    /// resolved, and on one where `--me` was never supplied.
    pub won: Option<bool>,
    /// Whether a `CURRENT_PLAYER` line was ever attributed to a player.
    /// While this is false, `my_turn` is not an answer but a default.
    pub turn_read: bool,
    /// True once the mulligan is done and the game proper has started.
    ///
    /// Driven by `STEP=MAIN_READY`, not by the turn counter: `TURN=1` is set
    /// inside `CREATE_GAME`, before any card is dealt, so a turn counter of
    /// one does not mean the opening has been handed out.
    pub started: bool,
}

fn card_of(card_id: &str) -> Option<CardId> {
    if card_id.is_empty() {
        return None;
    }
    by_id(card_id)
}

impl Tracker {
    pub fn new(me_name: Option<String>) -> Tracker {
        Tracker {
            me_name,
            ..Tracker::default()
        }
    }

    fn index(&self, player: u8) -> Option<usize> {
        let me = self.me?;
        Some(if player == me { 0 } else { 1 })
    }

    /// My side, then theirs — `0` is always the player holding the log.
    pub fn opponent_class(&self) -> Option<Class> {
        self.classes[1]
    }

    pub fn my_class(&self) -> Option<Class> {
        self.classes[0]
    }

    /// Whether I was on the draw, which the log says by dealing me the Coin.
    ///
    /// `None` before the opening hand is complete: an empty opening hand is
    /// not "no Coin", it is "not dealt yet".
    /// Whose turn it is: read from the log, else worked out, else unknown.
    ///
    /// The log states it on a `CURRENT_PLAYER` line, but only usably when
    /// that line carries a player number or a name that has been matched to
    /// one. A client that writes those lines as a bare battletag and never
    /// as a bracketed descriptor gives neither, and then the turn was never
    /// attributed at all -- which used to mean no turn plan for the whole
    /// game, on every turn, in silence.
    ///
    /// The opening hand answers it anyway, by two rules rather than a guess:
    /// The Coin is "granted at the start of each game to whichever player is
    /// selected to go second", and the second player takes the even-numbered
    /// turns -- the wiki's own turn limit note counts them, "Player 1 has 45
    /// complete turns (turn 1, 3, 5...), while Player 2 has 44 (turn 2, 4,
    /// 6...)". So a Coin in the opening hand says which parity is mine.
    ///
    /// `None` only when the opening was never seen either, which is the
    /// watcher having been started in the middle of a game.
    pub fn whose_turn(&self) -> Option<bool> {
        if self.turn_read {
            return Some(self.my_turn);
        }
        let coin = self.had_coin()?;
        (self.turn > 0).then_some((self.turn % 2 == 0) == coin)
    }

    pub fn had_coin(&self) -> Option<bool> {
        if self.opening.is_empty() {
            return None;
        }
        Some(self.opening.iter().any(|c| c.name() == "The Coin"))
    }

    pub fn mana_left(&self) -> Option<i16> {
        Some((self.crystals? - self.spent).max(0))
    }

    /// Whether a name the log used is the one `--me` named.
    ///
    /// Exact first. The fallback compares only the part before `#`, without
    /// case: the client does not always write the numeric half, and a
    /// battletag that is right except for that is not worth losing every
    /// mana line over. Ambiguous only if both players share a base name.
    fn is_me(&self, name: &str) -> bool {
        let Some(mine) = self.me_name.as_deref() else {
            return false;
        };
        if mine == name {
            return true;
        }
        let base = |s: &str| s.split('#').next().unwrap_or(s).to_ascii_lowercase();
        !name.is_empty() && base(mine) == base(name)
    }

    /// Whether a player line is about me, by number where the line carries
    /// one and by name where it does not.
    ///
    /// The number is the surer of the two and needs nothing matched first,
    /// so it is asked first; the name is what a bare-battletag line leaves.
    fn mine(&self, name: &str, player: Option<u8>) -> bool {
        match (player, self.me) {
            (Some(p), Some(me)) => p == me,
            _ => self.is_me(name),
        }
    }

    /// Work out which battletag is yours, from a line that says so.
    ///
    /// The log never states it in words, but it does state it in numbers:
    /// zone lines say FRIENDLY or OPPOSING and carry `player=N`, which is
    /// where `me` comes from, and a player's `TAG_CHANGE` written as a
    /// bracketed descriptor carries both the name and the same `player=N`.
    /// Put together, they name you.
    ///
    /// This is what `--me` was for. The flag still wins when it is given --
    /// a person overriding a guess should not have to argue with it -- and
    /// stays necessary for a log that only ever writes the bare-battletag
    /// form, which carries no player number at all.
    fn learn_me(&mut self, name: &str, player: Option<u8>) {
        if self.me_name.is_some() || name.is_empty() {
            return;
        }
        if let (Some(p), Some(me)) = (player, self.me)
            && p == me
        {
            self.me_name = Some(name.to_string());
            self.me_learned = true;
        }
    }

    fn note_name(&mut self, name: &str) {
        if !name.is_empty() && !self.names.iter().any(|n| n == name) {
            self.names.push(name.to_string());
        }
    }

    pub fn feed(&mut self, ev: Event) {
        match ev {
            Event::NewGame => *self = Tracker::new(self.me_name.clone()),
            Event::Turn(t) => {
                self.turn = t;
                // Turn 1 is written at CREATE_GAME, before the opening is
                // dealt. Turn 2 means someone has already taken a turn, so
                // the mulligan is over even if MAIN_READY was never seen
                // (synthetic logs, a truncated file).
                if t > 1 {
                    self.started = true;
                }
            }
            Event::Started => self.started = true,
            Event::Result {
                player_name,
                player,
                won,
            } => {
                self.note_name(&player_name);
                self.learn_me(&player_name, player);
                // The line names one player. Mine says it straight; theirs
                // says the opposite, and only once a name has been matched --
                // without `--me` neither is attributable and the game is
                // recorded as unresolved rather than as a guess.
                if self.me_name.is_some() {
                    self.won = Some(if self.is_me(&player_name) { won } else { !won });
                }
                self.over = true;
            }
            Event::Corpses {
                player_name,
                player,
                value,
            } => {
                self.note_name(&player_name);
                self.learn_me(&player_name, player);
                if self.mine(&player_name, player) {
                    self.corpses = Some(value);
                }
            }
            Event::CurrentPlayer {
                player_name,
                player,
                current,
            } => {
                self.learn_me(&player_name, player);
                self.note_name(&player_name);
                // A name is learnable here even with no number on the line.
                // The opening hand says whose turn this is -- see
                // `whose_turn` -- and a `CURRENT_PLAYER value=1` names the
                // player whose turn it is. Put together they name you, which
                // is the bridge between the zone lines that carry a player
                // number and the Power lines that carry only a battletag.
                //
                // Only the positive direction: "it is my turn and this line
                // says who is current" identifies you. "It is their turn"
                // would identify you only by elimination, and a log that has
                // shown one name so far would then pin the wrong one.
                if self.me_name.is_none()
                    && current
                    && self.whose_turn() == Some(true)
                    && !player_name.is_empty()
                {
                    self.me_name = Some(player_name.clone());
                    self.me_learned = true;
                }
                // The player number first, when the line carries one: it
                // says whose turn it is without anyone's name being matched.
                if let (Some(p), Some(me)) = (player, self.me) {
                    if p == me {
                        self.my_turn = current;
                        self.turn_read = true;
                    } else if current {
                        self.my_turn = false;
                        self.turn_read = true;
                    }
                    return;
                }
                if self.me_name.is_none() {
                    return;
                }
                if self.is_me(&player_name) {
                    self.my_turn = current;
                    self.turn_read = true;
                } else if current {
                    self.my_turn = false;
                    self.turn_read = true;
                }
            }
            Event::Resources {
                player_name,
                player,
                total,
                used,
            } => {
                self.note_name(&player_name);
                self.learn_me(&player_name, player);
                // Only my own crystals are actionable, and telling the two
                // players apart needs the name.
                if !self.is_me(&player_name) {
                    return;
                }
                if total >= 0 {
                    self.crystals = Some(total);
                }
                if used >= 0 {
                    self.spent = used;
                }
            }
            Event::Reveal { entity, card_id } => {
                if let Some(card) = card_of(&card_id) {
                    for side in 0..2 {
                        for b in self.board[side].iter_mut() {
                            if b.entity == entity {
                                b.reveal(card);
                            }
                        }
                    }
                    for side in 0..2 {
                        for sec in self.secrets[side].iter_mut() {
                            if sec.entity == entity {
                                sec.card = Some(card);
                            }
                        }
                    }
                    for b in self.hand.iter_mut() {
                        if b.entity == entity {
                            b.reveal(card);
                        }
                    }
                }
            }
            Event::Tag { entity, what } => {
                for h in self.heroes.iter_mut() {
                    if h.entity == Some(entity) {
                        h.apply(what);
                        return;
                    }
                }
                for w in self.weapons.iter_mut().flatten() {
                    if w.entity == entity {
                        w.apply(what);
                        return;
                    }
                }
                for p in self.hero_powers.iter_mut().flatten() {
                    if p.entity == entity {
                        p.apply(what);
                        return;
                    }
                }
                for side in 0..2 {
                    for b in self.board[side].iter_mut() {
                        if b.entity == entity {
                            b.apply(what);
                            return;
                        }
                    }
                }
                for b in self.hand.iter_mut() {
                    if b.entity == entity {
                        b.apply(what);
                        return;
                    }
                }
            }
            Event::Zone(m) => self.zone(m),
        }
    }

    fn zone(&mut self, m: ZoneMove) {
        let ZoneMove {
            entity,
            player,
            mine,
            ..
        } = m;
        let kind = m.kind.as_deref();
        let zone = m.zone.as_str();
        if mine && self.me.is_none() {
            self.me = Some(player);
        }
        let Some(i) = self.index(player) else { return };
        let card = card_of(&m.card_id);

        // A hero landing is how each side's class is announced.
        if kind == Some("Hero") {
            if let Some(c) = card
                && c.def().class() != Class::Neutral
            {
                self.classes[i] = Some(c.def().class());
            }
            // Which entity the health and armour lines will be about. A hero
            // card replaces the starting hero, and the new entity is the one
            // that counts from here on -- but only a different entity starts
            // over, so a hero line repeated for the same hero does not throw
            // away the damage already read.
            if self.heroes[i].entity != Some(entity) {
                self.heroes[i] = Hero {
                    entity: Some(entity),
                    ..Hero::default()
                };
            }
            return;
        }
        // The Hero Power's own entity, so the `EXHAUSTED` written on it can
        // be found. Which power it is comes from the class; whether it has
        // been used this turn is only knowable from this line's id.
        if kind == Some("Hero Power") {
            self.hero_powers[i] = Some(Body::new(entity, card.unwrap_or_default(), self.turn));
            return;
        }
        if kind.is_some() && kind != Some("Weapon") {
            return; // anything else parenthesised.
        }

        // Leaving a zone: drop it from wherever it was. A weapon leaves when
        // it breaks, and a broken weapon is no swings rather than a stale one;
        // a secret leaves when it fires, and a spent secret is not a threat.
        self.board[i].retain(|b| b.entity != entity);
        self.secrets[i].retain(|s| s.entity != entity);
        if self.weapons[i].is_some_and(|w| w.entity == entity) {
            self.weapons[i] = None;
        }
        if i == 0 {
            self.hand.retain(|b| b.entity != entity);
        }

        match zone {
            "HAND" if i == 0 => {
                if let Some(c) = card {
                    self.hand.push(Body::new(entity, c, self.turn));
                    if !self.started {
                        self.opening.push(c);
                    }
                }
            }
            "PLAY" => {
                if let Some(c) = card {
                    match c.def().kind() {
                        // A Location takes a board slot the way a minion
                        // does, and the engine offers `UseLocation` for one
                        // that is in play -- so leaving them out was leaving
                        // a whole play out of the turn.
                        tavernlab_core::cards::Kind::Minion
                        | tavernlab_core::cards::Kind::Location => {
                            self.board[i].push(Body::new(entity, c, self.turn));
                        }
                        // What the card is, not what the line called it. The
                        // client marks a weapon `(Weapon)` and the branch
                        // above lets that through, but the corpus already
                        // knows the card is a weapon -- so this reads
                        // correctly whether or not the note is written, and
                        // no line shape has to be assumed.
                        tavernlab_core::cards::Kind::Weapon => {
                            self.weapons[i] = Some(Weapon {
                                entity,
                                card: c,
                                atk: None,
                                durability: None,
                            });
                        }
                        _ => {}
                    }
                    self.played[i].push(c);
                    if c.def().class() != Class::Neutral && self.classes[i].is_none() {
                        self.classes[i] = Some(c.def().class());
                    }
                }
            }
            // Set face down. Yours carries a card id because it is your own
            // client writing the log; theirs does not, and is kept as the one
            // thing the log did say -- that there is one.
            "SECRET" => {
                self.secrets[i].push(Secret { entity, card });
                if let Some(c) = card {
                    self.played[i].push(c);
                    if c.def().class() != Class::Neutral && self.classes[i].is_none() {
                        self.classes[i] = Some(c.def().class());
                    }
                }
            }
            // A spell goes straight to the graveyard, and is still a card
            // they played: it is what the opponent read is built from.
            "GRAVEYARD" => {
                if let Some(c) = card
                    && c.def().kind() == tavernlab_core::cards::Kind::Spell
                {
                    self.played[i].push(c);
                    if c.def().class() != Class::Neutral && self.classes[i].is_none() {
                        self.classes[i] = Some(c.def().class());
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::watch_mod::log::parse;

    fn feed(t: &mut Tracker, lines: &[&str]) {
        for l in lines {
            if let Some(ev) = parse(l) {
                t.feed(ev);
            }
        }
    }

    #[test]
    fn the_first_friendly_line_decides_which_player_i_am() {
        let mut t = Tracker::new(None);
        feed(&mut t, &[
            "[Zone] [entityName=Chillwind Yeti id=42 zone=DECK zonePos=0 \
             cardId=CS2_182 player=2] zone from OPPOSING DECK -> FRIENDLY HAND",
        ]);
        assert_eq!(t.me, Some(2));
        assert_eq!(t.hand.len(), 1);
        assert_eq!(t.opening.len(), 1, "before the first turn it is the opening hand");
    }

    #[test]
    fn heroes_announce_both_classes() {
        let mut t = Tracker::new(None);
        feed(&mut t, &[
            "[Zone] [entityName=Jaina Proudmoore id=64 zone=PLAY zonePos=0 \
             cardId=HERO_08 player=1] zone from  -> FRIENDLY PLAY (Hero)",
            "[Zone] [entityName=Garrosh Hellscream id=65 zone=PLAY zonePos=0 \
             cardId=HERO_01 player=2] zone from  -> OPPOSING PLAY (Hero)",
        ]);
        assert_eq!(t.my_class(), Some(Class::Mage));
        assert_eq!(t.opponent_class(), Some(Class::Warrior));
    }

    #[test]
    fn a_minion_played_leaves_hand_and_joins_the_board() {
        let mut t = Tracker::new(None);
        feed(&mut t, &[
            "[Zone] [entityName=Chillwind Yeti id=42 zone=DECK zonePos=0 \
             cardId=CS2_182 player=1] zone from FRIENDLY DECK -> FRIENDLY HAND",
            "D [Power] TAG_CHANGE Entity=GameEntity tag=TURN value=1",
            "[Zone] [entityName=Chillwind Yeti id=42 zone=HAND zonePos=1 \
             cardId=CS2_182 player=1] zone from FRIENDLY HAND -> FRIENDLY PLAY",
        ]);
        assert!(t.hand.is_empty(), "it left hand");
        assert_eq!(t.board[0].len(), 1);
        assert_eq!(t.board[0][0].card.name(), "Chillwind Yeti");
        assert_eq!(t.played[0].len(), 1);
    }

    #[test]
    fn the_opening_hand_stops_growing_once_the_game_starts() {
        let mut t = Tracker::new(None);
        feed(&mut t, &[
            "[Zone] [entityName=Wisp id=1 zone=DECK zonePos=0 cardId=CS2_231 \
             player=1] zone from FRIENDLY DECK -> FRIENDLY HAND",
            "D [Power] GameState.DebugPrintPower() -     TAG_CHANGE Entity=1 tag=TURN value=1",
            "[Zone] [entityName=The Coin id=3 zone=DECK zonePos=0 cardId=GAME_005 \
             player=1] zone from  -> FRIENDLY HAND",
            "D [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity \
             tag=STEP value=MAIN_READY",
            "[Zone] [entityName=Fireball id=2 zone=DECK zonePos=0 cardId=CS2_029 \
             player=1] zone from FRIENDLY DECK -> FRIENDLY HAND",
        ]);
        assert_eq!(
            t.opening.len(),
            2,
            "TURN=1 is set at CREATE_GAME, before any card is dealt; \
             the opening is everything until MAIN_READY"
        );
        assert_eq!(t.opening[1].name(), "The Coin");
        assert_eq!(t.had_coin(), Some(true));
        assert_eq!(t.hand.len(), 3, "but the hand keeps up after mulligan");
        assert!(t.started);
    }

    #[test]
    fn my_mana_is_read_and_the_opponents_is_not() {
        let mut t = Tracker::new(Some("Me#1".into()));
        feed(&mut t, &[
            "D [Power] TAG_CHANGE Entity=Me#1 tag=RESOURCES value=7",
            "D [Power] TAG_CHANGE Entity=Me#1 tag=RESOURCES_USED value=3",
            "D [Power] TAG_CHANGE Entity=Them#2 tag=RESOURCES value=10",
        ]);
        assert_eq!(t.crystals, Some(7));
        assert_eq!(t.mana_left(), Some(4));
    }

    #[test]
    fn the_coin_says_which_turns_are_mine_when_the_log_will_not() {
        // A client that writes CURRENT_PLAYER as a bare battletag carries no
        // player number, so no name is ever matched to one. The opening hand
        // answers it: The Coin goes to the player on the draw, and that
        // player takes the even turns.
        let mut t = Tracker::new(None);
        feed(&mut t, &[
            "D 09:00:00.0 [Power] GameState.DebugPrintPower() - CREATE_GAME",
            "D 09:00:00.2 [Zone] ZoneChangeList.ProcessChanges() - id=3 local=False [entityName=The Coin id=11 zone=DECK zonePos=0 cardId=GAME_005 player=1] zone from  -> FRIENDLY HAND",
            "D 09:00:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=TURN value=2",
        ]);
        assert_eq!(t.me_name, None, "the name was never learned");
        assert_eq!(t.whose_turn(), Some(true), "but turn two is the Coin holder's");

        t.feed(crate::watch_mod::log::Event::Turn(3));
        assert_eq!(t.whose_turn(), Some(false), "and turn three is not");
    }

    #[test]
    fn the_turn_you_can_place_names_the_player_who_is_taking_it() {
        // The bridge between the two halves of the log: zone lines carry a
        // player number and no name, Power lines carry a name and no number.
        // Knowing whose turn it is from the opening hand puts them together.
        let mut t = Tracker::new(None);
        feed(&mut t, &[
            "D 09:00:00.0 [Power] GameState.DebugPrintPower() - CREATE_GAME",
            "D 09:00:00.2 [Zone] ZoneChangeList.ProcessChanges() - id=3 local=False [entityName=The Coin id=11 zone=DECK zonePos=0 cardId=GAME_005 player=1] zone from  -> FRIENDLY HAND",
            "D 09:00:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=TURN value=2",
            "D 09:00:01.1 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=xror#21652 tag=CURRENT_PLAYER value=1",
            "D 09:00:01.3 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=xror#21652 tag=RESOURCES value=1",
            "D 09:00:01.4 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=xror#21652 tag=RESOURCES_USED value=0",
        ]);
        assert_eq!(t.me_name.as_deref(), Some("xror#21652"));
        assert!(t.me_learned, "worked out rather than supplied");
        assert_eq!(t.mana_left(), Some(1), "so the mana lines attribute too");
    }

    #[test]
    fn the_opponents_turn_does_not_name_you_by_elimination() {
        // "It is their turn and this line says who is current" identifies
        // them, not you -- and with one name seen so far, eliminating would
        // pin the wrong one.
        let mut t = Tracker::new(None);
        feed(&mut t, &[
            "D 09:00:00.0 [Power] GameState.DebugPrintPower() - CREATE_GAME",
            "D 09:00:00.2 [Zone] ZoneChangeList.ProcessChanges() - id=3 local=False [entityName=The Coin id=11 zone=DECK zonePos=0 cardId=GAME_005 player=1] zone from  -> FRIENDLY HAND",
            "D 09:00:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=TURN value=1",
            "D 09:00:01.1 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=starkalpha#2221 tag=CURRENT_PLAYER value=1",
        ]);
        assert_eq!(t.me_name, None, "turn one is not the Coin holder's");
    }

    #[test]
    fn no_coin_means_the_odd_turns_are_mine() {
        let mut t = Tracker::new(None);
        feed(&mut t, &[
            "D 09:00:00.0 [Power] GameState.DebugPrintPower() - CREATE_GAME",
            "D 09:00:00.2 [Zone] ZoneChangeList.ProcessChanges() - id=3 local=False [entityName=Corpse Cannon id=12 zone=DECK zonePos=0 cardId=JAIL_450 player=1] zone from  -> FRIENDLY HAND",
            "D 09:00:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=TURN value=1",
        ]);
        assert_eq!(t.whose_turn(), Some(true), "no Coin is going first");
        t.feed(crate::watch_mod::log::Event::Turn(2));
        assert_eq!(t.whose_turn(), Some(false));
    }

    #[test]
    fn a_log_read_from_the_middle_admits_it_does_not_know() {
        // No opening hand to reason from, and no attributable CURRENT_PLAYER.
        let mut t = Tracker::new(None);
        t.feed(crate::watch_mod::log::Event::Turn(5));
        assert_eq!(t.whose_turn(), None);
    }

    #[test]
    fn a_battletag_without_its_numbers_is_still_me() {
        // The client does not always write the `#12345` half.
        let mut t = Tracker::new(Some("xror#21652".into()));
        feed(&mut t, &[
            "D [Power] TAG_CHANGE Entity=xror tag=RESOURCES value=6",
            "D [Power] TAG_CHANGE Entity=xror tag=RESOURCES_USED value=2",
            "D [Power] TAG_CHANGE Entity=SomeoneElse#9 tag=RESOURCES value=10",
        ]);
        assert_eq!(t.mana_left(), Some(4));
        assert_eq!(
            t.names,
            vec!["xror".to_string(), "SomeoneElse#9".to_string()],
            "and both names are kept, to print when nothing matched"
        );
    }

    #[test]
    fn the_log_overrides_the_printed_stats() {
        let mut t = Tracker::new(None);
        feed(&mut t, &[
            "[Zone] [entityName=Chillwind Yeti id=42 zone=HAND zonePos=1 \
             cardId=CS2_182 player=1] zone from FRIENDLY HAND -> FRIENDLY PLAY",
            "D [Power] TAG_CHANGE Entity=[entityName=Chillwind Yeti id=42 \
             zone=PLAY zonePos=1 cardId=CS2_182 player=1] tag=ATK value=6",
            "D [Power] TAG_CHANGE Entity=[entityName=Chillwind Yeti id=42 \
             zone=PLAY zonePos=1 cardId=CS2_182 player=1] tag=DAMAGE value=2",
            "D [Power] TAG_CHANGE Entity=[entityName=Chillwind Yeti id=42 \
             zone=PLAY zonePos=1 cardId=CS2_182 player=1] tag=TAUNT value=1",
        ]);
        let b = &t.board[0][0];
        assert_eq!(b.stats(), (6, 3), "a 4/5 buffed to 6 attack with 2 damage on it");
        assert!(b.keywords.has(Keywords::TAUNT), "granted, not printed");
    }

    #[test]
    fn a_silenced_keyword_goes_away() {
        let mut t = Tracker::new(None);
        const PLAY: &str = "[Zone] [entityName=Goldshire Footman id=7 \
             zone=HAND zonePos=1 cardId=CS1_042 player=1] zone from \
             FRIENDLY HAND -> FRIENDLY PLAY";
        feed(&mut t, &[PLAY]);
        assert!(
            t.board[0][0].keywords.has(Keywords::TAUNT),
            "Goldshire Footman prints Taunt, so the silence below has \
             something to take away"
        );
        feed(&mut t, &[
            "D [Power] TAG_CHANGE Entity=[entityName=Goldshire Footman id=7 \
             zone=PLAY zonePos=1 cardId=CS1_042 player=1] tag=TAUNT value=0",
        ]);
        assert!(!t.board[0][0].keywords.has(Keywords::TAUNT));
    }

    #[test]
    fn the_heroes_health_and_armour_are_read() {
        let mut t = Tracker::new(None);
        feed(&mut t, &[
            "[Zone] [entityName=Jaina Proudmoore id=64 zone=PLAY zonePos=0 \
             cardId=HERO_08 player=1] zone from  -> FRIENDLY PLAY (Hero)",
            "[Zone] [entityName=Garrosh Hellscream id=65 zone=PLAY zonePos=0 \
             cardId=HERO_01 player=2] zone from  -> OPPOSING PLAY (Hero)",
            "D [Power] TAG_CHANGE Entity=64 tag=DAMAGE value=8",
            "D [Power] TAG_CHANGE Entity=65 tag=ARMOR value=5",
        ]);
        assert_eq!(t.heroes[0].health(), 22);
        assert_eq!(t.heroes[0].armor, 0);
        assert_eq!(t.heroes[1].health(), 30);
        assert_eq!(t.heroes[1].armor, 5);
    }

    #[test]
    fn the_battletag_is_read_off_the_log_rather_than_asked_for() {
        // FRIENDLY zone lines say which player number you are; a player's
        // TAG_CHANGE written as a descriptor carries the same number next to
        // the name. Between them the log names you, and `--me` becomes an
        // override rather than a requirement.
        let mut t = Tracker::new(None);
        t.feed(parse(
            "D [Zone] ZoneChangeList.ProcessChanges() - id=1 local=False \
             [entityName=Jaina Proudmoore id=64 zone=PLAY zonePos=0 \
             cardId=HERO_08 player=1] zone from  -> FRIENDLY PLAY (Hero)",
        )
        .expect("a zone line"));
        assert_eq!(t.me, Some(1), "FRIENDLY named the player number");
        assert!(t.me_name.is_none(), "and nothing has named the player yet");

        t.feed(
            parse(
                "D [Power] TAG_CHANGE Entity=[entityName=xror id=2 zone=PLAY \
                 zonePos=0 cardId= player=1] tag=RESOURCES value=7",
            )
            .expect("a resources line"),
        );
        assert_eq!(t.me_name.as_deref(), Some("xror"));
        assert!(t.me_learned);
        assert_eq!(t.crystals, Some(7), "and the mana is attributed");
    }

    #[test]
    fn the_other_players_descriptor_is_not_mistaken_for_yours() {
        let mut t = Tracker::new(None);
        t.feed(parse(
            "D [Zone] ZoneChangeList.ProcessChanges() - id=1 local=False \
             [entityName=Jaina Proudmoore id=64 zone=PLAY zonePos=0 \
             cardId=HERO_08 player=1] zone from  -> FRIENDLY PLAY (Hero)",
        )
        .expect("a zone line"));
        t.feed(
            parse(
                "D [Power] TAG_CHANGE Entity=[entityName=them id=3 zone=PLAY \
                 zonePos=0 cardId= player=2] tag=RESOURCES value=9",
            )
            .expect("a resources line"),
        );
        assert!(t.me_name.is_none(), "player 2 is not player 1");
        assert_eq!(t.crystals, None, "and their mana is not yours");
    }

    #[test]
    fn a_new_game_wipes_the_last_one() {
        let mut t = Tracker::new(None);
        feed(&mut t, &[
            "[Zone] [entityName=Wisp id=1 zone=DECK zonePos=0 cardId=CS2_231 \
             player=1] zone from FRIENDLY DECK -> FRIENDLY HAND",
            "D [Power] CREATE_GAME",
        ]);
        assert!(t.hand.is_empty());
        assert!(t.me.is_none());
    }
}
