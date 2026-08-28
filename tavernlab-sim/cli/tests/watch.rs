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

use std::process::Command;

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
        .output()
        .expect("run tavernsim watch");
    String::from_utf8_lossy(&out.stdout).to_string()
        + &String::from_utf8_lossy(&out.stderr)
}

#[test]
fn before_the_first_turn_it_advises_the_mulligan() {
    let out = run("mulligan", LOG, &["--me", "Me#1"]);
    assert!(out.contains("MAGE"), "{out}");
    assert!(out.contains("WARRIOR"), "the opponent's hero named their class: {out}");
    assert!(out.contains("МУЛІГАН"), "{out}");
    assert!(out.contains("Chillwind Yeti"), "{out}");
    assert!(
        !out.contains("ХІД\n"),
        "there is no turn to plan before the game starts: {out}"
    );
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
    assert!(out.contains("HS_ME"), "and it says how to fix it: {out}");
}

#[test]
fn a_missing_log_says_so_instead_of_pretending() {
    let out = Command::new(env!("CARGO_BIN_EXE_tavernsim"))
        .arg("watch")
        .arg("--logs")
        .arg(std::env::temp_dir().join("tavernlab-no-such-logs-dir"))
        .arg("--once")
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
    // The bug the first real log showed. `CREATE_GAME` is written only to
    // Power.log, so reading one file and then the other puts every reset at
    // the front and lays every zone move of the whole session on top of the
    // last game: minions on boards that are empty, and the classes of a game
    // that finished an hour ago.
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
        .output()
        .expect("run tavernsim watch");
    let out = String::from_utf8_lossy(&out.stdout).to_string()
        + &String::from_utf8_lossy(&out.stderr);

    assert!(
        out.contains("SHAMAN проти PALADIN"),
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
