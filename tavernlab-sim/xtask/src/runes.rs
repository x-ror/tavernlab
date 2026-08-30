//! Rune costs, which the corpus does not carry.
//!
//! Runes are a Death Knight feature: a deck has three slots across Blood,
//! Frost and Unholy, and a card can go in only where its own cost fits. The
//! corpus snapshot has no field for them at all -- `cls`, `kw`, `mech`,
//! `races` and `school`, and nothing about runes -- so a card whose text
//! reads "Discover a Blood Rune card" cannot be implemented from it.
//!
//! Blizzard's own card API does serve them, as
//! `runeCost: {blood, frost, unholy}`. This reads a dump of that and writes
//! `data/runes.json`.
//!
//! # A file of its own
//!
//! Not spliced into the corpus, for the reason [`backfill`](super::backfill)
//! gives for appending rather than re-rendering: modifying entries in place
//! would put a 1.6 MB file in the diff and stake the result on this crate's
//! writer agreeing with the old builder's about every escape. A hundred and
//! seventy-odd rows in a file of their own are reviewable, and where they
//! came from stays visible.
//!
//! # Two printings, one cost
//!
//! The API serves a card once per printing -- Hematurge is `78356` in its
//! original set and `111320` in Core -- and the corpus may hold either. The
//! rune cost is a property of the card rather than the printing, so every id
//! the dump offers for a card is written, its own and each of its
//! `copyOfCardId`s. Joining on one id and hoping is how a card silently
//! arrives with no runes.
//!
//! # The dump
//!
//! Fetched by hand, never by the app:
//!
//! ```sh
//! for p in 1 2 3 4; do
//!   curl -s "https://hearthstone.blizzard.com/en-us/api/cards\
//! ?locale=en-us&class=deathknight&collectible=0,1&pageSize=500&page=$p"
//!   echo
//! done > runes_dump.json
//! ```
//!
//! ```text
//! cargo run -p xtask -- runes runes_dump.json
//! ```

use std::collections::BTreeMap;
use std::path::Path;

use tavernlab_json::Json;

/// Blood, Frost, Unholy.
type Cost = (i64, i64, i64);

pub fn run(root: &Path, dump: &Path) -> Result<String, String> {
    let src = std::fs::read_to_string(dump).map_err(|e| format!("{}: {e}", dump.display()))?;

    // One JSON object per line, the shape the fetch above produces.
    let mut costs: BTreeMap<i64, Cost> = BTreeMap::new();
    let mut cards = 0usize;
    for line in src.lines().filter(|l| !l.trim().is_empty()) {
        let doc = Json::parse(line).map_err(|e| format!("{}: {e}", dump.display()))?;
        for c in doc.arr_or_empty("cards") {
            cards += 1;
            let Some(rc) = c.get("runeCost") else { continue };
            let cost = (
                rc.i64_or_zero("blood"),
                rc.i64_or_zero("frost"),
                rc.i64_or_zero("unholy"),
            );
            if cost == (0, 0, 0) {
                continue;
            }
            let mut ids = vec![c.i64_or_zero("id")];
            ids.extend(c.arr_or_empty("copyOfCardId").iter().filter_map(Json::as_i64));
            for id in ids.into_iter().filter(|i| *i > 0) {
                // A disagreement between two printings would mean one of them
                // is not the card this thinks it is, which is worth stopping
                // for rather than picking a side.
                if let Some(seen) = costs.get(&id)
                    && *seen != cost
                {
                    return Err(format!(
                        "dbf {id} is served with two different rune costs, \
                         {seen:?} and {cost:?}"
                    ));
                }
                costs.insert(id, cost);
            }
        }
    }
    if cards == 0 {
        return Err(format!("{} holds no cards", dump.display()));
    }
    if costs.is_empty() {
        return Err(format!(
            "{} holds {cards} card(s) and no rune costs at all -- \
             is it a Death Knight dump?",
            dump.display()
        ));
    }

    let path = root.join("data/runes.json");
    let mut out = String::from(
        "{\n\
         \x20\"_\": \"Rune costs from Blizzard's card API (runeCost), by dbfId. \
         Death Knight only; every other class asks for none. Written by \
         `cargo run -p xtask -- runes <dump>` -- see xtask/src/runes.rs.\"",
    );
    for (id, (b, f, u)) in &costs {
        out.push_str(&format!(
            ",\n \"{id}\": {{\"b\": {b}, \"f\": {f}, \"u\": {u}}}"
        ));
    }
    out.push_str("\n}\n");
    Json::parse(&out).map_err(|e| format!("the file this wrote does not parse: {e}"))?;
    std::fs::write(&path, out).map_err(|e| format!("{}: {e}", path.display()))?;

    Ok(format!(
        "{} rune cost(s) from {cards} card(s) -> {}",
        costs.len(),
        path.display()
    ))
}
