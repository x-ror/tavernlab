//! Every real deck the simulator can field, played to the end.
//!
//! `tavernsim decks` says which of the checked-in meta lists resolve against
//! the implemented card table; that is a question about the table. This is the
//! other question: whether the engine can actually *play* them. A card can
//! resolve, be counted as implemented, and still put the game into a state no
//! fixture-sized test reaches — a Quest whose reward lands on a full hand, a
//! resurrect that runs past the graveyard, a board wipe that hands both
//! players a card at once.
//!
//! Two seeds per deck, mirror matches, so both sides draw the whole list.
//! Nothing here asserts who wins: a panic, a hang or a game that never ends is
//! the failure this is looking for.

use tavernlab_core::agent::{Scripted, Style};
use tavernlab_core::cards::Class;
use tavernlab_core::deckstring;
use tavernlab_core::game::Agent;
use tavernlab_core::state::{Game, Side};

fn deck_codes() -> Vec<(String, String)> {
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../data/hsreplay_decks.txt"
    ))
    .expect("data/hsreplay_decks.txt should be checked into the repo root");
    let mut out = Vec::new();
    let mut name = String::new();
    for line in src.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("###") {
            name = rest.trim().to_string();
        } else if !line.is_empty() && !line.starts_with('#') {
            out.push((name.clone(), line.to_string()));
        }
    }
    out
}

#[test]
fn every_fieldable_meta_deck_plays_a_whole_game() {
    let mut played = 0;
    for (name, code) in deck_codes() {
        let Ok(r) = deckstring::resolve(&code) else {
            continue;
        };
        if !r.playable() || r.class == Class::Neutral {
            continue;
        }
        for seed in [1u64, 2] {
            let mut g = Game::new((r.class, &r.ids), (r.class, &r.ids), seed)
                .unwrap_or_else(|e| panic!("{name} is not a legal game: {e:?}"));
            let mut a = Scripted::new(Style::Midrange);
            let mut b = Scripted::new(Style::Control);
            let mut agents: [&mut dyn Agent; 2] = [&mut a, &mut b];
            g.start(Side::Player0, &mut agents);
            g.play_out(&mut agents);
            assert!(
                g.turn <= 200,
                "{name} on seed {seed} ran {} turns without ending",
                g.turn
            );
        }
        played += 1;
    }
    // The count the `decks` command prints. It is asserted rather than only
    // reported so that a change which quietly stops a deck from fielding
    // fails here too, and not only in a command nobody runs in CI.
    assert_eq!(
        played, 30,
        "the number of fieldable meta decks moved; re-measure with `tavernsim decks` and update this deliberately"
    );
}
