//! How big the graveyard has to be.
//!
//! `Player::graveyard` is capped so a `Game` stays a fixed few hundred bytes,
//! and a cap is only honest if it is above what real games produce. This plays
//! a sample and checks that.

use tavernlab_core::agent::{Scripted, Style};
use tavernlab_core::cards::{Class, Formats};
use tavernlab_core::deck::curve_deck;
use tavernlab_core::state::{GRAVEYARD, Game, Side};

#[test]
fn real_games_do_not_fill_the_graveyard() {
    const CLASSES: [Class; 5] = [
        Class::Mage,
        Class::Druid,
        Class::Hunter,
        Class::Warrior,
        Class::Warlock,
    ];
    let mut worst = 0u8;
    for seed in 0..120u64 {
        let a = CLASSES[seed as usize % CLASSES.len()];
        let b = CLASSES[(seed as usize / 3) % CLASSES.len()];
        let (da, db) = (
            curve_deck(a, Formats::STANDARD).unwrap(),
            curve_deck(b, Formats::STANDARD).unwrap(),
        );
        let mut g = Game::new((a, &da), (b, &db), seed).unwrap();
        let (mut p0, mut p1) = (Scripted::new(Style::Midrange), Scripted::new(Style::Midrange));
        let first = if seed % 2 == 0 { Side::Player0 } else { Side::Player1 };
        g.run(first, &mut [&mut p0, &mut p1]);
        worst = worst.max(g.players[0].deaths).max(g.players[1].deaths);
    }
    // Over a larger sample (8000 player-games) the worst seen was 23, and the
    // cap was never reached. If this ever fires, the pool has stopped being a
    // complete record and either the cap or the claim has to change.
    assert!(
        (worst as usize) < GRAVEYARD,
        "worst graveyard was {worst} of {GRAVEYARD}"
    );
}
