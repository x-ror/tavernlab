//! Filling the holes the card corpus points at.
//!
//! The corpus names tokens it does not contain. `Cannonmaster` says its
//! Battlecry gets you card `127012` and the corpus has no `127012`, so the
//! card cannot be implemented at all: there is nothing to summon. Twenty
//! entries across ten cards were in that state, and every one of them exists
//! in Blizzard's own card library.
//!
//! The reason they are missing is that the pipeline that built the corpus went
//! away with the Python backend. This does not rebuild it. It does the one
//! thing the engine actually needs, and does it additively: **only cards the
//! corpus already references and does not have are added.** Nothing existing
//! is touched, nothing speculative is imported, and 3049 Battlegrounds cards
//! the library also serves stay out because nothing points at them.
//!
//! # The numeric ids
//!
//! The library serves `cardTypeId: 4` where the corpus says `"type": "MINION"`.
//! The old builder learned those maps by joining against CardDefs.xml; this
//! learns them by joining against the corpus itself — nine thousand cards
//! appear in both, which is more than enough to pin six small maps. A map that
//! comes out ambiguous is a hard error rather than a majority vote, because a
//! silently wrong set id would mislabel a card's format.
//!
//! # The dump
//!
//! Fetched by hand, once per set, and never by the app:
//!
//! ```sh
//! for p in $(seq 1 25); do
//!   curl -s "https://hearthstone.blizzard.com/en-us/api/cards\
//! ?locale=en-us&class=all&collectible=0,1&pageSize=500&page=$p"
//! done > dump.json     # one JSON object per line is fine; see `read_dump`
//! ```
//!
//! ```text
//! cargo run -p xtask -- backfill dump.json
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use tavernlab_json::Json;

/// One numeric-id map learned from the overlap.
struct Learned {
    /// `id` in the library, the corpus's word for it.
    map: BTreeMap<i64, String>,
}

/// Read the dump in any of the shapes a hand-run fetch produces: one
/// `{"cards":[…]}` response, a bare array, several responses one per line, or
/// one card object per line.
fn read_dump(src: &str) -> Result<Vec<Json>, String> {
    fn cards_of(doc: Json) -> Vec<Json> {
        if let Some(cards) = doc.get("cards").and_then(Json::as_array) {
            return cards.to_vec();
        }
        if let Some(arr) = doc.as_array() {
            return arr.to_vec();
        }
        // A bare card object, which is what one-per-line dumps hold.
        if doc.get("id").is_some() {
            return vec![doc];
        }
        Vec::new()
    }

    let mut out = Vec::new();
    // A whole-file parse first: the common case is a single response.
    if let Ok(doc) = Json::parse(src.trim()) {
        out = cards_of(doc);
        if !out.is_empty() {
            return Ok(out);
        }
    }
    for (i, line) in src.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let doc = Json::parse(line).map_err(|e| format!("line {}: {e}", i + 1))?;
        out.extend(cards_of(doc));
    }
    if out.is_empty() {
        return Err("the dump holds no cards".into());
    }
    Ok(out)
}

fn int(v: &Json, key: &str) -> Option<i64> {
    v.get(key).and_then(|x| x.as_i64())
}

fn text(v: &Json, key: &str) -> Option<String> {
    v.get(key).and_then(|x| x.as_str()).map(str::to_string)
}

/// Learn `id -> word` from every card that is in both the library and the
/// corpus. Ambiguity is an error: the whole point is that these are exact.
fn learn(
    name: &'static str,
    id_key: &str,
    corpus_key: &str,
    api: &[Json],
    ours: &BTreeMap<i64, Json>,
) -> Result<Learned, String> {
    let mut seen: BTreeMap<i64, BTreeSet<String>> = BTreeMap::new();
    for c in api {
        let (Some(id), Some(dbf)) = (int(c, id_key), int(c, "id")) else {
            continue;
        };
        let Some(mine) = ours.get(&dbf) else { continue };
        let word = match mine.get(corpus_key).and_then(|x| x.as_str()) {
            Some(w) => w.to_string(),
            None => continue,
        };
        seen.entry(id).or_default().insert(word);
    }
    let mut map = BTreeMap::new();
    for (id, words) in seen {
        if words.len() > 1 {
            return Err(format!(
                "{name}: id {id} maps to {:?} in the corpus — the join is not exact, \
                 so nothing is written",
                words
            ));
        }
        map.insert(id, words.into_iter().next().unwrap_or_default());
    }
    Ok(Learned { map })
}

impl Learned {
    fn get(&self, id: Option<i64>) -> Option<&str> {
        self.map.get(&id?).map(String::as_str)
    }
}

/// Keywords are lists on both sides, so they are learned only from the cards
/// that carry exactly one of each -- which is enough: seventy ids come out of
/// the overlap, unambiguously. An id that never appears alone cannot be
/// named, and [`run`] reports those rather than dropping them quietly.
fn learn_keywords(api: &[Json], ours: &BTreeMap<i64, Json>) -> Result<Learned, String> {
    let mut seen: BTreeMap<i64, BTreeSet<String>> = BTreeMap::new();
    for c in api {
        let Some(dbf) = int(c, "id") else { continue };
        let Some(mine) = ours.get(&dbf) else { continue };
        let ids = c.arr_or_empty("keywordIds");
        let words = mine.arr_or_empty("kw");
        if ids.len() == 1
            && words.len() == 1
            && let (Some(id), Some(w)) = (ids[0].as_i64(), words[0].as_str())
        {
            seen.entry(id).or_default().insert(w.to_string());
        }
    }
    let mut map = BTreeMap::new();
    for (id, words) in seen {
        if words.len() > 1 {
            return Err(format!("keywordId: id {id} maps to {words:?}"));
        }
        map.insert(id, words.into_iter().next().unwrap_or_default());
    }
    Ok(Learned { map })
}

/// Minion types are a list in the corpus, so they are learned only from the
/// cards that carry exactly one.
fn learn_races(api: &[Json], ours: &BTreeMap<i64, Json>) -> Result<Learned, String> {
    let mut seen: BTreeMap<i64, BTreeSet<String>> = BTreeMap::new();
    for c in api {
        let (Some(id), Some(dbf)) = (int(c, "minionTypeId"), int(c, "id")) else {
            continue;
        };
        let Some(mine) = ours.get(&dbf) else { continue };
        let races: Vec<&str> = mine
            .get("races")
            .and_then(|r| r.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str()).collect())
            .unwrap_or_default();
        if races.len() == 1 {
            seen.entry(id).or_default().insert(races[0].to_string());
        }
    }
    let mut map = BTreeMap::new();
    for (id, words) in seen {
        if words.len() > 1 {
            return Err(format!("minionTypeId: id {id} maps to {words:?}"));
        }
        map.insert(id, words.into_iter().next().unwrap_or_default());
    }
    Ok(Learned { map })
}

/// The corpus keeps card text without markup.
fn clean(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let mut chars = t.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '<' {
            // Drop a tag whole: <b>, </b>, <i>, </i>.
            for d in chars.by_ref() {
                if d == '>' {
                    break;
                }
            }
            continue;
        }
        if c == '[' && chars.peek() == Some(&'x') {
            let mut probe = chars.clone();
            probe.next();
            if probe.next() == Some(']') {
                chars.next();
                chars.next();
                continue;
            }
        }
        out.push(if c == '\n' || c == '\t' { ' ' } else { c });
    }
    // Collapse runs of spaces, the way the corpus has them.
    let mut squeezed = String::with_capacity(out.len());
    let mut space = false;
    for c in out.chars() {
        if c == ' ' {
            space = true;
            continue;
        }
        if space && !squeezed.is_empty() {
            squeezed.push(' ');
        }
        space = false;
        squeezed.push(c);
    }
    squeezed
}

pub fn run(root: &Path, dump: &Path) -> Result<String, String> {
    let std_path = root.join("data/standard_cards.json");
    let wild_path = root.join("data/wild_cards.json");
    let std_src = std::fs::read_to_string(&std_path)
        .map_err(|e| format!("{}: {e}", std_path.display()))?;
    let std_doc = Json::parse(&std_src).map_err(|e| format!("{}: {e}", std_path.display()))?;
    let wild_doc = if wild_path.exists() {
        let s = std::fs::read_to_string(&wild_path)
            .map_err(|e| format!("{}: {e}", wild_path.display()))?;
        Some(Json::parse(&s).map_err(|e| format!("{}: {e}", wild_path.display()))?)
    } else {
        None
    };

    // Everything the corpus already has, by dbf id.
    let mut ours: BTreeMap<i64, Json> = BTreeMap::new();
    for doc in [Some(&std_doc), wild_doc.as_ref()].into_iter().flatten() {
        for (_, v) in doc.as_object().unwrap_or_default() {
            if let Some(dbf) = int(v, "dbf") {
                ours.insert(dbf, v.clone());
            }
        }
    }

    // Every token the corpus points at and does not hold.
    let mut wanted: BTreeSet<i64> = BTreeSet::new();
    for doc in [Some(&std_doc), wild_doc.as_ref()].into_iter().flatten() {
        for (_, v) in doc.as_object().unwrap_or_default() {
            for ch in v.arr_or_empty("child").iter().filter_map(Json::as_i64) {
                if !ours.contains_key(&ch) {
                    wanted.insert(ch);
                }
            }
        }
    }
    if wanted.is_empty() {
        return Ok("nothing to backfill: the corpus holds every card it names".into());
    }

    let dump_src =
        std::fs::read_to_string(dump).map_err(|e| format!("{}: {e}", dump.display()))?;
    let api = read_dump(&dump_src)?;

    let types = learn("cardTypeId", "cardTypeId", "type", &api, &ours)?;
    let classes = learn("classId", "classId", "cls", &api, &ours)?;
    let sets = learn("cardSetId", "cardSetId", "set", &api, &ours)?;
    let rarities = learn("rarityId", "rarityId", "rarity", &api, &ours)?;
    let schools = learn("spellSchoolId", "spellSchoolId", "school", &api, &ours)?;
    let races = learn_races(&api, &ours)?;
    let keywords = learn_keywords(&api, &ours)?;

    let mut added: BTreeMap<String, String> = BTreeMap::new();
    let mut unknown: Vec<String> = Vec::new();
    // Keyword ids the overlap cannot name. Reported, never dropped in
    // silence: the card's own text still says what they are, and a reader of
    // this output has to know the list is short of one.
    let mut unnamed: Vec<String> = Vec::new();
    for c in &api {
        let Some(dbf) = int(c, "id") else { continue };
        if !wanted.contains(&dbf) {
            continue;
        }
        let Some(kind) = types.get(int(c, "cardTypeId")) else {
            unknown.push(format!("dbf {dbf}: unknown cardTypeId"));
            continue;
        };
        let Some(set) = sets.get(int(c, "cardSetId")) else {
            unknown.push(format!("dbf {dbf}: unknown cardSetId"));
            continue;
        };
        // No CardDefs entry means no string card id; the library's slug is
        // the only stable name it has, and the corpus already holds cards
        // keyed that way.
        let id = text(c, "slug").unwrap_or_else(|| dbf.to_string());
        let hp = int(c, "health").unwrap_or(0);
        // The library keeps a weapon's durability and a location's health in
        // the same field the corpus calls `dur`.
        let dur = match int(c, "durability") {
            Some(d) => d,
            None if kind == "WEAPON" || kind == "LOCATION" => hp,
            None => 0,
        };
        let race = races.get(int(c, "minionTypeId"));
        let mut kw: Vec<&str> = Vec::new();
        for id in c.arr_or_empty("keywordIds").iter().filter_map(Json::as_i64) {
            match keywords.get(Some(id)) {
                Some(w) => kw.push(w),
                None => unnamed.push(format!(
                    "dbf {dbf} ({}) carries keyword id {id}, which no card in the corpus \
                     carries alone -- its text is the only record of it",
                    text(c, "name").unwrap_or_default()
                )),
            }
        }
        let school = schools.get(int(c, "spellSchoolId"));
        let rarity = rarities.get(int(c, "rarityId"));
        let body = tavernlab_json::to_string(|o| {
            o.obj(|o| {
                o.str_field("id", &id);
                o.int_field("dbf", dbf);
                o.str_field("name", &text(c, "name").unwrap_or_default());
                o.str_field("type", kind);
                o.str_field("cls", classes.get(int(c, "classId")).unwrap_or("NEUTRAL"));
                o.int_field("cost", int(c, "manaCost").unwrap_or(0));
                o.int_field("atk", int(c, "attack").unwrap_or(0));
                o.int_field("hp", hp);
                o.int_field("dur", dur);
                o.int_field("armor", int(c, "armor").unwrap_or(0));
                o.field("races", |v| {
                    v.arr(|a| {
                        if let Some(r) = race {
                            a.item(|v| v.str(r));
                        }
                    })
                });
                o.field("school", |v| match school {
                    Some(s) => v.str(s),
                    None => v.null(),
                });
                o.field("mech", |v| v.arr(|_| {}));
                // The library's own keyword list. The corpus keeps it under
                // `kw`, as the fallback for exactly this case: a card
                // CardDefs has never described.
                o.field("kw", |v| {
                    v.arr(|a| {
                        for w in &kw {
                            a.item(|v| v.str(w));
                        }
                    })
                });
                o.str_field("text", &clean(&text(c, "text").unwrap_or_default()));
                o.bool_field("coll", int(c, "collectible").unwrap_or(0) == 1);
                o.field("rarity", |v| match rarity {
                    Some(r) => v.str(r),
                    None => v.null(),
                });
                o.str_field("set", set);
                // The corpus marks a card the library served but CardDefs
                // never saw. Everything CardDefs would have contributed --
                // the string id, the mechanics list -- is absent here, and a
                // reader has to know that rather than infer it from an empty
                // list.
                o.bool_field("nodefs", true);
            })
        });
        added.insert(id, body);
    }

    if !unknown.is_empty() {
        return Err(format!(
            "the dump holds cards whose ids the corpus cannot name:\n  {}",
            unknown.join("\n  ")
        ));
    }
    let found: BTreeSet<i64> = api
        .iter()
        .filter_map(|c| int(c, "id"))
        .filter(|d| wanted.contains(d))
        .collect();
    let missing: Vec<i64> = wanted.difference(&found).copied().collect();

    if added.is_empty() {
        return Err(format!(
            "the dump holds none of the {} cards the corpus is missing",
            wanted.len()
        ));
    }

    // Written into the Standard file, which is where the corpus keeps tokens
    // and hero powers; Wild is only the delta of what Wild adds.
    //
    // Spliced in as text rather than re-serialised. Re-rendering five
    // thousand entries to add twenty would put the whole file in the diff and
    // stake the result on this crate's writer agreeing with the old builder's
    // about every number and escape. Appending leaves every existing byte
    // exactly as it was.
    let close = std_src
        .rfind('}')
        .ok_or_else(|| format!("{} is not a JSON object", std_path.display()))?;
    let n = added.len();
    let mut tail = String::new();
    for (id, body) in &added {
        tail.push_str(", ");
        tail.push_str(&tavernlab_json::to_string(|o| o.str(id)));
        tail.push_str(": ");
        tail.push_str(body);
    }
    let rendered = format!("{}{}{}", &std_src[..close], tail, &std_src[close..]);
    Json::parse(&rendered).map_err(|e| format!("the spliced corpus does not parse: {e}"))?;
    std::fs::write(&std_path, rendered).map_err(|e| format!("{}: {e}", std_path.display()))?;

    let mut msg = format!(
        "added {n} card(s) the corpus named but did not hold, to {}",
        std_path.display()
    );
    if !missing.is_empty() {
        msg.push_str(&format!(
            "\n  still missing, and not in this dump: {missing:?}"
        ));
    }
    for u in &unnamed {
        msg.push_str(&format!("\n  keyword not carried across: {u}"));
    }
    msg.push_str("\n  now run: cargo run -p xtask -- cards");
    Ok(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markup_is_stripped_and_spaces_collapsed() {
        assert_eq!(clean("<b>Lifesteal</b>  <i>(for you)</i>"), "Lifesteal (for you)");
        assert_eq!(clean("[x]Deal 3\ndamage."), "Deal 3 damage.");
        assert_eq!(clean(""), "");
    }

    #[test]
    fn a_dump_is_read_whole_or_line_by_line() {
        let one = r#"{"cards":[{"id":1},{"id":2}]}"#;
        assert_eq!(read_dump(one).unwrap().len(), 2);
        let many = format!("{one}\n{one}");
        assert_eq!(read_dump(&many).unwrap().len(), 4);
        let bare = r#"[{"id":9}]"#;
        assert_eq!(read_dump(bare).unwrap().len(), 1);
        assert!(read_dump("").is_err());
    }

    #[test]
    fn an_ambiguous_map_is_refused_rather_than_voted_on() {
        // Two corpus cards that disagree about what type id 4 means. A
        // majority vote here would mislabel every card the map is used on.
        let api = vec![
            Json::parse(r#"{"id":1,"cardTypeId":4}"#).unwrap(),
            Json::parse(r#"{"id":2,"cardTypeId":4}"#).unwrap(),
        ];
        let mut ours = BTreeMap::new();
        ours.insert(1, Json::parse(r#"{"dbf":1,"type":"MINION"}"#).unwrap());
        ours.insert(2, Json::parse(r#"{"dbf":2,"type":"SPELL"}"#).unwrap());
        let Err(err) = learn("cardTypeId", "cardTypeId", "type", &api, &ours) else {
            panic!("an ambiguous map must be refused")
        };
        assert!(err.contains("not exact"), "{err}");
    }

    #[test]
    fn a_clean_join_learns_the_map() {
        let api = vec![
            Json::parse(r#"{"id":1,"cardTypeId":4}"#).unwrap(),
            Json::parse(r#"{"id":2,"cardTypeId":4}"#).unwrap(),
            Json::parse(r#"{"id":3,"cardTypeId":5}"#).unwrap(),
        ];
        let mut ours = BTreeMap::new();
        ours.insert(1, Json::parse(r#"{"dbf":1,"type":"MINION"}"#).unwrap());
        ours.insert(2, Json::parse(r#"{"dbf":2,"type":"MINION"}"#).unwrap());
        ours.insert(3, Json::parse(r#"{"dbf":3,"type":"SPELL"}"#).unwrap());
        let Ok(m) = learn("cardTypeId", "cardTypeId", "type", &api, &ours) else {
            panic!("a clean join must produce a map")
        };
        assert_eq!(m.get(Some(4)), Some("MINION"));
        assert_eq!(m.get(Some(5)), Some("SPELL"));
        assert_eq!(m.get(Some(6)), None);
    }
}
