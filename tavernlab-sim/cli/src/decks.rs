//! `tavernsim decks <file>` — what a list of real deck codes asks of the engine.
//!
//! The point is prioritisation. The backlog says which cards are missing; this
//! says which of them stand between the simulator and the decks people are
//! actually playing, ranked by how many of those decks each one blocks.
//!
//! The file format is the one a person gets by copying deck codes out of a
//! tracker: a `### name` heading, `# key: value` notes, then the code. Notes
//! are read for provenance and nothing else — in particular a `winrate:` line
//! is **not** read as a measurement and never reaches an answer. Numbers this
//! app reports come from its own games; that is U24 in `docs/DESIGN.md`, and a
//! win rate copied from a website would be exactly the thing it rules out.

use std::collections::BTreeMap;

use tavernlab_core::deckstring;

/// One block of the file.
pub struct Entry {
    pub name: String,
    /// As the file spells it: `DEATHKNIGHT`, `MAGE`.
    pub class: String,
    pub code: String,
}

/// Read the whole file. Malformed blocks are skipped rather than fatal: this
/// is a hand-kept file, and one bad paste should not hide the other thirty.
pub fn parse(src: &str) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut name = String::new();
    let mut class = String::new();
    for line in src.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("### ") {
            name = rest.to_string();
            class.clear();
        } else if let Some(rest) = line.strip_prefix('#') {
            // Every other note -- `archetype:`, `source:`, `url:`, and
            // `winrate:` above all -- is read as nothing. The archetype
            // repeats the heading, and a win rate copied from a website is
            // the one number this app must never report (U24).
            if let Some(v) = rest.trim().strip_prefix("class:") {
                class = v.trim().to_string();
            }
        } else if !line.is_empty() && !name.is_empty() {
            out.push(Entry {
                name: std::mem::take(&mut name),
                class: std::mem::take(&mut class),
                code: line.to_string(),
            });
        }
    }
    out
}

pub fn run(path: Option<&str>) {
    let path = path.unwrap_or("../data/hsreplay_decks.txt");
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("failed to read {path}: {e}");
            std::process::exit(1);
        }
    };
    let entries = parse(&src);
    if entries.is_empty() {
        eprintln!("{path} holds no deck blocks (expected `### name` then a code)");
        std::process::exit(1);
    }

    // How many of these decks each unimplemented card keeps off the field.
    // Our own count, over the lists in this file -- not a popularity figure
    // borrowed from wherever the codes came from.
    let mut blocks: BTreeMap<String, usize> = BTreeMap::new();
    // Per class, `(fieldable, total)`: which classes the simulator can meet
    // the real field in, and which it cannot meet at all.
    let mut per_class: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut fieldable = 0;
    println!("{}", path);
    for e in &entries {
        let class = if e.class.is_empty() {
            "?".to_string()
        } else {
            e.class.clone()
        };
        per_class.entry(class).or_default().1 += 1;
        match deckstring::resolve(&e.code) {
            Err(err) => println!("  ✗ {:32} не декодується: {err}", e.name),
            Ok(r) => {
                let mut why: Vec<String> = Vec::new();
                if !r.illegal.is_empty() {
                    why.push(format!("не в форматі: {}", crate::names::list(&r.illegal)));
                }
                if !r.unimplemented.is_empty() {
                    for c in &r.unimplemented {
                        *blocks.entry(c.to_string()).or_default() += 1;
                    }
                    why.push(crate::names::list(&r.unimplemented));
                }
                if why.is_empty() {
                    fieldable += 1;
                    if let Some(slot) = per_class.get_mut(e.class.as_str()) {
                        slot.0 += 1;
                    }
                    println!("  ✓ {:32} виставляється", e.name);
                } else {
                    println!("  ✗ {:32} {}", e.name, why.join("; "));
                }
            }
        }
    }

    println!(
        "\n{fieldable} з {} колод виставляється; {} карт стоять на заваді",
        entries.len(),
        blocks.len()
    );
    println!("\nпо класах:");
    for (class, (ok, all)) in &per_class {
        println!("  {class:14} {ok}/{all}");
    }
    if blocks.is_empty() {
        return;
    }
    let mut ranked: Vec<(String, usize)> = blocks.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    println!("\nщо реалізувати першим — за числом колод, які карта розблокує:");
    for (card, n) in ranked.iter().take(25) {
        let text = tavernlab_core::cards::by_name(card)
            .map(|c| {
                let d = c.def();
                format!("({}) {}", d.cost, short(c.info().text))
            })
            .unwrap_or_else(|| "немає в корпусі".into());
        println!("  {n:2}  {card:28} {text}");
    }
    if ranked.len() > 25 {
        println!("  … і ще {}", ranked.len() - 25);
    }
}

fn short(text: &str) -> String {
    let clean = text.replace('\n', " ");
    if clean.chars().count() <= 70 {
        return clean;
    }
    clean.chars().take(69).collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# a comment about where these came from
### Corpse Death Knight
# class: DEATHKNIGHT
# archetype: Corpse Death Knight
# winrate: 60.31
# games: 1867
AAECAfHhBALtnweI3QcOkeQEhfYEsvcE1J4G1uUGyIwHupUHopcH0q0H4rEH/78H9sEHrNoHptwHAAA=

### Egg Death Knight
# class: DEATHKNIGHT
AAECAfHhBAjDgwf1mAfsmwfXnQftnwfSrged2we/3wcLodQEl4IHvJQHupUH4J0Hn54H4rEH2tcHrtoHtNoHptwHAAA=
";

    #[test]
    fn a_block_is_a_heading_some_notes_and_a_code() {
        let out = parse(SAMPLE);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].name, "Corpse Death Knight");
        assert_eq!(out[0].class, "DEATHKNIGHT");
        assert!(out[0].code.starts_with("AAECAfHhBALtnwe"));
        assert_eq!(out[1].name, "Egg Death Knight");
        assert_eq!(out[1].class, "DEATHKNIGHT");
    }

    #[test]
    fn the_codes_in_the_sample_really_decode() {
        for e in parse(SAMPLE) {
            assert!(
                deckstring::resolve(&e.code).is_ok(),
                "{} did not decode",
                e.name
            );
        }
    }

    #[test]
    fn a_winrate_note_is_read_as_nothing() {
        // It is provenance in the file and must never become an answer here:
        // every number this app reports comes from its own games (U24).
        let out = parse(SAMPLE);
        assert_eq!(out[0].class, "DEATHKNIGHT");
        // `Entry` has nowhere to put one, which is the point: the class is
        // the only note read, so no answer built from this file can carry a
        // win rate somebody else measured.
    }
}
