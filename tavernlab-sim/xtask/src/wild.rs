//! Regenerating the Wild gauntlet.
//!
//! **These are baselines, not the meta.** The Standard gauntlet is twelve real
//! top-legend lists typed in from trackers. Wild has no source we are allowed
//! to use — the legal posture rules out scraping HSReplay or Untapped — so
//! there is no Wild meta here to load.
//!
//! What this writes instead is one deck per class, assembled from the cards
//! the engine actually implements in Wild, following the same curve the
//! benchmark decks use. That gives Wild evaluation a measurable, reproducible
//! opponent field. It does not claim to be what people are queuing into, and
//! every deck says so in its own `source` field.
//!
//! It has to be regenerated whenever the implemented pool changes shape,
//! because a deck holding a card the engine cannot play is not fielded at
//! all: the previous, Python-generated file resolved to zero playable decks
//! against this engine's table.

use std::fmt::Write as _;
use std::path::Path;

use tavernlab_core::cards::{Class, Formats, PLAYABLE_CLASSES};
use tavernlab_core::deck::curve_deck;

/// Write `data/gauntlet_wild.json`. Returns a summary for the console.
pub fn generate(root: &Path) -> Result<String, String> {
    let path = root.join("data/gauntlet_wild.json");
    let mut decks: Vec<(String, Class, Vec<(&'static str, u32)>)> = Vec::new();
    let mut skipped: Vec<Class> = Vec::new();

    for class in PLAYABLE_CLASSES {
        let Some(deck) = curve_deck(class, Formats::WILD) else {
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
        decks.push((format!("{} Wild Baseline", title(class)), class, counts));
    }
    if decks.is_empty() {
        return Err("no class could field a Wild deck from the implemented pool".into());
    }

    let mut out = String::from("{\n");
    for (i, (name, class, cards)) in decks.iter().enumerate() {
        let total: u32 = cards.iter().map(|(_, n)| n).sum();
        let _ = write!(
            out,
            " {}: {{\n  \"class\": {},\n  \"archetype\": \"midrange\",\n  \
             \"source\": \"generated from the implemented Wild pool \
             (cargo run -p xtask -- wild-gauntlet) — not tracker meta\",\n  \
             \"cards\": [\n",
            tavernlab_json::escape(name),
            tavernlab_json::escape(tavernlab_core::gauntlet::class_name(*class))
        );
        for (j, (card, n)) in cards.iter().enumerate() {
            let _ = write!(
                out,
                "   [{}, {n}]{}\n",
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

    let mut msg = format!("{} Wild baseline decks -> {}", decks.len(), path.display());
    if !skipped.is_empty() {
        let names: Vec<String> = skipped.iter().map(|c| format!("{c:?}")).collect();
        let _ = write!(
            msg,
            "\n  {} class(es) skipped, too few implemented Wild cards: {}",
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
