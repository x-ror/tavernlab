//! `tavernsim watch`, end to end.
//!
//! The log parser is unit-tested line by line; this runs the real binary
//! against a whole log file, because the failure that reaches a user is the
//! wiring — a file found but not read, a class read but not used, a plan
//! built from a board nobody filled in.
//!
//! The log below is synthetic, written to the line shapes the parser's own
//! tests pin. It is not a substitute for one real session, and the command
//! prints the position it reconstructed precisely so that a mismatch against
//! a real log is visible rather than silent.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

const LOG: &str = "\
D 09:00:00.0 [Power] GameState.DebugPrintPower() - CREATE_GAME
D 09:00:00.1 [Zone] ZoneChangeList.ProcessChanges() - id=1 local=False [entityName=Jaina Proudmoore id=64 zone=PLAY zonePos=0 cardId=HERO_08 player=1] zone from  -> FRIENDLY PLAY (Hero)
D 09:00:00.1 [Zone] ZoneChangeList.ProcessChanges() - id=2 local=False [entityName=Garrosh Hellscream id=65 zone=PLAY zonePos=0 cardId=HERO_01 player=2] zone from  -> OPPOSING PLAY (Hero)
D 09:00:00.2 [Zone] ZoneChangeList.ProcessChanges() - id=3 local=False [entityName=Chillwind Yeti id=10 zone=DECK zonePos=0 cardId=CS2_182 player=1] zone from FRIENDLY DECK -> FRIENDLY HAND
D 09:00:00.2 [Zone] ZoneChangeList.ProcessChanges() - id=4 local=False [entityName=Fireball id=11 zone=DECK zonePos=0 cardId=CS2_029 player=1] zone from FRIENDLY DECK -> FRIENDLY HAND
D 09:00:00.2 [Zone] ZoneChangeList.ProcessChanges() - id=5 local=False [entityName=UNKNOWN ENTITY [cardType=INVALID] id=90 zone=DECK zonePos=0 cardId= player=2] zone from OPPOSING DECK -> OPPOSING HAND
";

const AFTER_MULLIGAN: &str = "\
D 09:00:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=Me#1 tag=RESOURCES value=4
D 09:00:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=Me#1 tag=RESOURCES_USED value=0
D 09:00:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=TURN value=7
D 09:00:01.1 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=Me#1 tag=CURRENT_PLAYER value=1
D 09:00:02.0 [Zone] ZoneChangeList.ProcessChanges() - id=6 local=False [entityName=Bloodfen Raptor id=20 zone=HAND zonePos=1 cardId=CS2_172 player=2] zone from OPPOSING HAND -> OPPOSING PLAY
";

/// `tag` names the file: the tests run in parallel and two of them pass the
/// same arguments, so keying the path off those would have them clobber each
/// other's log.
/// Isolate the data home so a test cannot pick up the user's deckstring
/// or write a finished game into the real history file.
fn home_for(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir()
        .join(format!("tavernlab-watch-{}", std::process::id()))
        .join(format!("{tag}-home"));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn run(tag: &str, body: &str, extra: &[&str]) -> String {
    let dir = std::env::temp_dir().join(format!("tavernlab-watch-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("{tag}.log"));
    std::fs::write(&path, body).expect("write the log");
    let out = Command::new(env!("CARGO_BIN_EXE_tavernsim"))
        .arg("watch")
        .arg("--log")
        .arg(&path)
        .arg("--once")
        .args(extra)
        .env("TAVERNLAB_HOME", home_for(tag))
        .env_remove("HS_DECK")
        .env_remove("HS_ME")
        .output()
        .expect("run tavernsim watch");
    String::from_utf8_lossy(&out.stdout).to_string()
        + &String::from_utf8_lossy(&out.stderr)
}

#[test]
fn before_the_first_turn_it_advises_the_mulligan() {
    let out = run("mulligan", LOG, &["--me", "Me#1"]);
    // The class names are the app's own, in the app's language: the terminal
    // and the Live tab write the same keys out of `locales/`.
    assert!(out.contains("Маг"), "{out}");
    assert!(out.contains("Воїн"), "the opponent's hero named their class: {out}");
    assert!(out.contains("МУЛІГАН"), "{out}");
    assert!(out.contains("Chillwind Yeti"), "{out}");
    assert!(
        !out.contains("ХІД\n"),
        "there is no turn to plan before the game starts: {out}"
    );
}

/// A legal thirty-card Standard Mage list, generated from the implemented
/// pool by `deck::curve_deck` and encoded once. Regenerate it if the pool
/// ever loses a card in it -- `deckstring::resolve` will say so by returning
/// nothing, and the test below will fail rather than pass quietly.
const MAGE_DECK: &str = "AAECAf0ECKiKBKqKBP6eBJSgBJegBJ2gBKOgBKfUBAumigT8ngT9ngShnwSmnwSrnwS1nwS9nwTnnwSaoATx0wQAAA==";

/// The same opening, but with the player's tags written as descriptors --
/// the shape that carries `player=`, and the one the game writes for most of
/// a real session.
const AFTER_MULLIGAN_DESCRIBED: &str = "\
D 09:00:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=[entityName=xror id=2 zone=PLAY zonePos=0 cardId= player=1] tag=RESOURCES value=4
D 09:00:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=[entityName=xror id=2 zone=PLAY zonePos=0 cardId= player=1] tag=RESOURCES_USED value=0
D 09:00:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=TURN value=7
D 09:00:01.1 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=[entityName=xror id=2 zone=PLAY zonePos=0 cardId= player=1] tag=CURRENT_PLAYER value=1
D 09:00:02.0 [Zone] ZoneChangeList.ProcessChanges() - id=6 local=False [entityName=Bloodfen Raptor id=20 zone=HAND zonePos=1 cardId=CS2_172 player=2] zone from OPPOSING HAND -> OPPOSING PLAY
";

/// The player who goes first, on turn one, with a one-drop in hand. The
/// mulligan is over but the turn counter has not moved past one and no
/// MAIN_READY has arrived -- which is exactly the position the watcher used
/// to have nothing to say about.
const FIRST_TURN: &str = "\
D 09:00:00.3 [Zone] ZoneChangeList.ProcessChanges() - id=5 local=False [entityName=Wolfrider id=12 zone=DECK zonePos=0 cardId=CS2_124 player=1] zone from FRIENDLY DECK -> FRIENDLY HAND
D 09:00:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=[entityName=xror id=2 zone=PLAY zonePos=0 cardId= player=1] tag=RESOURCES value=3
D 09:00:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=[entityName=xror id=2 zone=PLAY zonePos=0 cardId= player=1] tag=RESOURCES_USED value=0
D 09:00:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=TURN value=1
D 09:00:01.1 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=[entityName=xror id=2 zone=PLAY zonePos=0 cardId= player=1] tag=CURRENT_PLAYER value=1
";

#[test]
fn the_first_turn_gets_a_plan_and_not_only_a_mulligan() {
    // Reported from a real game: going first, nothing told you what to play.
    // "Started" is MAIN_READY or a turn past one, and the first turn of the
    // player who leads is turn one -- so the report stopped at the mulligan
    // even though its own heading said the turn was yours.
    let out = run("firstturn", &format!("{LOG}{FIRST_TURN}"), &[]);
    assert!(out.contains("хід 1 (ваш)"), "{out}");
    assert!(out.contains("МУЛІГАН"), "the mulligan is still there: {out}");
    assert!(out.contains("ХІД"), "and now so is the turn: {out}");
    assert!(
        out.contains("зіграти Wolfrider"),
        "three mana buys the one-drop: {out}"
    );
}

#[test]
fn the_battletag_does_not_have_to_be_typed() {
    // No `--me`. The log says which player number is FRIENDLY and, on a
    // descriptor line, which name holds that number -- so the mana is
    // attributed without being told whose it is.
    let out = run(
        "autome",
        &format!("{LOG}{AFTER_MULLIGAN_DESCRIBED}"),
        &[],
    );
    assert!(out.contains("мана 4/4"), "the mana was attributed: {out}");
    assert!(
        out.contains("ви: xror (визначено з логу)"),
        "and it says which name it settled on: {out}"
    );
}

#[test]
fn a_deck_passed_once_is_remembered() {
    // The lab already fills the deck in from its settings; this is the other
    // direction, so that a code typed on the command line does not have to be
    // typed again.
    let home = home_for("remembered");
    let log = home.join("Power.log");
    std::fs::write(&log, format!("{LOG}{AFTER_MULLIGAN_DESCRIBED}")).expect("write the log");
    let once = Command::new(env!("CARGO_BIN_EXE_tavernsim"))
        .args(["watch", "--log"])
        .arg(&log)
        .args(["--once", "--no-history", "--deck", MAGE_DECK])
        .env("TAVERNLAB_HOME", &home)
        .env_remove("HS_DECK")
        .env_remove("HS_ME")
        .output()
        .expect("run with the deck");
    let once = String::from_utf8_lossy(&once.stdout).to_string();
    assert!(!once.contains("колоду не відновлено"), "{once}");

    let again = Command::new(env!("CARGO_BIN_EXE_tavernsim"))
        .args(["watch", "--log"])
        .arg(&log)
        .args(["--once", "--no-history"])
        .env("TAVERNLAB_HOME", &home)
        .env_remove("HS_DECK")
        .env_remove("HS_ME")
        .output()
        .expect("run without it");
    let again = String::from_utf8_lossy(&again.stdout).to_string();
    assert!(
        again.contains("колода з лабораторії"),
        "the second run says where the deck came from: {again}"
    );
    assert!(
        !again.contains("колоду не відновлено"),
        "and it really has one: {again}"
    );
}

#[test]
fn without_a_deck_the_plan_says_a_draw_will_read_as_fatigue() {
    // The rebuilt position has no deck unless the deck code supplies one, and
    // an empty deck turns every draw effect into damage to your own hero --
    // so the plan quietly refuses to play the card that draws. That is a real
    // limit on the advice and it has to be said out loud.
    let out = run("nodeck", &format!("{LOG}{AFTER_MULLIGAN}"), &["--me", "Me#1"]);
    assert!(out.contains("колоду не відновлено"), "{out}");
}

#[test]
fn with_a_deck_the_plan_is_drawn_over_a_real_library() {
    let out = run(
        "withdeck",
        &format!("{LOG}{AFTER_MULLIGAN}"),
        &["--me", "Me#1", "--deck", MAGE_DECK],
    );
    assert!(
        !out.contains("колоду не відновлено"),
        "the deck code was read, so the caveat does not apply: {out}"
    );
    // And the advice is still advice: the position has not changed.
    assert!(out.contains("зіграти Chillwind Yeti"), "{out}");
}

#[test]
fn once_the_game_starts_it_reads_the_board_and_plans_the_turn() {
    let out = run("turn", &format!("{LOG}{AFTER_MULLIGAN}"), &["--me", "Me#1"]);
    assert!(out.contains("хід 7"), "{out}");
    assert!(out.contains("мана 4/4"), "the mana lines were attributed: {out}");
    assert!(
        out.contains("ворожа дошка: Bloodfen Raptor"),
        "the opponent's play landed on their board: {out}"
    );
    assert!(
        out.contains("рука: Chillwind Yeti, Fireball"),
        "and the hand is still the hand: {out}"
    );
    assert!(out.contains("ХІД"), "{out}");
    assert!(
        out.contains("зіграти Chillwind Yeti"),
        "four mana buys the Yeti: {out}"
    );
}

#[test]
fn without_a_battletag_the_mana_is_unknown_rather_than_zero() {
    // The log names both players and never says which one is you, so an
    // unattributed RESOURCES line has to read as unknown. Printing 0/0 would
    // be a number nobody measured.
    let out = run("nameless", &format!("{LOG}{AFTER_MULLIGAN}"), &[]);
    assert!(out.contains("мана невідома"), "{out}");
    assert!(
        out.contains("бойовий тег"),
        "and it says what would fix it: {out}"
    );
}

#[test]
fn a_missing_log_says_so_instead_of_pretending() {
    let out = Command::new(env!("CARGO_BIN_EXE_tavernsim"))
        .arg("watch")
        .arg("--logs")
        .arg(std::env::temp_dir().join("tavernlab-no-such-logs-dir"))
        .arg("--once")
        .env("TAVERNLAB_HOME", home_for("missing"))
        .env_remove("HS_DECK")
        .env_remove("HS_ME")
        .output()
        .expect("run tavernsim watch");
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("log.config"), "it names the fix: {err}");
}


/// A session directory the way the client lays one out, with the two files
/// the watcher reads.
fn session(tag: &str, power: &str, zone: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir()
        .join(format!("tavernlab-watch-{}", std::process::id()))
        .join(tag);
    let _ = std::fs::create_dir_all(&dir);
    // The watcher skips a Power.log too small to be real verbose logging, so
    // the fixture has to be padded past that floor.
    let mut padded = String::new();
    while padded.len() < 5000 {
        padded.push_str("D 00:00:00.0000000 [Power] padding that says nothing\n");
    }
    padded.push_str(power);
    std::fs::write(dir.join("Power.log"), padded).expect("write Power.log");
    std::fs::write(dir.join("Zone.log"), zone).expect("write Zone.log");
    dir
}

#[test]
fn a_finished_games_board_does_not_pile_up_on_the_next_one() {
    // `CREATE_GAME` is written only to Power.log, so reading one file and
    // then the other would put every reset at the front and lay every zone
    // move of the whole session on top of the last game: minions on boards
    // that are empty, and the classes of a game that finished an hour ago.
    let power = "\
D 09:00:00.0000000 [Power] GameState.DebugPrintPower() - CREATE_GAME
D 09:30:00.0000000 [Power] GameState.DebugPrintPower() - CREATE_GAME
D 09:30:05.0000000 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=TURN value=2
D 09:30:05.1000000 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=Me#1 tag=CURRENT_PLAYER value=1
";
    // The first game's hero and board, then the second game's hero. In file
    // order the first game's Chillwind Yeti survives into the second.
    let zone = "\
D 09:00:01.0000000 [Zone] [entityName=Jaina Proudmoore id=64 zone=PLAY zonePos=0 cardId=HERO_08 player=1] zone from  -> FRIENDLY PLAY (Hero)
D 09:00:02.0000000 [Zone] [entityName=Chillwind Yeti id=42 zone=HAND zonePos=1 cardId=CS2_182 player=1] zone from FRIENDLY HAND -> FRIENDLY PLAY
D 09:30:01.0000000 [Zone] [entityName=Thrall id=70 zone=PLAY zonePos=0 cardId=HERO_02 player=1] zone from  -> FRIENDLY PLAY (Hero)
D 09:30:02.0000000 [Zone] [entityName=Uther Lightbringer id=71 zone=PLAY zonePos=0 cardId=HERO_04 player=2] zone from  -> OPPOSING PLAY (Hero)
";
    // `--logs` is the directory *of* session directories, the way the client
    // lays them out, so the parent is what the watcher is pointed at.
    let dir = session("two-games", power, zone);
    let out = Command::new(env!("CARGO_BIN_EXE_tavernsim"))
        .arg("watch")
        .arg("--logs")
        .arg(dir.parent().expect("a parent"))
        .arg("--me")
        .arg("Me#1")
        .arg("--once")
        .env("TAVERNLAB_HOME", home_for("two-games"))
        .env_remove("HS_DECK")
        .env_remove("HS_ME")
        .output()
        .expect("run tavernsim watch");
    let out = String::from_utf8_lossy(&out.stdout).to_string()
        + &String::from_utf8_lossy(&out.stderr);

    assert!(
        out.contains("Шаман проти Паладин"),
        "the classes come from the game being played, not the one before it: {out}"
    );
    assert!(
        out.contains("ваша дошка: порожня"),
        "the finished game's board must not carry over: {out}"
    );
    assert!(
        !out.contains("Chillwind Yeti"),
        "nothing of the first game is left: {out}"
    );
}

/// A minion that landed this turn is still summoning sick, and the plan has
/// to know it.
///
/// From a real session: the board was four freshly summoned 3/2 Shades and
/// the watcher told the player to swing all of them into the enemy hero. The
/// Shades take Rush from a Bonus Effect, and Rush is the point — it trades on
/// the turn it lands and goes face on the turn after. Neither a sick minion
/// nor a rushed one may hit the hero the turn it arrives, so nothing that
/// entered play on the current turn may be told to attack a face.
#[test]
fn a_minion_played_this_turn_is_not_told_to_attack() {
    const EARLIER: &str = "\
D 09:00:03.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=TURN value=5
D 09:00:03.1 [Zone] ZoneChangeList.ProcessChanges() - id=7 local=False [entityName=Chillwind Yeti id=10 zone=HAND zonePos=1 cardId=CS2_182 player=1] zone from FRIENDLY HAND -> FRIENDLY PLAY
D 09:00:03.2 [Zone] ZoneChangeList.ProcessChanges() - id=8 local=False [entityName=Bloodfen Raptor id=21 zone=DECK zonePos=0 cardId=CS2_172 player=1] zone from FRIENDLY DECK -> FRIENDLY HAND
";
    const NOW: &str = "\
D 09:00:04.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=Me#1 tag=RESOURCES value=7
D 09:00:04.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=Me#1 tag=RESOURCES_USED value=0
D 09:00:04.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=TURN value=7
D 09:00:04.1 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=Me#1 tag=CURRENT_PLAYER value=1
D 09:00:04.2 [Zone] ZoneChangeList.ProcessChanges() - id=9 local=False [entityName=Bloodfen Raptor id=21 zone=HAND zonePos=1 cardId=CS2_172 player=1] zone from FRIENDLY HAND -> FRIENDLY PLAY
";
    let out = run("sick", &format!("{LOG}{EARLIER}{NOW}"), &["--me", "Me#1"]);
    assert!(
        out.contains("ваша дошка: Chillwind Yeti 4/5, Bloodfen Raptor 3/2"),
        "both are on the board: {out}"
    );
    assert!(
        !out.contains("атакувати: Bloodfen Raptor"),
        "it landed this turn, so it cannot attack at all: {out}"
    );
    assert!(
        out.contains("атакувати: Chillwind Yeti"),
        "but the one from turn five is free to swing: {out}"
    );
}

/// The position the command prints is the one the log states, not the one the
/// cards print.
///
/// Stats, damage, granted keywords and both heroes' health all come off
/// `TAG_CHANGE` lines. Before this the reconstruction used printed stats and
/// two untouched heroes, which made every lethal it saw a wrong one.
#[test]
fn the_printed_position_carries_what_the_log_said_about_it() {
    const PLAYED: &str = "\
D 09:00:03.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=TURN value=5
D 09:00:03.1 [Zone] ZoneChangeList.ProcessChanges() - id=7 local=False [entityName=Chillwind Yeti id=10 zone=HAND zonePos=1 cardId=CS2_182 player=1] zone from FRIENDLY HAND -> FRIENDLY PLAY
D 09:00:03.2 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=[entityName=Chillwind Yeti id=10 zone=PLAY zonePos=1 cardId=CS2_182 player=1] tag=ATK value=6
D 09:00:03.3 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=[entityName=Chillwind Yeti id=10 zone=PLAY zonePos=1 cardId=CS2_182 player=1] tag=DAMAGE value=2
D 09:00:03.4 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=[entityName=Chillwind Yeti id=10 zone=PLAY zonePos=1 cardId=CS2_182 player=1] tag=TAUNT value=1
D 09:00:03.5 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=64 tag=DAMAGE value=8
D 09:00:03.6 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=65 tag=ARMOR value=5
D 09:00:03.7 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=65 tag=DAMAGE value=11
";
    const NOW: &str = "\
D 09:00:04.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=Me#1 tag=RESOURCES value=7
D 09:00:04.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=Me#1 tag=RESOURCES_USED value=0
D 09:00:04.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=TURN value=7
D 09:00:04.1 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=Me#1 tag=CURRENT_PLAYER value=1
";
    let out = run("tags", &format!("{LOG}{PLAYED}{NOW}"), &["--me", "Me#1"]);
    assert!(
        out.contains("ваша дошка: Chillwind Yeti 6/3 Taunt"),
        "buffed to 6 attack, two damage on it, Taunt granted: {out}"
    );
    assert!(
        out.contains("ваш герой 22, ворожий 19 (+5 броні)"),
        "both heroes off their own DAMAGE and ARMOR lines: {out}"
    );
}

/// A Rush minion played this turn may trade, but not go to the face.
///
/// From a real session: four freshly summoned Shades take Rush from a Bonus
/// Effect and the watcher sent all four into the enemy hero. Now the log's
/// own `tag=RUSH` line is read, so the minion is offered the trade it really
/// has and refused the swing it does not.
#[test]
fn a_rush_minion_played_this_turn_trades_but_does_not_go_face() {
    const NOW: &str = "\
D 09:00:04.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=Me#1 tag=RESOURCES value=7
D 09:00:04.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=Me#1 tag=RESOURCES_USED value=7
D 09:00:04.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=TURN value=7
D 09:00:04.1 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=Me#1 tag=CURRENT_PLAYER value=1
D 09:00:04.2 [Zone] ZoneChangeList.ProcessChanges() - id=8 local=False [entityName=Wisp id=30 zone=HAND zonePos=1 cardId=CS2_231 player=2] zone from OPPOSING HAND -> OPPOSING PLAY
D 09:00:04.3 [Zone] ZoneChangeList.ProcessChanges() - id=9 local=False [entityName=Chillwind Yeti id=31 zone=HAND zonePos=1 cardId=CS2_182 player=1] zone from FRIENDLY HAND -> FRIENDLY PLAY
D 09:00:04.4 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=[entityName=Chillwind Yeti id=31 zone=PLAY zonePos=1 cardId=CS2_182 player=1] tag=RUSH value=1
";
    let out = run("rush", &format!("{LOG}{NOW}"), &["--me", "Me#1"]);
    assert!(
        out.contains("ваша дошка: Chillwind Yeti 4/5 Rush"),
        "the granted keyword is shown: {out}"
    );
    assert!(
        out.contains("атакувати: Chillwind Yeti → ворожий Wisp"),
        "Rush trades on the turn it lands: {out}"
    );
    assert!(
        !out.contains("Chillwind Yeti → ворожий герой"),
        "but it does not go face until the turn after: {out}"
    );
}

/// A read is a fraction of what was played, and it says so.
///
/// From a real session: before the opponent had played a single card the
/// report said `Herald Warlock 100%`, and after one card that no list held it
/// said `Herald Warlock 0%` — a deck name with the confidence stripped off
/// but the name left standing. `Read::frac` is 1.0 on no evidence by design,
/// for the web UI's neutral prior; printing it as a percentage is what was
/// wrong.
#[test]
fn the_opponent_read_does_not_name_a_deck_it_has_no_evidence_for() {
    const STARTED: &str = "\
D 09:00:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=TURN value=2
";
    let bare = run("read-none", &format!("{LOG}{STARTED}"), &["--me", "Me#1"]);
    assert!(
        bare.contains("ще нічого не зіграно"),
        "no card played, so no read: {bare}"
    );
    assert!(!bare.contains("100%"), "and no percentage either: {bare}");

    // One card, and it is not in any Warrior list in the gauntlet.
    const ODD: &str = "\
D 09:00:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=TURN value=2
D 09:00:02.0 [Zone] ZoneChangeList.ProcessChanges() - id=6 local=False [entityName=Wisp id=20 zone=HAND zonePos=1 cardId=CS2_231 player=2] zone from OPPOSING HAND -> OPPOSING PLAY
";
    let odd = run("read-miss", &format!("{LOG}{ODD}"), &["--me", "Me#1"]);
    assert!(
        odd.contains("жодна колода гаунтлета не пояснює зіграного"),
        "nothing matched, so no deck is named: {odd}"
    );
}

/// A dead hero reads as zero, and the report says the game is over.
///
/// The killing blow's `DAMAGE` tag is the whole hit rather than the part that
/// fit, so the raw subtraction goes negative — a real log printed
/// `ваш герой -3`.
#[test]
fn a_dead_hero_is_zero_and_the_game_is_called_over() {
    const LETHAL: &str = "\
D 09:00:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=TURN value=12
D 09:00:02.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=64 tag=DAMAGE value=33
D 09:00:03.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=Me#1 tag=PLAYSTATE value=LOST
";
    let out = run("dead", &format!("{LOG}{LETHAL}"), &["--me", "Me#1"]);
    assert!(out.contains("ваш герой 0,"), "not -3: {out}");
    assert!(out.contains("гру завершено"), "{out}");
    assert!(!out.contains("ХІД"), "and a finished game gets no plan: {out}");
}

/// The Coin is not advice when there is nothing left to spend it on.
///
/// The engine's agent plays it whenever it is legal, which is right for a
/// game it is playing out and wrong as the last line of a plan.
#[test]
fn a_coin_with_nothing_after_it_is_not_suggested() {
    const NOW: &str = "\
D 09:00:04.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=Me#1 tag=RESOURCES value=0
D 09:00:04.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=Me#1 tag=RESOURCES_USED value=0
D 09:00:04.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=TURN value=1
D 09:00:04.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=STEP value=MAIN_READY
D 09:00:04.1 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=Me#1 tag=CURRENT_PLAYER value=1
D 09:00:04.2 [Zone] ZoneChangeList.ProcessChanges() - id=8 local=False [entityName=The Coin id=33 zone=DECK zonePos=0 cardId=GAME_005 player=1] zone from  -> FRIENDLY HAND
";
    let out = run("coin", &format!("{LOG}{NOW}"), &["--me", "Me#1"]);
    assert!(
        out.contains("рука: Chillwind Yeti, Fireball, The Coin"),
        "the Coin is in hand: {out}"
    );
    assert!(
        !out.contains("зіграти The Coin"),
        "the one crystal it makes buys nothing here, so neither does it: {out}"
    );
}

/// A minion the log has taken to zero health is off the board already.
///
/// The `DAMAGE` line and the line that moves the body out of play arrive in
/// different poll batches, up to a poll apart. In between, the body reads as
/// standing at zero health, and the plan would be drawn over a board with a
/// corpse on it.
#[test]
fn a_body_at_zero_health_is_not_in_the_position() {
    const NOW: &str = "\
D 09:00:03.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=TURN value=5
D 09:00:03.1 [Zone] ZoneChangeList.ProcessChanges() - id=7 local=False [entityName=Chillwind Yeti id=10 zone=HAND zonePos=1 cardId=CS2_182 player=1] zone from FRIENDLY HAND -> FRIENDLY PLAY
D 09:00:03.2 [Zone] ZoneChangeList.ProcessChanges() - id=8 local=False [entityName=Bloodfen Raptor id=11 zone=HAND zonePos=1 cardId=CS2_172 player=2] zone from OPPOSING HAND -> OPPOSING PLAY
D 09:00:04.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=Me#1 tag=RESOURCES value=7
D 09:00:04.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=Me#1 tag=RESOURCES_USED value=7
D 09:00:04.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=TURN value=7
D 09:00:04.1 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=Me#1 tag=CURRENT_PLAYER value=1
D 09:00:04.2 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=[entityName=Bloodfen Raptor id=11 zone=PLAY zonePos=1 cardId=CS2_172 player=2] tag=DAMAGE value=2
";
    let out = run("dead-body", &format!("{LOG}{NOW}"), &["--me", "Me#1"]);
    assert!(
        out.contains("ворожа дошка: порожня"),
        "a 3/2 with two damage on it is dead, whatever the zone line has not said yet: {out}"
    );
    assert!(
        out.contains("ваша дошка: Chillwind Yeti"),
        "and the live one is still there: {out}"
    );
    assert!(
        !out.contains("→ ворожий Bloodfen Raptor"),
        "so nothing is told to trade with it: {out}"
    );
}

/// Two copies of a weapon are not two plays.
///
/// Straight from a real plan: `зіграти Corpse Cannon` twice in one turn.
/// Equipping the second breaks the first, and both are 1/3 -- three swings
/// thrown away for nothing. The engine has always broken the weapon
/// correctly; the policy simply never asked what the equip would destroy.
#[test]
fn a_weapon_is_not_played_twice_in_one_turn() {
    // A hand of nothing but the two weapons, and mana for both, so that what
    // stops the second is the rule rather than the curve.
    const ONLY_WEAPONS: &str = "\
D 09:00:00.0 [Power] GameState.DebugPrintPower() - CREATE_GAME
D 09:00:00.1 [Zone] ZoneChangeList.ProcessChanges() - id=1 local=False [entityName=Jaina Proudmoore id=64 zone=PLAY zonePos=0 cardId=HERO_08 player=1] zone from  -> FRIENDLY PLAY (Hero)
D 09:00:00.1 [Zone] ZoneChangeList.ProcessChanges() - id=2 local=False [entityName=Garrosh Hellscream id=65 zone=PLAY zonePos=0 cardId=HERO_01 player=2] zone from  -> OPPOSING PLAY (Hero)
D 09:00:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=TURN value=7
D 09:00:01.1 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=Me#1 tag=RESOURCES value=8
D 09:00:01.1 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=Me#1 tag=RESOURCES_USED value=0
D 09:00:01.2 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=Me#1 tag=CURRENT_PLAYER value=1
D 09:00:02.0 [Zone] ZoneChangeList.ProcessChanges() - id=8 local=False [entityName=Corpse Cannon id=40 zone=DECK zonePos=0 cardId=JAIL_450 player=1] zone from FRIENDLY DECK -> FRIENDLY HAND
D 09:00:02.1 [Zone] ZoneChangeList.ProcessChanges() - id=9 local=False [entityName=Corpse Cannon id=41 zone=DECK zonePos=0 cardId=JAIL_450 player=1] zone from FRIENDLY DECK -> FRIENDLY HAND
";
    let out = run("weapon-twice", ONLY_WEAPONS, &["--me", "Me#1"]);
    assert!(
        out.contains("рука: Corpse Cannon, Corpse Cannon"),
        "both copies are in hand, and eight mana buys four of them: {out}"
    );
    // Not "at most once": re-equipping a fresh 1/3 over one worn down to 1/2
    // gains a swing, and the plan is right to do it after the hero has
    // attacked. What is never right is two equips with nothing in between --
    // the second breaks a weapon that has not been used at all.
    let plan: Vec<&str> = out
        .lines()
        .skip_while(|l| !l.contains("ХІД"))
        .map(str::trim)
        .collect();
    assert!(
        plan.contains(&"зіграти Corpse Cannon"),
        "the weapon is worth equipping once: {out}"
    );
    assert!(
        !plan
            .windows(2)
            .any(|w| w[0] == "зіграти Corpse Cannon" && w[1] == "зіграти Corpse Cannon"),
        "but never twice in a row, which throws the first one away: {out}"
    );
}

/// A damaging spell goes at them, not at you.
///
/// The corpus gives Fireball and a heal the same target spec — any character —
/// so with the target unscored the pick fell to enumeration order, and a real
/// plan read `зіграти Fireball → свій герой`.
#[test]
fn a_damaging_spell_is_not_aimed_at_your_own_hero() {
    const NOW: &str = "\
D 09:00:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=TURN value=7
D 09:00:01.1 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=Me#1 tag=RESOURCES value=8
D 09:00:01.1 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=Me#1 tag=RESOURCES_USED value=0
D 09:00:01.2 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=Me#1 tag=CURRENT_PLAYER value=1
";
    let out = run("own-face", &format!("{LOG}{NOW}"), &["--me", "Me#1"]);
    assert!(out.contains("зіграти Fireball → ворожий герой"), "{out}");
    assert!(
        !out.contains("Fireball → свій герой"),
        "six damage to your own face is never the play: {out}"
    );
}

/// A real Power.log writes CREATE_GAME and PLAYSTATE twice — GameState,
/// then PowerTaskList a moment later — and sets TURN=1 before any card
/// is dealt. Taking both copies doubled the history; taking TURN=1 as
/// "the game has started" left every row with an empty opening.
#[test]
fn a_real_shaped_log_records_one_game_with_its_opening() {
    const BODY: &str = "\
D 22:16:06.5689958 GameState.DebugPrintPower() - CREATE_GAME
D 22:16:06.5689958 GameState.DebugPrintPower() -     TAG_CHANGE Entity=1 tag=TURN value=1
D 22:16:06.5689958 PowerTaskList.DebugPrintPower() -     CREATE_GAME
D 22:16:08.6500861 ZoneChangeList.ProcessChanges() - id=1 local=False [entityName=Jaina Proudmoore id=64 zone=PLAY zonePos=0 cardId=HERO_08 player=1] zone from  -> FRIENDLY PLAY (Hero)
D 22:16:08.7487601 ZoneChangeList.ProcessChanges() - id=1 local=False [entityName=Garrosh Hellscream id=66 zone=PLAY zonePos=0 cardId=HERO_01 player=2] zone from  -> OPPOSING PLAY (Hero)
D 22:16:09.8666320 ZoneChangeList.ProcessChanges() - id=2 local=False [entityName=Chillwind Yeti id=30 zone=HAND zonePos=0 cardId=CS2_182 player=1] zone from FRIENDLY DECK -> FRIENDLY HAND
D 22:16:10.0999193 ZoneChangeList.ProcessChanges() - id=2 local=False [entityName=The Coin id=68 zone=HAND zonePos=5 cardId=GAME_005 player=1] zone from  -> FRIENDLY HAND
D 22:16:40.9516948 GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=STEP value=MAIN_READY
D 22:16:42.2228160 ZoneChangeList.ProcessChanges() - id=4 local=False [entityName=Fireball id=25 zone=HAND zonePos=0 cardId=CS2_029 player=1] zone from FRIENDLY DECK -> FRIENDLY HAND
D 22:22:19.2393682 GameState.DebugPrintPower() - TAG_CHANGE Entity=Me#1 tag=PLAYSTATE value=WON
D 22:22:20.0049768 PowerTaskList.DebugPrintPower() -     TAG_CHANGE Entity=Me#1 tag=PLAYSTATE value=WON
";
    let dir = std::env::temp_dir().join(format!("tavernlab-watch-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("real-shaped.log");
    std::fs::write(&path, BODY).expect("write the log");
    let home = home_for("real-shaped");
    let out = Command::new(env!("CARGO_BIN_EXE_tavernsim"))
        .args(["watch", "--log"])
        .arg(&path)
        .args(["--me", "Me#1", "--once", "--quiet"])
        .env("TAVERNLAB_HOME", &home)
        .env_remove("HS_DECK")
        .env_remove("HS_ME")
        .output()
        .expect("run tavernsim watch --quiet");
    let out = String::from_utf8_lossy(&out.stdout).to_string()
        + &String::from_utf8_lossy(&out.stderr);
    assert!(
        out.contains("записано в історію: 1 "),
        "PowerTaskList is an echo, not a second game: {out}"
    );

    let hist = Command::new(env!("CARGO_BIN_EXE_tavernsim"))
        .arg("history")
        .env("TAVERNLAB_HOME", &home)
        .output()
        .expect("read the history back");
    let hist = String::from_utf8_lossy(&hist.stdout).to_string();
    assert!(hist.contains("1 бій"), "one row, not two: {hist}");
    assert!(hist.contains("MAGE"), "{hist}");
    assert!(hist.contains("WARRIOR"), "{hist}");

    // Opening is in the sqlite file even if the CLI summary does not print
    // the cards. sqlite3 would, and so does a second watch against the same
    // log: zero new rows, because the game is already there.
    let again = Command::new(env!("CARGO_BIN_EXE_tavernsim"))
        .args(["watch", "--log"])
        .arg(&path)
        .args(["--me", "Me#1", "--once", "--quiet"])
        .env("TAVERNLAB_HOME", &home)
        .env_remove("HS_DECK")
        .env_remove("HS_ME")
        .output()
        .expect("watch the same log again");
    let again = String::from_utf8_lossy(&again.stdout).to_string()
        + &String::from_utf8_lossy(&again.stderr);
    assert!(
        !again.contains("записано в історію"),
        "re-reading must not double it: {again}"
    );
}

/// `--quiet` is the recorder: finished games go to history, and the
/// advice — including the mulligan batch — is not printed.
#[test]
fn quiet_records_a_finished_game_without_advising() {
    const OVER: &str = "\
D 09:00:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=TURN value=12
D 09:05:00.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=Me#1 tag=PLAYSTATE value=WON
";
    let dir = std::env::temp_dir().join(format!("tavernlab-watch-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("quiet.log");
    std::fs::write(&path, format!("{LOG}{OVER}")).expect("write the log");
    let home = home_for("quiet");
    let out = Command::new(env!("CARGO_BIN_EXE_tavernsim"))
        .args(["watch", "--log"])
        .arg(&path)
        .args(["--me", "Me#1", "--once", "--quiet"])
        .env("TAVERNLAB_HOME", &home)
        .env_remove("HS_DECK")
        .env_remove("HS_ME")
        .output()
        .expect("run tavernsim watch --quiet");
    let out = String::from_utf8_lossy(&out.stdout).to_string()
        + &String::from_utf8_lossy(&out.stderr);
    assert!(
        out.contains("записано в історію"),
        "the finished game is the whole point: {out}"
    );
    assert!(
        !out.contains("МУЛІГАН") && !out.contains("ХІД") && !out.contains("СУПЕРНИК"),
        "quiet means no advice: {out}"
    );
    assert!(
        home.join("history.sqlite").exists(),
        "and the file is where the rest of the program will look for it"
    );
}

/// The last game of a session is not lost when the client rotates.
///
/// The recorder follows the newest session directory. The client writes a
/// session's final lines as it exits and starts a fresh directory on the next
/// launch, and when those land inside one poll of each other the old file
/// still has unread lines at the moment the recorder lets go of it. That
/// dropped the game which ended the session — silently, in the one record
/// whose whole job is not to drop one.
///
/// Slow by nature: this is the daemon, and the daemon polls. The waits are
/// several times the poll interval so a loaded machine does not fail it.
#[test]
fn a_session_that_rotates_does_not_lose_its_last_game() {
    fn game(minute: u8, hero_name: &str, hero_id: &str, turns: u8) -> String {
        format!(
            "D 09:0{minute}:00.0 [Power] GameState.DebugPrintPower() - CREATE_GAME\n\
             D 09:0{minute}:00.1 [Zone] ZoneChangeList.ProcessChanges() - id=1 local=False \
             [entityName=Jaina Proudmoore id=64 zone=PLAY zonePos=0 cardId=HERO_08 player=1] \
             zone from  -> FRIENDLY PLAY (Hero)\n\
             D 09:0{minute}:00.1 [Zone] ZoneChangeList.ProcessChanges() - id=2 local=False \
             [entityName={hero_name} id=65 zone=PLAY zonePos=0 cardId={hero_id} player=2] \
             zone from  -> OPPOSING PLAY (Hero)\n\
             D 09:0{minute}:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE \
             Entity=GameEntity tag=TURN value={turns}\n\
             D 09:0{minute}:30.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE \
             Entity=Me#1 tag=PLAYSTATE value=WON\n"
        )
    }
    // The recorder ignores a Power.log too small to be a real session.
    let pad = "D 09:00:00.0 [Power] pad ".to_string() + &"#".repeat(5000) + "\n";

    let root = std::env::temp_dir().join(format!("tavernlab-rotate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let logs = root.join("logs");
    let first = logs.join("S1");
    std::fs::create_dir_all(&first).expect("session one");
    let home = root.join("home");
    std::fs::create_dir_all(&home).expect("home");
    let s1 = first.join("Power.log");
    std::fs::write(&s1, format!("{}{pad}", game(1, "Garrosh Hellscream", "HERO_01", 12)))
        .expect("write session one");

    let mut child = Command::new(env!("CARGO_BIN_EXE_tavernsim"))
        .args(["watch", "--quiet", "--logs"])
        .arg(&logs)
        .args(["--me", "Me#1"])
        .env("TAVERNLAB_HOME", &home)
        .env_remove("HS_DECK")
        .env_remove("HS_ME")
        .env_remove("HS_LOGS")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start the recorder");

    // Let it take the first session in.
    std::thread::sleep(Duration::from_millis(2500));

    // The client finishes a game and rotates, both inside one poll.
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&s1)
        .expect("append to session one");
    f.write_all(game(3, "Uther Lightbringer", "HERO_04", 9).as_bytes())
        .expect("the game that ends the session");
    drop(f);
    // Far enough apart that the two directories cannot share a modification
    // time -- the recorder picks the newest, and with equal stamps there is
    // no rotation to test -- and far inside one poll, which is the whole
    // point: both land before the recorder looks again.
    std::thread::sleep(Duration::from_millis(100));
    let second = logs.join("S2");
    std::fs::create_dir_all(&second).expect("session two");
    std::fs::write(
        second.join("Power.log"),
        format!("{}{pad}", game(5, "Thrall", "HERO_02", 7)),
    )
    .expect("write session two");

    std::thread::sleep(Duration::from_millis(2500));
    let _ = child.kill();
    let _ = child.wait();

    let out = Command::new(env!("CARGO_BIN_EXE_tavernsim"))
        .arg("history")
        .env("TAVERNLAB_HOME", &home)
        .output()
        .expect("read the history back");
    let out = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.contains("WARRIOR"), "the first session's game: {out}");
    assert!(
        out.contains("PALADIN"),
        "the game that ended the rotated session — the one that used to go missing: {out}"
    );
    assert!(out.contains("SHAMAN"), "and the new session's game: {out}");
}

/// A weapon in play is swings the plan can spend.
///
/// The rebuilt hero used to have bare hands whatever the log said, so the
/// plan never suggested the attack -- which for the classes that equip is
/// most of what a turn is. The corpus says which cards are weapons, so this
/// reads correctly whether or not the client marks the zone line `(Weapon)`.
const WEAPON: &str = "\
D 09:00:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=Me#1 tag=RESOURCES value=4
D 09:00:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=Me#1 tag=RESOURCES_USED value=4
D 09:00:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=TURN value=7
D 09:00:01.1 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=Me#1 tag=CURRENT_PLAYER value=1
D 09:00:02.0 [Zone] ZoneChangeList.ProcessChanges() - id=9 local=False [entityName=Fiery War Axe id=40 zone=HAND zonePos=1 cardId=CS2_106 player=1] zone from FRIENDLY HAND -> FRIENDLY PLAY (Weapon)
";

#[test]
fn an_equipped_weapon_is_a_swing_the_plan_can_spend() {
    let out = run("weapon", &format!("{LOG}{WEAPON}"), &["--me", "Me#1"]);
    assert!(
        out.contains("ваша зброя: Fiery War Axe 3/2"),
        "the printed numbers stand until the log says otherwise: {out}"
    );
    assert!(
        out.contains("бити героєм"),
        "and the plan spends the swing: {out}"
    );
}

#[test]
fn a_hero_that_has_already_swung_is_not_told_to_swing_again() {
    // `NUM_ATTACKS_THIS_TURN` on the hero. Offering the attack a second time
    // is offering one the game will not allow.
    let swung = format!(
        "{WEAPON}D 09:00:03.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE \
         Entity=[entityName=Jaina Proudmoore id=64 zone=PLAY zonePos=0 cardId=HERO_08 \
         player=1] tag=NUM_ATTACKS_THIS_TURN value=1\n"
    );
    let out = run("weapon-spent", &format!("{LOG}{swung}"), &["--me", "Me#1"]);
    assert!(
        out.contains("ваша зброя: Fiery War Axe 3/2"),
        "the weapon is still equipped: {out}"
    );
    assert!(
        !out.contains("бити героєм"),
        "but the swing is spent: {out}"
    );
}

#[test]
fn a_weapon_that_broke_is_no_longer_in_the_position() {
    let broken = format!(
        "{WEAPON}D 09:00:03.0 [Zone] ZoneChangeList.ProcessChanges() - id=10 local=False \
         [entityName=Fiery War Axe id=40 zone=PLAY zonePos=0 cardId=CS2_106 player=1] \
         zone from FRIENDLY PLAY (Weapon) -> FRIENDLY GRAVEYARD\n"
    );
    let out = run("weapon-broken", &format!("{LOG}{broken}"), &["--me", "Me#1"]);
    assert!(
        !out.contains("ваша зброя"),
        "a broken weapon is no swings, not a stale line: {out}"
    );
}

/// A secret the log named is yours; one it did not is theirs.
///
/// The two are not the same fact and are not treated the same way. Yours goes
/// into the rebuilt position, so the plan stops offering the copy the game
/// would refuse to set. Theirs goes nowhere near it: filling the opponent's
/// zone from what their deck usually plays would be a card the log never
/// said, and the plan would be drawn around a guess. It is said out loud
/// instead, and the reader plays around what the plan cannot see.
const SECRETS: &str = "\
D 09:00:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=Me#1 tag=RESOURCES value=4
D 09:00:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=Me#1 tag=RESOURCES_USED value=0
D 09:00:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=TURN value=7
D 09:00:01.1 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=Me#1 tag=CURRENT_PLAYER value=1
D 09:00:02.0 [Zone] ZoneChangeList.ProcessChanges() - id=9 local=False [entityName=Counterspell id=41 zone=HAND zonePos=1 cardId=EX1_287 player=1] zone from FRIENDLY HAND -> FRIENDLY SECRET
D 09:00:02.1 [Zone] ZoneChangeList.ProcessChanges() - id=10 local=False [entityName=UNKNOWN ENTITY [cardType=INVALID] id=42 zone=HAND zonePos=1 cardId= player=2] zone from OPPOSING HAND -> OPPOSING SECRET
";

#[test]
fn your_secret_is_named_and_theirs_is_only_counted() {
    let out = run("secrets", &format!("{LOG}{SECRETS}"), &["--me", "Me#1"]);
    assert!(
        out.contains("ваші секрети: Counterspell"),
        "your own client writes your card id: {out}"
    );
    assert!(
        out.contains("ворожих секретів: 1"),
        "and theirs is a count, because that is all the log said: {out}"
    );
    assert!(
        !out.contains("ворожі секрети: Counterspell"),
        "nothing names a card the log did not: {out}"
    );
    assert!(
        out.contains("у суперника секретів: 1"),
        "the plan says what it did not account for: {out}"
    );
}

#[test]
fn a_secret_that_fired_is_no_longer_in_the_position() {
    let fired = format!(
        "{SECRETS}D 09:00:03.0 [Zone] ZoneChangeList.ProcessChanges() - id=11 local=False \
         [entityName=UNKNOWN ENTITY [cardType=INVALID] id=42 zone=SECRET zonePos=0 cardId= \
         player=2] zone from OPPOSING SECRET -> OPPOSING GRAVEYARD\n"
    );
    let out = run("secret-fired", &format!("{LOG}{fired}"), &["--me", "Me#1"]);
    assert!(
        !out.contains("ворожих секретів"),
        "a spent secret is not a threat: {out}"
    );
    assert!(
        out.contains("ваші секрети: Counterspell"),
        "and yours is untouched: {out}"
    );
}

/// A secret is a play now, and a secret already set is not.
///
/// `Planner::eval` weighs boards, hero totals, cards in hand and unspent
/// mana, so before `Weights::secret` every line that set a secret scored
/// below the line that did not, and the plan never offered one. With a price
/// on it -- measured, see the README -- it does, and the engine's own rule
/// that a secret cannot be set twice becomes something the plan obeys rather
/// than something no plan ever reached.
const ONLY_A_SECRET: &str = "\
D 09:00:00.0 [Power] GameState.DebugPrintPower() - CREATE_GAME
D 09:00:00.1 [Zone] ZoneChangeList.ProcessChanges() - id=1 local=False [entityName=Jaina Proudmoore id=64 zone=PLAY zonePos=0 cardId=HERO_08 player=1] zone from  -> FRIENDLY PLAY (Hero)
D 09:00:00.1 [Zone] ZoneChangeList.ProcessChanges() - id=2 local=False [entityName=Garrosh Hellscream id=65 zone=PLAY zonePos=0 cardId=HERO_01 player=2] zone from  -> OPPOSING PLAY (Hero)
D 09:00:00.2 [Zone] ZoneChangeList.ProcessChanges() - id=3 local=False [entityName=Counterspell id=41 zone=DECK zonePos=0 cardId=EX1_287 player=1] zone from FRIENDLY DECK -> FRIENDLY HAND
D 09:00:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=Me#1 tag=RESOURCES value=7
D 09:00:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=Me#1 tag=RESOURCES_USED value=0
D 09:00:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=TURN value=9
D 09:00:01.1 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=Me#1 tag=CURRENT_PLAYER value=1
";

#[test]
fn a_secret_in_hand_is_a_play_the_plan_offers() {
    let out = run("secret-hand", ONLY_A_SECRET, &["--me", "Me#1"]);
    assert!(
        out.contains("зіграти Counterspell"),
        "the evaluation prices it now, so the plan spends the mana: {out}"
    );
}

#[test]
fn the_copy_of_a_secret_already_set_is_not_offered() {
    // The same hand and the same mana, with one already in the zone. The
    // engine refuses to offer a secret it holds a copy of, and the rebuilt
    // position carries the real zone -- so the plan does not suggest the
    // set that the game would not allow.
    let already = format!(
        "{ONLY_A_SECRET}D 09:00:02.0 [Zone] ZoneChangeList.ProcessChanges() - id=9 local=False \
         [entityName=Counterspell id=50 zone=HAND zonePos=1 cardId=EX1_287 player=1] \
         zone from FRIENDLY HAND -> FRIENDLY SECRET\n"
    );
    let out = run("secret-dup", &already, &["--me", "Me#1"]);
    assert!(
        out.contains("ваші секрети: Counterspell"),
        "one is set: {out}"
    );
    assert!(
        !out.contains("зіграти Counterspell"),
        "so the copy in hand is not a play: {out}"
    );
}

/// The turn plan does not need to know your name to know it is your turn.
///
/// A client that writes `CURRENT_PLAYER` as a bare battletag gives no player
/// number, so the name can never be matched to one and the turn was never
/// attributed -- which meant no plan at all, every turn, for the whole game,
/// in silence. The opening hand answers it anyway: The Coin goes to the
/// player on the draw, and that player takes the even-numbered turns.
const BARE_TAGS: &str = "\
D 09:00:00.0 [Power] GameState.DebugPrintPower() - CREATE_GAME
D 09:00:00.1 [Zone] ZoneChangeList.ProcessChanges() - id=1 local=False [entityName=The Lich King id=64 zone=PLAY zonePos=0 cardId=HERO_11 player=1] zone from  -> FRIENDLY PLAY (Hero)
D 09:00:00.1 [Zone] ZoneChangeList.ProcessChanges() - id=2 local=False [entityName=Gul'dan id=65 zone=PLAY zonePos=0 cardId=HERO_07 player=2] zone from  -> OPPOSING PLAY (Hero)
D 09:00:00.2 [Zone] ZoneChangeList.ProcessChanges() - id=3 local=False [entityName=The Coin id=11 zone=DECK zonePos=0 cardId=GAME_005 player=1] zone from  -> FRIENDLY HAND
D 09:00:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=TURN value=2
D 09:00:01.1 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=xror#21652 tag=CURRENT_PLAYER value=1
D 09:00:01.2 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=starkalpha#2221 tag=CURRENT_PLAYER value=0
";

#[test]
fn a_log_that_never_names_you_still_gets_a_turn_plan() {
    let out = run("baretags", BARE_TAGS, &[]);
    assert!(out.contains("хід 2 (ваш)"), "the Coin says the even turns are mine: {out}");
    assert!(out.contains("ХІД"), "and so there is a plan at all: {out}");
    assert!(
        out.contains("у лозі трапилися імена: xror#21652, starkalpha#2221"),
        "the names are still offered, because the battletag is still unmatched: {out}"
    );
}

#[test]
fn the_guessed_mana_is_the_crystals_the_turn_has() {
    // Each player gains one at the start of their own turn and the two
    // alternate, so turn two is the second player's first: one crystal, not
    // two. `turn / 2 + 1` said two, and a plan drawn at two spends what you
    // do not have.
    let out = run("guessmana", BARE_TAGS, &[]);
    assert!(out.contains("рахував як 1"), "{out}");
    assert!(
        out.contains("зіграти The Coin"),
        "and with one crystal the Coin is what buys the turn: {out}"
    );
}
