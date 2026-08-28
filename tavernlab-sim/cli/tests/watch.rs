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
