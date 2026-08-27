//! Pins how many real meta-deck slots resolve against the implemented card
//! table, so that number cannot quietly fall as the table changes.
//!
//! `tavernsim gauntlet` prints the same measurement with the missing cards
//! named; this only pins the total, the way `most_classes_can_field_a_curve_deck`
//! pins the curve-deck count in `deck.rs`.

use tavernlab_core::deck::resolve_slots;
use tavernlab_json::Json;

/// The `[name, count]` pairs of a deck's `"cards"` or `"sideboard"` array.
fn pairs(v: Option<&Json>) -> Vec<(&str, u32)> {
    v.and_then(Json::as_array)
        .unwrap_or(&[])
        .iter()
        .filter_map(|entry| {
            let a = entry.as_array()?;
            let name = a.first()?.as_str()?;
            let count = a.get(1)?.as_i64()?;
            Some((name, count.max(0) as u32))
        })
        .collect()
}

#[test]
fn meta_deck_slots_resolve_at_least_as_well_as_measured() {
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../data/gauntlet_standard.json"
    ))
    .expect("data/gauntlet_standard.json should be checked into the repo root");
    let doc = Json::parse(&src).expect("gauntlet_standard.json should be valid JSON");
    let decks = doc.as_object().expect("gauntlet_standard.json is a JSON object");

    let mut ok = 0u32;
    let mut total = 0u32;
    for (_, deck) in decks {
        let report = resolve_slots(&pairs(deck.get("cards")));
        ok += report.ok;
        total += report.total;
    }

    assert_eq!(
        total, 350,
        "the meta-deck file's own slot count moved; re-measure and update this test deliberately"
    );
    // Raised from the 224 of the first Rust card batch to what the table
    // reaches today. Only two slots are left, both `Lunarwing Messenger` in a
    // deck list that is twenty cards long anyway.
    assert!(
        ok >= 348,
        "only {ok}/{total} meta-deck slots resolve; this must not fall below the last measured baseline"
    );
}
