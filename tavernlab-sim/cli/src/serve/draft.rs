//! The Arena draft endpoints.
//!
//! Two questions, two handlers, two standards of proof.
//!
//! `POST /api/arena/draft` is the cheap one: counters over the cards already
//! picked — curve, Taunt, weapons, the text-read approximations — plus, for
//! each typed candidate, whether the engine could simulate it at all. It
//! answers instantly and never simulates.
//!
//! `POST /api/arena/pick` is the measured one: complete the draft around
//! each candidate and play it against the Arena field
//! (`core::arena::compare_picks`). It runs as a job, and it refuses to run
//! unless every candidate is implemented — a pick compared by dropping the
//! cards the engine cannot play would be a number about a different draft
//! (docs/ARENA_RESEARCH.md §5.2).
//!
//! The counters that read card text (`removal`, `AOE`, `draw`) are string
//! matches over the printed English text, the same trick every draft helper
//! uses. They are labelled approximate in the UI and never feed a
//! simulation.

use std::path::Path;
use std::sync::Arc;

use tavernlab_core::arena::{PickBudget, compare_picks};
use tavernlab_core::cards::{CardId, Class, Formats, Kind, Keywords, by_name};
use tavernlab_core::deck::DECK_SIZE;
use tavernlab_core::gauntlet::class_name;
use tavernlab_json::{Json, to_string};

use super::api::{body, clamped, field_str, find_card, started, strings};
use super::http::{Request, Response};
use super::state::App;

/// Games per field deck per tail a pick run may ask for.
const PICK_MIN: usize = 10;
const PICK_MAX: usize = 200;

/// A typed list of names resolved against the corpus, stopping at the first
/// name that means nothing.
fn resolve_names(names: &[String]) -> Result<Vec<CardId>, String> {
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        match find_card(name) {
            Some(c) => out.push(c),
            None => return Err(format!("unknown card: {name}")),
        }
    }
    Ok(out)
}

/// Legendary Groups seen in our own drafts (`data/legendary_groups.json`).
/// The first pick is a package; later offers of the same legendary are not.
fn load_groups(root: &Path) -> Vec<(CardId, Vec<CardId>)> {
    let src = match std::fs::read_to_string(root.join("data/legendary_groups.json")) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let Ok(doc) = Json::parse(&src) else {
        return Vec::new();
    };
    let Some(groups) = doc.get("groups").and_then(Json::as_object) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (name, arr) in groups {
        let Some(leg) = by_name(name) else { continue };
        let mut support = Vec::new();
        for item in arr.as_array().unwrap_or(&[]) {
            let Some(n) = item.as_str() else { continue };
            if let Some(c) = by_name(n) {
                support.push(c);
            }
        }
        if !support.is_empty() {
            out.push((leg, support));
        }
    }
    out
}

fn group_of<'a>(groups: &'a [(CardId, Vec<CardId>)], card: CardId) -> &'a [CardId] {
    groups
        .iter()
        .find(|(leg, _)| *leg == card)
        .map(|(_, s)| s.as_slice())
        .unwrap_or(&[])
}

/// Five cards a full Underground deck would rather cut, by curve and
/// keywords — not by simulation. Stated as a hint: §5.1, not a pick ranking.
fn weakest_five(picked: &[CardId]) -> Vec<CardId> {
    if picked.len() < DECK_SIZE {
        return Vec::new();
    }
    let mut idx: Vec<usize> = (0..picked.len()).collect();
    idx.sort_by_key(|&i| cut_score(picked[i]));
    idx.into_iter().take(5).map(|i| picked[i]).collect()
}

fn cut_score(c: CardId) -> i32 {
    let d = c.def();
    let mut s = 10 - d.cost as i32;
    if d.kind() == Kind::Minion && d.cost == 2 {
        s += 4;
    }
    if d.keywords.has(Keywords::TAUNT) {
        s += 2;
    }
    if d.kind() == Kind::Weapon {
        s += 2;
    }
    s
}

fn class_of(payload: &tavernlab_json::Json) -> Result<Class, Response> {
    tavernlab_core::gauntlet::class_by_name(field_str(payload, "class")).ok_or_else(|| {
        Response::error(
            400,
            &format!("unknown class: {}", field_str(payload, "class")),
        )
    })
}

/// `POST /api/arena/draft` — counters over the picks so far, and whether
/// each candidate could be simulated.
pub fn draft(app: &App, req: &Request) -> Response {
    let payload = body(req);
    let class = match class_of(&payload) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let picked = match resolve_names(&strings(&payload, "picked")) {
        Ok(v) => v,
        Err(e) => return Response::error(400, &e),
    };
    let candidates = match resolve_names(&strings(&payload, "candidates")) {
        Ok(v) => v,
        Err(e) => return Response::error(400, &e),
    };
    let groups = load_groups(&app.root);

    // Curve by cost, 0..=6 and 7+. Counted over every pick, and minions of
    // cost two separately: "a 2-cost card" and "a 2-drop you can lead with"
    // are different claims, and the second is the one a tempo format cares
    // about.
    let mut curve = [0u32; 8];
    let mut two_drop_minions = 0u32;
    let mut taunts = 0u32;
    let mut weapons = 0u32;
    let mut hard_removal = 0u32;
    let mut damage_spells = 0u32;
    let mut aoe = 0u32;
    let mut draw = 0u32;
    let mut runes = (0u8, 0u8, 0u8);
    let mut unimplemented: Vec<CardId> = Vec::new();
    for &c in &picked {
        let d = c.def();
        curve[(d.cost.max(0) as usize).min(7)] += 1;
        if d.kind() == Kind::Minion && d.cost == 2 {
            two_drop_minions += 1;
        }
        if d.keywords.has(Keywords::TAUNT) {
            taunts += 1;
        }
        if d.kind() == Kind::Weapon {
            weapons += 1;
        }
        let text = c.info().text;
        if text.contains("Destroy a") || text.contains("Destroy an") {
            hard_removal += 1;
        }
        if d.kind() == Kind::Spell && text.contains("damage") {
            damage_spells += 1;
        }
        if text.contains("all enemy minions") || text.contains("all minions") {
            aoe += 1;
        }
        if text.contains("Draw ") {
            draw += 1;
        }
        runes.0 = runes.0.max(d.runes.blood());
        runes.1 = runes.1.max(d.runes.frost());
        runes.2 = runes.2.max(d.runes.unholy());
        if !tavernlab_core::cards::is_implemented(c) {
            unimplemented.push(c);
        }
    }

    let season = tavernlab_core::cards::arena_pool_present();
    let cuts = weakest_five(&picked);
    // A Legendary Group is the first pick. Later in the draft the same
    // legendary is just a card; expanding it then would invent four cards
    // the client did not offer.
    let first_pick = picked.is_empty();
    Response::json(
        200,
        to_string(|o| {
            o.obj(|o| {
                o.str_field("class", class_name(class));
                o.int_field("picked", picked.len() as i64);
                o.int_field("deck_size", DECK_SIZE as i64);
                o.bool_field("season_pool", season);
                o.field("curve", |v| {
                    v.arr(|a| {
                        for n in curve {
                            a.item(|v| v.int(n as i64));
                        }
                    })
                });
                o.int_field("two_drop_minions", two_drop_minions as i64);
                o.int_field("taunts", taunts as i64);
                o.int_field("weapons", weapons as i64);
                o.int_field("hard_removal", hard_removal as i64);
                o.int_field("damage_spells", damage_spells as i64);
                o.int_field("aoe", aoe as i64);
                o.int_field("draw", draw as i64);
                o.field("runes", |v| {
                    v.arr(|a| {
                        a.item(|v| v.int(runes.0 as i64));
                        a.item(|v| v.int(runes.1 as i64));
                        a.item(|v| v.int(runes.2 as i64));
                    })
                });
                o.field("unimplemented_picked", |v| {
                    v.arr(|a| {
                        for c in &unimplemented {
                            a.str_item(c.name());
                        }
                    })
                });
                o.field("cuts", |v| {
                    v.arr(|a| {
                        for c in &cuts {
                            a.str_item(c.name());
                        }
                    })
                });
                o.field("candidates", |v| {
                    v.arr(|a| {
                        for &c in &candidates {
                            a.item(|v| {
                                v.obj(|o| {
                                    o.str_field("name", c.name());
                                    o.int_field("cost", c.def().cost as i64);
                                    o.bool_field(
                                        "implemented",
                                        tavernlab_core::cards::is_implemented(c),
                                    );
                                    o.bool_field(
                                        "in_season",
                                        c.def().formats.has(Formats::ARENA),
                                    );
                                    o.field("group", |v| {
                                        v.arr(|a| {
                                            if first_pick {
                                                for s in group_of(&groups, c) {
                                                    a.str_item(s.name());
                                                }
                                            }
                                        })
                                    });
                                })
                            });
                        }
                    })
                });
            })
        }),
    )
}

/// `POST /api/arena/pick` — complete the draft around each candidate and
/// play it against the Arena field. A job: three candidates cost a couple
/// of thousand games.
pub fn pick(app: &Arc<App>, req: &Request) -> Response {
    let payload = body(req);
    let class = match class_of(&payload) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let picked_names = strings(&payload, "picked");
    let candidate_names = strings(&payload, "candidates");
    let games = clamped(&payload, "games", 25, PICK_MIN, PICK_MAX);
    let app = Arc::clone(app);
    let id = app.clone().jobs.start(move |p| {
        if !tavernlab_core::cards::arena_pool_present() {
            return Err(
                "the corpus carries no Arena season pool — write data/arena_season.json \
                 and rerun `cargo run -p xtask -- cards`"
                    .into(),
            );
        }
        let picked = resolve_names(&picked_names)?;
        let candidates = resolve_names(&candidate_names)?;
        if candidates.is_empty() {
            return Err("no candidates to compare".into());
        }
        // §5.2: all candidates simulated, or none. A comparison where one
        // side is a real card and the other a blank would rank the blank.
        let dead: Vec<&str> = candidates
            .iter()
            .filter(|c| !tavernlab_core::cards::is_implemented(**c))
            .map(|c| c.name())
            .collect();
        if !dead.is_empty() {
            return Err(format!(
                "the engine does not play: {} — no comparison rather than a wrong one",
                dead.join(", ")
            ));
        }
        // Picks already made are different: a real draft at 42% coverage
        // will hold cards the engine cannot play, and refusing would make
        // every later pick unanswerable. They leave the simulated deck --
        // their slots go to the random tail -- and the answer says so; the
        // comparison between candidates stays fair because every candidate
        // loses the same cards.
        let (sim_picked, dropped): (Vec<CardId>, Vec<CardId>) = picked
            .iter()
            .partition(|c| tavernlab_core::cards::is_implemented(**c));
        if !dropped.is_empty() {
            p.say(format!(
                "not simulated ({} of your picks the engine does not play; the random \
                 tail covers their slots): {}",
                dropped.len(),
                dropped
                    .iter()
                    .map(|c| c.name())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        let picked = sim_picked;
        let groups = load_groups(&app.root);
        let first_pick = picked_names.is_empty();
        let extras: Vec<Vec<CardId>> = if first_pick {
            candidates
                .iter()
                .map(|&c| {
                    group_of(&groups, c)
                        .iter()
                        .copied()
                        .filter(|s| tavernlab_core::cards::is_implemented(*s))
                        .collect()
                })
                .collect()
        } else {
            vec![Vec::new(); candidates.len()]
        };
        let dropped_group: Vec<String> = if first_pick {
            candidates
                .iter()
                .flat_map(|&c| group_of(&groups, c).iter().copied())
                .filter(|s| !tavernlab_core::cards::is_implemented(*s))
                .map(|c| c.name().to_string())
                .collect()
        } else {
            Vec::new()
        };
        if !dropped_group.is_empty() {
            p.say(format!(
                "Legendary Group support the engine does not play (dropped from the package): {}",
                dropped_group.join(", ")
            ));
        }
        let field = app.gauntlet("arena");
        if field.iter().filter(|d| d.playable()).count() == 0 {
            return Err(
                "no Arena field — run `cargo run -p xtask -- arena-gauntlet`".into(),
            );
        }
        let budget = PickBudget {
            games_per_deck: games,
            threads: app.threads,
            ..PickBudget::default()
        };
        p.say(format!(
            "{}: {} picked, {} candidate(s), {} tails x {} games x {} field decks each",
            class_name(class),
            picked.len(),
            candidates.len(),
            budget.tails,
            budget.games_per_deck,
            field.iter().filter(|d| d.playable()).count()
        ));
        let scores = compare_picks(class, &picked, &candidates, &extras, &field, &budget)
            .map_err(str::to_string)?;
        let total: u32 = scores.iter().map(|s| s.games).sum();
        app.count_games(total as u64);
        p.say(format!("done: {total} games"));
        let real = scores.iter().map(|s| s.real_cards).max().unwrap_or(picked.len() + 1);
        Ok(to_string(|o| {
            o.obj(|o| {
                o.str_field("class", class_name(class));
                // How much of each simulated deck was actual picks: the
                // rest is a random tail, and early in a draft that caveat
                // is most of the answer. A Legendary Group raises this
                // on the first pick.
                o.int_field("real_cards", real as i64);
                o.int_field("deck_size", DECK_SIZE as i64);
                o.field("dropped_picked", |v| {
                    v.arr(|a| {
                        for c in &dropped {
                            a.str_item(c.name());
                        }
                    })
                });
                o.field("scores", |v| {
                    v.arr(|a| {
                        for s in &scores {
                            a.item(|v| {
                                v.obj(|o| {
                                    o.str_field("card", s.card.name());
                                    o.field("winrate", |v| match s.winrate {
                                        Some(w) => v.round(w, 4),
                                        None => v.null(),
                                    });
                                    o.int_field("games", s.games as i64);
                                    o.int_field("real_cards", s.real_cards as i64);
                                    o.field("group", |v| {
                                        v.arr(|a| {
                                            for c in &s.extra {
                                                a.str_item(c.name());
                                            }
                                        })
                                    });
                                })
                            });
                        }
                    })
                });
            })
        }))
    });
    started(&id)
}
