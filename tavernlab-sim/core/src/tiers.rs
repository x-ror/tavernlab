//! A tier list the simulator can defend.
//!
//! **Not the ladder.** TavernLab does not scrape HSReplay or Untapped for
//! win rates, so it cannot say what is strong in ranked play. What it can do
//! is play the gauntlet against itself and report *that*, which is a
//! different claim and has to be labelled as one.
//!
//! Every deck plays every other deck; mirrors are excluded, because a deck's
//! standing against the field should not include itself. The tier is a band
//! of its mean win rate across that field.
//!
//! The matrix is quadratic in the size of the field, which is why this is a
//! job a user starts rather than something that runs on load — and why
//! [`margin`] is published next to the table: at twenty games a pair the
//! error bar is wider than the tier bands themselves, and a table that hides
//! that is a horoscope.

use crate::gauntlet::MetaDeck;
use crate::state::Side;

/// Bands on "win rate against this field", stated here rather than in the UI
/// so the number and its label cannot disagree.
pub const BANDS: [(&str, f64); 5] = [
    ("S", 0.56),
    ("A", 0.52),
    ("B", 0.48),
    ("C", 0.44),
    ("D", 0.0),
];

/// 95% half-width on one matchup, in win-rate points.
///
/// At 20 games a pair this is ±0.22 — the 100% and 20% matchups such a run
/// produces are noise wearing a number's clothes.
pub fn margin(games_per_pair: usize) -> f64 {
    if games_per_pair == 0 {
        return 1.0;
    }
    1.96 * (0.25 / games_per_pair as f64).sqrt()
}

pub fn tier_for(rate: f64) -> &'static str {
    BANDS
        .iter()
        .find(|(_, floor)| rate >= *floor)
        .map(|(name, _)| *name)
        .unwrap_or("D")
}

/// One deck's standing.
#[derive(Clone, Debug)]
pub struct Row {
    pub name: String,
    pub class: crate::cards::Class,
    pub style: crate::agent::Style,
    /// Mean win rate against the rest of the field.
    pub winrate: f64,
    pub tier: &'static str,
    /// `(opponent, win rate)`, mirrors excluded.
    pub vs: Vec<(String, f64)>,
}

/// The whole table.
#[derive(Clone, Debug)]
pub struct Table {
    pub games_per_pair: usize,
    pub margin: f64,
    /// Decks ranked by win rate against the field.
    pub rows: Vec<Row>,
    /// Decks that could not be fielded at all, and are therefore in no row
    /// and in nobody's average.
    pub skipped: Vec<String>,
}

/// Play the field against itself.
///
/// Cost is `playable² × games_per_pair` games. `log` is called once per deck
/// so a long run can show progress.
pub fn build(
    field: &[MetaDeck],
    games_per_pair: usize,
    threads: usize,
    log: impl FnMut(String),
) -> Table {
    build_with(
        field,
        [crate::batch::Policy::Greedy; 2],
        games_per_pair,
        threads,
        log,
    )
}

/// [`build`] with the policy named rather than assumed.
///
/// A tier list is a statement about decks, and it is only that if it does not
/// move when the policy behind it changes. Building the same table twice with
/// two policies is how that gets checked.
pub fn build_with(
    field: &[MetaDeck],
    policies: [crate::batch::Policy; 2],
    games_per_pair: usize,
    threads: usize,
    mut log: impl FnMut(String),
) -> Table {
    let playable: Vec<&MetaDeck> = field.iter().filter(|d| d.playable()).collect();
    let skipped: Vec<String> = field
        .iter()
        .filter(|d| !d.playable())
        .map(|d| d.name.clone())
        .collect();
    let pairs = playable.len().saturating_sub(1) * playable.len();
    log(format!(
        "Матриця {n}×{n}: {} боїв…",
        pairs * games_per_pair,
        n = playable.len()
    ));

    let mut rows = Vec::with_capacity(playable.len());
    for deck in &playable {
        let mut vs = Vec::with_capacity(playable.len().saturating_sub(1));
        for other in &playable {
            if other.name == deck.name {
                continue; // a mirror says nothing about standing
            }
            // The seed base is derived from the pair, not from a counter, so
            // one deck's row does not depend on how many decks precede it.
            let r = crate::gauntlet::matchup_with(
                deck.contender(),
                other.contender(),
                policies,
                games_per_pair,
                threads,
                7,
            );
            vs.push((other.name.clone(), r.rate(Side::Player0)));
        }
        let winrate = if vs.is_empty() {
            0.0
        } else {
            vs.iter().map(|(_, r)| r).sum::<f64>() / vs.len() as f64
        };
        log(format!("{} — {:.1}%", deck.name, winrate * 100.0));
        rows.push(Row {
            name: deck.name.clone(),
            class: deck.class,
            style: deck.style,
            winrate,
            tier: tier_for(winrate),
            vs,
        });
    }
    rows.sort_by(|a, b| b.winrate.total_cmp(&a.winrate));
    Table {
        games_per_pair,
        margin: margin(games_per_pair),
        rows,
        skipped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Style;
    use crate::cards::{Class, Formats};
    use crate::deck::curve_deck;

    fn field() -> Vec<MetaDeck> {
        let mut out: Vec<MetaDeck> = [Class::Mage, Class::Druid, Class::Hunter]
            .into_iter()
            .map(|c| {
                let deck = curve_deck(c, Formats::STANDARD).unwrap();
                let mut list: Vec<(String, u32)> = Vec::new();
                for id in &deck {
                    match list.iter_mut().find(|(n, _)| n == id.name()) {
                        Some((_, n)) => *n += 1,
                        None => list.push((id.name().to_string(), 1)),
                    }
                }
                MetaDeck::new(format!("{c:?}"), c, Style::Midrange, &list, &[])
            })
            .collect();
        out.push(MetaDeck::new(
            "Broken",
            Class::Mage,
            Style::Midrange,
            &[("Not A Real Card".to_string(), 30)],
            &[],
        ));
        out
    }

    #[test]
    fn every_playable_deck_gets_a_row_and_the_rest_are_named() {
        let t = build(&field(), 30, 2, |_| {});
        assert_eq!(t.rows.len(), 3);
        assert_eq!(t.skipped, vec!["Broken".to_string()]);
        for r in &t.rows {
            assert_eq!(r.vs.len(), 2, "{} should meet the other two", r.name);
            assert!(r.vs.iter().all(|(n, _)| *n != r.name), "a mirror leaked in");
            assert_eq!(r.tier, tier_for(r.winrate));
        }
    }

    #[test]
    fn naming_the_greedy_policy_is_the_same_as_not_naming_it() {
        // `build` is `build_with` under the default policy, and the whole
        // point of the pair is that a table can be rebuilt by a different
        // one. If the delegation drifted, a policy comparison would be
        // measuring the plumbing.
        let f = field();
        let plain = build(&f, 30, 2, |_| {});
        let named = build_with(&f, [crate::batch::Policy::Greedy; 2], 30, 2, |_| {});
        assert_eq!(plain.skipped, named.skipped);
        assert_eq!(plain.rows.len(), named.rows.len());
        for (a, b) in plain.rows.iter().zip(named.rows.iter()) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.winrate, b.winrate);
        }
    }

    #[test]
    fn rows_are_ranked() {
        let t = build(&field(), 30, 2, |_| {});
        for w in t.rows.windows(2) {
            assert!(w[0].winrate >= w[1].winrate);
        }
    }

    #[test]
    fn the_field_averages_to_a_coin_flip() {
        // Every game is somebody's win, so the mean across a full round robin
        // sits at 0.5 whatever the decks are. A pairing or book-keeping bug
        // shows up here immediately.
        //
        // It is a *statistical* 0.5, not an exact one: A-against-B and
        // B-against-A are different games (the decks swap sides), so the two
        // rates only sum to 1 in expectation. The sample is large enough that
        // the tolerance below is about four standard errors — tightening it
        // without raising the games would make this fail on noise, which it
        // once did.
        let games = 400;
        let t = build(&field(), games, 2, |_| {});
        let mean: f64 = t.rows.iter().map(|r| r.winrate).sum::<f64>() / t.rows.len() as f64;
        assert!(
            (mean - 0.5).abs() < 0.03,
            "field mean {mean} over {games} games a pair"
        );
    }

    #[test]
    fn the_margin_is_published_and_shrinks_with_sample_size() {
        assert!(margin(20) > 0.2);
        assert!(margin(500) < 0.05);
        assert!(margin(20) > margin(500));
        assert_eq!(margin(0), 1.0);
    }

    #[test]
    fn bands_are_ordered_and_cover_everything() {
        for w in BANDS.windows(2) {
            assert!(w[0].1 > w[1].1);
        }
        assert_eq!(tier_for(1.0), "S");
        assert_eq!(tier_for(0.5), "B");
        assert_eq!(tier_for(0.0), "D");
    }
}
