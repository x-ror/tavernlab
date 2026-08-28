//! Reading Hearthstone's own log files.
//!
//! The game writes these itself, when `log.config` asks it to. Nothing here
//! reads the game's memory, intercepts its traffic or types into it — the
//! same line every deck tracker stays on, and the one `docs/DESIGN.md` drew
//! for this project.
//!
//! Hand-rolled rather than regex, because the workspace has no dependencies
//! and a scanner for `key=value` inside square brackets is smaller than the
//! engine that would parse the pattern. The line shapes are the ones the
//! project's earlier Python reader was built against and validated on real
//! logs; `tests` pins each of them against a sample line.

/// What a line of the log tells us.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A new game is starting. Everything before it belongs to the last one.
    NewGame,
    /// A card entered or left a zone, from the point of view of the player
    /// whose client wrote the log.
    Zone(ZoneMove),
    /// `Entity=<n> CardID=<id>`: a card that was hidden has been revealed.
    Reveal { entity: u32, card_id: String },
    /// The turn counter moved.
    Turn(u16),
    /// Mana crystals owned, and mana spent so far this turn, for one player.
    Resources { player_name: String, total: i16, used: i16 },
    /// Whose turn it is, by player name and whether they are now current.
    CurrentPlayer { player_name: String, current: bool },
    /// The game ended for one player.
    Result { player_name: String, won: bool },
    /// A tag changed on one entity -- a minion or a hero, not a player.
    ///
    /// Only the handful of tags the advice actually needs; everything else
    /// is dropped in `parse` rather than carried here, because `TAG_CHANGE`
    /// is most of what `Power.log` is and a session runs to hundreds of
    /// thousands of lines.
    Tag { entity: u32, what: EntityTag },
}

/// The tags worth reading off an entity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EntityTag {
    /// Current Attack, buffs and auras included.
    Atk(i16),
    /// Current maximum Health, buffs included.
    Health(i16),
    /// Damage taken. Health is `Health - Damage`.
    Damage(i16),
    /// A hero's Armor.
    Armor(i16),
    /// How many times it has attacked this turn.
    Attacks(u8),
    /// A keyword granted or taken away, by the log's own name for it. A
    /// `&'static str` from the table below rather than the parsed slice, so
    /// reading a tag never allocates.
    Keyword(&'static str, bool),
}

/// The keyword tags this reads, spelled as the log spells them.
///
/// A tag not on this list is dropped, which keeps a keyword the engine has
/// no model for from arriving as a silent half-truth.
const KEYWORD_TAGS: &[&str] = &[
    "TAUNT",
    "DIVINE_SHIELD",
    "STEALTH",
    "CHARGE",
    "RUSH",
    "WINDFURY",
    "LIFESTEAL",
    "POISONOUS",
    "REBORN",
    "CANT_BE_TARGETED_BY_SPELLS",
    "CANT_ATTACK",
    "IMMUNE",
    "FROZEN",
];

/// One card moving between zones -- the richest line either file writes.
#[derive(Clone, Debug, PartialEq)]
pub struct ZoneMove {
    pub name: String,
    pub entity: u32,
    pub card_id: String,
    pub player: u8,
    /// Whether the move is on the side of the player whose client wrote the
    /// log, which is how that player is identified at all.
    pub mine: bool,
    pub zone: String,
    /// `Hero`, `Hero Power`, or nothing — the parenthesised note the log
    /// puts after the zone.
    pub kind: Option<String>,
}

/// The time of day a line was written, in nanoseconds since midnight.
///
/// Both files are written by the same client and stamped `D HH:MM:SS.fffffff`,
/// which is the only thing that puts them in one order. It matters: only
/// Power.log carries `CREATE_GAME`, so reading one file and then the other
/// replays every zone move of every game in the session *after* the last
/// reset, and the boards of finished games pile up on the current one.
///
/// A session that runs across midnight would order wrongly for the lines
/// after it. Nothing else here depends on the clock, and a log with no stamps
/// still replays in file order.
pub fn stamp(line: &str) -> Option<u64> {
    let b = line.as_bytes();
    // `D 09:12:33.1234567` — the letter, a space, then the time.
    if b.len() < 10 || b[1] != b' ' {
        return None;
    }
    let rest = &line[2..];
    let mut it = rest.splitn(3, ':');
    let h: u64 = it.next()?.parse().ok()?;
    let m: u64 = it.next()?.parse().ok()?;
    let tail = it.next()?;
    let (sec, frac) = match tail.split_once('.') {
        Some((s, f)) => (s, f),
        None => (tail.split_whitespace().next()?, ""),
    };
    let s: u64 = sec.parse().ok()?;
    if h > 23 || m > 59 || s > 59 {
        return None;
    }
    // However many digits the client writes, scale them to nanoseconds.
    let digits: String = frac.chars().take_while(char::is_ascii_digit).collect();
    let mut nanos: u64 = digits.parse().unwrap_or(0);
    for _ in digits.len()..9 {
        nanos *= 10;
    }
    for _ in 9..digits.len() {
        nanos /= 10;
    }
    Some(((h * 60 + m) * 60 + s) * 1_000_000_000 + nanos)
}

/// The text after `key=`, up to the next space or `]`.
fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let at = line.find(key)? + key.len();
    let rest = &line[at..];
    let end = rest
        .find([' ', ']'])
        .unwrap_or(rest.len());
    Some(&rest[..end])
}

/// `entityName=` runs until ` id=`, because a card name has spaces in it.
fn entity_name(line: &str) -> Option<&str> {
    let at = line.find("entityName=")? + "entityName=".len();
    let rest = &line[at..];
    let end = rest.find(" id=")?;
    Some(&rest[..end])
}

/// A `TAG_CHANGE Entity=<name> tag=<tag> value=<value>` line, where the name
/// is a battletag and may hold spaces but never ` tag=`.
fn tag_change(line: &str) -> Option<(&str, &str, &str)> {
    let at = line.find("TAG_CHANGE Entity=")? + "TAG_CHANGE Entity=".len();
    let rest = &line[at..];
    let name_end = rest.find(" tag=")?;
    let name = &rest[..name_end];
    let tag = field(rest, "tag=")?;
    let value = field(rest, "value=")?;
    Some((name, tag, value))
}

/// Read one line. `None` for the great majority of them, which say nothing
/// this needs.
pub fn parse(line: &str) -> Option<Event> {
    if line.contains("CREATE_GAME") {
        return Some(Event::NewGame);
    }

    // A zone move, from Zone.log. The richest line in either file: it names
    // the card, its entity, its owner, and which side of the board it is on.
    if line.contains("zone from") && line.contains("entityName=") {
        // Read the fields from inside the bracketed descriptor only. The
        // line carries its own `id=` before the bracket -- `id=3 local=False
        // [entityName=... id=42 ...]` -- and taking the first match would
        // read the change's id where the entity's belongs.
        // The descriptor ends at the bracket that "zone from" follows, not
        // at the first one: an unrevealed card is named
        // `UNKNOWN ENTITY [cardType=INVALID]`, brackets and all.
        let open = line.find("[entityName=")?;
        let close = line.find("] zone from")?;
        if close < open {
            return None;
        }
        let desc = &line[open..=close];
        let name = entity_name(desc)?.to_string();
        let entity: u32 = field(desc, " id=")?.parse().ok()?;
        let card_id = field(desc, "cardId=").unwrap_or("").to_string();
        let player: u8 = field(desc, "player=")?.parse().ok()?;
        let arrow = line.rfind("-> ")? + 3;
        let tail = &line[arrow..];
        let mine = tail.starts_with("FRIENDLY");
        if !mine && !tail.starts_with("OPPOSING") {
            return None;
        }
        let mut words = tail.split_whitespace();
        words.next()?; // FRIENDLY / OPPOSING
        let zone = words.next()?.to_string();
        let kind = tail
            .find('(')
            .and_then(|i| tail[i + 1..].find(')').map(|j| tail[i + 1..i + 1 + j].to_string()));
        return Some(Event::Zone(ZoneMove {
            name,
            entity,
            card_id,
            player,
            mine,
            zone,
            kind,
        }));
    }

    if let Some(at) = line.find("SHOW_ENTITY - Updating Entity=") {
        let rest = &line[at + "SHOW_ENTITY - Updating Entity=".len()..];
        // Either a bare id, or a bracketed descriptor holding one.
        let entity: u32 = if rest.starts_with('[') {
            field(rest, " id=")?.parse().ok()?
        } else {
            rest.split_whitespace().next()?.parse().ok()?
        };
        let card_id = field(rest, "CardID=")?.to_string();
        if card_id.is_empty() {
            return None;
        }
        return Some(Event::Reveal { entity, card_id });
    }

    let (who, tag, value) = tag_change(line)?;

    // Dispatch on the tag, not on how the entity was written.
    //
    // The log spells `Entity=` three ways -- a battletag, a bare number, or
    // a bracketed descriptor -- and it does not keep one shape per kind of
    // tag. Reading the shape first meant a player written as a descriptor
    // lost its mana lines, which is what a real log did with `--me` supplied
    // and correct. So: player tags take the name out of whichever shape
    // arrived, entity tags take the id, and each says which it wants.
    match tag {
        "TURN" => return value.parse().ok().map(Event::Turn),
        "RESOURCES" => {
            return Some(Event::Resources {
                player_name: player_name(who)?.to_string(),
                total: value.parse().ok()?,
                used: -1,
            });
        }
        "RESOURCES_USED" => {
            return Some(Event::Resources {
                player_name: player_name(who)?.to_string(),
                total: -1,
                used: value.parse().ok()?,
            });
        }
        "CURRENT_PLAYER" => {
            return Some(Event::CurrentPlayer {
                player_name: player_name(who)?.to_string(),
                current: value == "1",
            });
        }
        "PLAYSTATE" if value == "WON" || value == "LOST" => {
            return Some(Event::Result {
                player_name: player_name(who)?.to_string(),
                won: value == "WON",
            });
        }
        _ => {}
    }

    let entity = entity_id(who)?;
    let n = value.parse::<i32>().ok();
    let what = match tag {
        "ATK" => EntityTag::Atk(n? as i16),
        "HEALTH" => EntityTag::Health(n? as i16),
        "DAMAGE" => EntityTag::Damage(n? as i16),
        "ARMOR" => EntityTag::Armor(n? as i16),
        "NUM_ATTACKS_THIS_TURN" => EntityTag::Attacks(n?.clamp(0, 255) as u8),
        other => {
            // Mega-Windfury writes `value=3`, so "on" is anything but zero
            // rather than exactly one.
            let known = KEYWORD_TAGS.iter().find(|k| **k == other)?;
            EntityTag::Keyword(known, n? != 0)
        }
    };
    Some(Event::Tag { entity, what })
}

/// The player's name out of an `Entity=` field, however it was written.
///
/// A bare number is an entity nobody has named yet and cannot be attributed
/// to a player, so it is dropped rather than guessed at.
fn player_name(who: &str) -> Option<&str> {
    if who.starts_with('[') {
        return entity_name(who);
    }
    if who.is_empty() || who.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(who)
}

/// The entity id out of an `Entity=` field, if it carries one.
fn entity_id(who: &str) -> Option<u32> {
    if who.starts_with('[') {
        return field(who, " id=")?.parse().ok();
    }
    who.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entity_tag_is_read_off_the_bracketed_descriptor() {
        let line = "D 09:12:33.1 [Power] GameState.DebugPrintPower() - \
             TAG_CHANGE Entity=[entityName=Chillwind Yeti id=42 zone=PLAY \
             zonePos=1 cardId=CS2_182 player=1] tag=DAMAGE value=3";
        assert_eq!(
            parse(line),
            Some(Event::Tag { entity: 42, what: EntityTag::Damage(3) })
        );
    }

    #[test]
    fn a_granted_keyword_is_read_and_an_unknown_tag_is_not() {
        let rush = "D 09:12:33.1 [Power] GameState.DebugPrintPower() - \
             TAG_CHANGE Entity=[entityName=Anomalous Shade id=51 zone=PLAY \
             zonePos=1 cardId=TIME_610t2 player=1] tag=RUSH value=1";
        assert_eq!(
            parse(rush),
            Some(Event::Tag { entity: 51, what: EntityTag::Keyword("RUSH", true) })
        );
        // Mega-Windfury writes 3, and it is still Windfury.
        let mega = rush.replace("tag=RUSH value=1", "tag=WINDFURY value=3");
        assert_eq!(
            parse(&mega),
            Some(Event::Tag { entity: 51, what: EntityTag::Keyword("WINDFURY", true) })
        );
        // A tag with no model here is dropped rather than half-read.
        let other = rush.replace("tag=RUSH value=1", "tag=ZONE_POSITION value=2");
        assert_eq!(parse(&other), None);
    }

    #[test]
    fn an_entity_named_by_a_bare_number_is_read_too() {
        let line = "D 09:12:33.1 [Power] GameState.DebugPrintPower() - \
             TAG_CHANGE Entity=76 tag=ATK value=5";
        assert_eq!(
            parse(line),
            Some(Event::Tag { entity: 76, what: EntityTag::Atk(5) })
        );
    }

    #[test]
    fn a_player_written_as_a_descriptor_still_gives_its_mana() {
        // The shape that lost every mana line when the reader dispatched on
        // how the entity was written instead of on the tag.
        let line = "D 09:12:33.1 [Power] GameState.DebugPrintPower() - \
             TAG_CHANGE Entity=[entityName=xror id=2 zone=PLAY zonePos=0 \
             cardId= player=1] tag=RESOURCES value=7";
        assert_eq!(
            parse(line),
            Some(Event::Resources {
                player_name: "xror".into(),
                total: 7,
                used: -1,
            })
        );
    }

    // Every line below is the shape the game actually writes, kept verbatim
    // from what the project's earlier reader was built against. If Blizzard
    // changes one, this is where it shows up rather than in silence.

    #[test]
    fn a_zone_move_names_the_card_its_owner_and_the_side() {
        let line = "D 09:12:33.1 [Zone] ZoneChangeList.ProcessChanges() - \
             id=3 local=False [entityName=Chillwind Yeti id=42 zone=HAND \
             zonePos=3 cardId=CS2_182 player=1] zone from FRIENDLY HAND -> \
             FRIENDLY PLAY";
        let Some(Event::Zone(m)) = parse(line) else {
            panic!("not parsed: {line}")
        };
        let ZoneMove {
            name,
            entity,
            card_id,
            player,
            mine,
            zone,
            kind,
        } = m;
        assert_eq!(name, "Chillwind Yeti");
        assert_eq!(entity, 42);
        assert_eq!(card_id, "CS2_182");
        assert_eq!(player, 1);
        assert!(mine);
        assert_eq!(zone, "PLAY");
        assert_eq!(kind, None);
    }

    #[test]
    fn a_hero_landing_is_marked_as_one() {
        let line = "[Zone] [entityName=Jaina Proudmoore id=64 zone=PLAY \
             zonePos=0 cardId=HERO_08 player=2] zone from  -> OPPOSING PLAY (Hero)";
        let Some(Event::Zone(m)) = parse(line) else {
            panic!("not parsed")
        };
        assert!(!m.mine, "the opponent's side");
        assert_eq!(m.kind.as_deref(), Some("Hero"));
    }

    #[test]
    fn an_unnamed_card_still_parses() {
        // The opponent's hand is written without a name or a card id.
        let line = "[Zone] [entityName=UNKNOWN ENTITY [cardType=INVALID] id=9 \
             zone=DECK zonePos=0 cardId= player=2] zone from OPPOSING DECK -> \
             OPPOSING HAND";
        let Some(Event::Zone(m)) = parse(line) else {
            panic!("not parsed")
        };
        assert_eq!(m.card_id, "");
        assert_eq!(m.zone, "HAND");
    }

    #[test]
    fn tag_changes_carry_the_players_name_with_its_hash() {
        assert_eq!(
            parse("D [Power] TAG_CHANGE Entity=Player#12345 tag=RESOURCES value=7"),
            Some(Event::Resources {
                player_name: "Player#12345".into(),
                total: 7,
                used: -1
            })
        );
        assert_eq!(
            parse("D [Power] TAG_CHANGE Entity=Player#12345 tag=RESOURCES_USED value=3"),
            Some(Event::Resources {
                player_name: "Player#12345".into(),
                total: -1,
                used: 3
            })
        );
        assert_eq!(
            parse("D [Power] TAG_CHANGE Entity=Player#12345 tag=CURRENT_PLAYER value=1"),
            Some(Event::CurrentPlayer {
                player_name: "Player#12345".into(),
                current: true
            })
        );
        assert_eq!(
            parse("D [Power] TAG_CHANGE Entity=GameEntity tag=TURN value=11"),
            Some(Event::Turn(11))
        );
        assert_eq!(
            parse("D [Power] TAG_CHANGE Entity=Player#12345 tag=PLAYSTATE value=WON"),
            Some(Event::Result {
                player_name: "Player#12345".into(),
                won: true
            })
        );
    }

    #[test]
    fn a_reveal_carries_the_card_id_in_both_spellings() {
        assert_eq!(
            parse("D [Power] SHOW_ENTITY - Updating Entity=42 CardID=CS2_182"),
            Some(Event::Reveal {
                entity: 42,
                card_id: "CS2_182".into()
            })
        );
        assert_eq!(
            parse(
                "D [Power] SHOW_ENTITY - Updating Entity=[entityName=Fireball \
                 id=57 zone=HAND zonePos=1 cardId= player=1] CardID=CS2_029"
            ),
            Some(Event::Reveal {
                entity: 57,
                card_id: "CS2_029".into()
            })
        );
    }

    #[test]
    fn a_line_carries_the_time_it_was_written() {
        let a = stamp("D 09:12:33.1234567 [Zone] whatever").expect("a stamp");
        let b = stamp("D 09:12:33.7654321 [Power] whatever").expect("a stamp");
        assert!(a < b, "the same second still orders by its fraction");
        let c = stamp("D 10:00:00.0000000 [Power] x").expect("a stamp");
        assert!(b < c);
        // Shorter fractions scale rather than compare as raw integers: .2 is
        // later than .1234567, not earlier.
        let short = stamp("D 09:12:33.2 [Zone] x").expect("a stamp");
        assert!(short > a);
        assert_eq!(stamp("no stamp here"), None);
        assert_eq!(stamp(""), None);
    }

    #[test]
    fn a_new_game_is_recognised_and_noise_is_not() {
        assert_eq!(parse("D [Power] CREATE_GAME"), Some(Event::NewGame));
        assert_eq!(parse("D [Power] BLOCK_START BlockType=PLAY"), None);
        assert_eq!(parse(""), None);
    }
}
