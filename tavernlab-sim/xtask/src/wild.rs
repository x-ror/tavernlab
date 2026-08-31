//! Regenerating the generated gauntlets: Wild, and the Arena field.
//!
//! **These are baselines, not the meta.** The Standard gauntlet is twelve real
//! top-legend lists typed in from trackers. Wild has no source we are allowed
//! to use — the legal posture rules out scraping HSReplay or Untapped — and
//! Arena opponents are drafts that exist nowhere as lists at all, so neither
//! has a meta to load.
//!
//! What this writes instead is one deck per class, assembled from the cards
//! the engine actually implements in that pool, following a fixed curve —
//! midrange for Wild, the 2–4-heavy tempo curve for Arena. That gives the
//! evaluation a measurable, reproducible opponent field. It does not claim
//! to be what people are queuing into, and every deck says so in its own
//! `source` field.
//!
//! It has to be regenerated whenever the implemented pool changes shape,
//! because a deck holding a card the engine cannot play is not fielded at
//! all: the previous, Python-generated file resolved to zero playable decks
//! against this engine's table.

use std::fmt::Write as _;
use std::path::Path;

use tavernlab_core::cards::{Class, Formats, PLAYABLE_CLASSES};
use tavernlab_core::deck::curve_deck;

/// One generated deck on its way to the file: the name it will carry, the
/// class it belongs to, and its cards as (name, count).
type Entry = (String, Class, Vec<(&'static str, u32)>);

/// Write `data/gauntlet_wild.json`. Returns a summary for the console.
pub fn generate(root: &Path) -> Result<String, String> {
    generate_for(root, Formats::WILD, "Wild", "midrange", "wild-gauntlet")
}

/// Write `data/gauntlet_arena.json`: the Arena field — one tempo-curve deck
/// per class from the season pool. Aggro at the table, because The Arena's
/// 5-loss format rewards games that end before the late game.
pub fn generate_arena(root: &Path) -> Result<String, String> {
    if !tavernlab_core::cards::arena_pool_present() {
        return Err(
            "the corpus carries no Arena pool — write data/arena_season.json \
             and rerun `cargo run -p xtask -- cards` first"
                .into(),
        );
    }
    generate_for(root, Formats::ARENA, "Arena", "aggro", "arena-gauntlet")
}

fn generate_for(
    root: &Path,
    format: Formats,
    word: &str,
    archetype: &str,
    cmd: &str,
) -> Result<String, String> {
    let path = root.join(format!("data/gauntlet_{}.json", word.to_lowercase()));
    let mut decks: Vec<Entry> = Vec::new();
    let mut skipped: Vec<Class> = Vec::new();

    for class in PLAYABLE_CLASSES {
        let Some(deck) = curve_deck(class, format) else {
            // Never silently drop a class: a missing deck reads as "this
            // class is bad in Wild" rather than "we could not build one".
            skipped.push(class);
            continue;
        };
        let mut counts: Vec<(&'static str, u32)> = Vec::new();
        for id in &deck {
            match counts.iter_mut().find(|(n, _)| *n == id.name()) {
                Some((_, c)) => *c += 1,
                None => counts.push((id.name(), 1)),
            }
        }
        counts.sort_unstable();
        decks.push((format!("{} {word} Baseline", title(class)), class, counts));
    }
    if decks.is_empty() {
        return Err(format!(
            "no class could field a {word} deck from the implemented pool"
        ));
    }

    let mut out = String::from("{\n");
    for (i, (name, class, cards)) in decks.iter().enumerate() {
        let total: u32 = cards.iter().map(|(_, n)| n).sum();
        let _ = write!(
            out,
            " {}: {{\n  \"class\": {},\n  \"archetype\": \"{archetype}\",\n  \
             \"source\": \"generated from the implemented {word} pool \
             (cargo run -p xtask -- {cmd}) — not tracker meta\",\n  \
             \"cards\": [\n",
            tavernlab_json::escape(name),
            tavernlab_json::escape(tavernlab_core::gauntlet::class_name(*class))
        );
        for (j, (card, n)) in cards.iter().enumerate() {
            let _ = writeln!(
                out,
                "   [{}, {n}]{}",
                tavernlab_json::escape(card),
                if j + 1 == cards.len() { "" } else { "," }
            );
        }
        let _ = write!(
            out,
            "  ],\n  \"sideboard\": [],\n  \"total\": {total}\n }}{}\n",
            if i + 1 == decks.len() { "" } else { "," }
        );
    }
    out.push_str("}\n");

    std::fs::write(&path, &out).map_err(|e| format!("writing {}: {e}", path.display()))?;

    let mut msg = format!(
        "{} {word} baseline decks -> {}",
        decks.len(),
        path.display()
    );
    if !skipped.is_empty() {
        let names: Vec<String> = skipped.iter().map(|c| format!("{c:?}")).collect();
        let _ = write!(
            msg,
            "\n  {} class(es) skipped, too few implemented {word} cards: {}",
            skipped.len(),
            names.join(", ")
        );
    }
    msg.push_str(
        "\n  These are baselines, not the meta. Replace them with real lists as you get them.",
    );
    Ok(msg)
}

/// `DeathKnight` -> `Deathknight`, matching the names the previous generator
/// wrote so a regeneration is a content diff rather than a rename.
fn title(c: Class) -> String {
    let name = format!("{c:?}");
    let mut out = String::with_capacity(name.len());
    for (i, ch) in name.chars().enumerate() {
        if i == 0 {
            out.extend(ch.to_uppercase());
        } else {
            out.extend(ch.to_lowercase());
        }
    }
    out
}
