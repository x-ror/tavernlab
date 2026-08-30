//! The `/api` handlers.
//!
//! Every one of them is a function of the request and the card table: there
//! is no database and nothing is sent anywhere. The only state that outlives
//! a request is the user's settings, the cached tier table they asked for,
//! and an in-memory telemetry cache that exists purely so a second question
//! about the same deck does not replay the same games.
//!
//! Two rules hold across all of them.
//!
//! **A deck the engine cannot field is never quietly approximated.** If a
//! list holds a card that is not implemented, not legal in its format, or not
//! in the corpus at all, the answer names the cards and stops. Dropping them
//! would move the win rate by an unknown amount and report it as a
//! measurement.
//!
//! **Numbers carry their sample.** Every rate is published with the number of
//! games behind it, and the gauntlet decks that could not be fielded are
//! listed rather than silently left out of an average.

use std::sync::Arc;

use tavernlab_core::agent::Style;
use tavernlab_core::batch::{Contender, Policy};
use tavernlab_core::cards::{CardId, Class, Formats, Kind, by_name, is_implemented};
use tavernlab_core::deckstring::{self, Resolved};
use tavernlab_core::gauntlet::{self, MetaDeck, Unfieldable, class_name};
use tavernlab_core::optimize::{self, Budget};
use tavernlab_core::telemetry::Verdict;
use tavernlab_core::tiers;
use tavernlab_json::{Json, Out, to_string};

use super::http::{Request, Response};
use super::state::{App, format_by_name};
use crate::watch_mod::advice::write_line;

/// Games per opponent a rating run may ask for.
const ANALYZE_MIN: usize = 100;
const ANALYZE_MAX: usize = 10_000;
/// Games per ordered pair a tier run may ask for.
const TIERS_MIN: usize = 20;
const TIERS_MAX: usize = 2_000;
/// Below this many appearances a per-card delta is noise, and is reported as
/// "no answer" rather than as a number.
const MIN_APPEARANCES: u32 = 30;
/// The same floor for the coach screen, which averages across matchups and
/// so needs each one to mean something.
const MIN_DRAWS: u32 = 100;

// ----------------------------------------------------------------- helpers

fn body(req: &Request) -> Json {
    Json::parse(req.body_str()).unwrap_or(Json::Null)
}

fn field_str<'a>(j: &'a Json, key: &str) -> &'a str {
    j.str_or_empty(key)
}

fn strings(j: &Json, key: &str) -> Vec<String> {
    j.arr_or_empty(key)
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .filter(|s| !s.trim().is_empty())
        .collect()
}

fn clamped(j: &Json, key: &str, default: usize, lo: usize, hi: usize) -> usize {
    match j.get(key).and_then(Json::as_i64) {
        Some(n) if n > 0 => (n as usize).clamp(lo, hi),
        _ => default,
    }
}

/// The format a request asked for, defaulting to Standard.
fn format_of(j: &Json) -> (&'static str, Formats) {
    format_by_name(field_str(j, "format")).unwrap_or(("standard", Formats::STANDARD))
}

/// A deck code read against the table, together with the field it will be
/// scored against.
struct Loaded {
    resolved: Resolved,
    format_name: &'static str,
    format: Formats,
    field: Arc<Vec<MetaDeck>>,
    /// True when the deck code did not say which format it is for, so
    /// Standard was assumed. Every answer built on it has to say so.
    assumed_format: bool,
}

impl Loaded {
    fn style(&self) -> Style {
        Style::Midrange
    }

    fn contender(&self) -> Contender<'_> {
        Contender {
            class: self.resolved.class,
            cards: &self.resolved.ids,
            style: self.style(),
        }
    }

    fn playable_field(&self) -> usize {
        self.field.iter().filter(|d| d.playable()).count()
    }
}

/// Resolve a pasted code and load its gauntlet, or explain what is wrong in
/// the words the player needs.
fn load(app: &App, code: &str) -> Result<Loaded, String> {
    let resolved = deckstring::resolve(code).map_err(|e| e.to_string())?;
    if !resolved.illegal.is_empty() {
        return Err(format!(
            "not legal in {}: {}",
            resolved
                .format
                .map(super::state::format_name)
                .unwrap_or("this format"),
            crate::names::list(&resolved.illegal)
        ));
    }
    if !resolved.unimplemented.is_empty() {
        return Err(format!(
            "the simulator cannot play these cards: {}",
            crate::names::list(&resolved.unimplemented)
        ));
    }
    if !resolved.missing.is_empty() {
        return Err(format!(
            "no card in the corpus has these dbf ids: {}",
            resolved
                .missing
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !resolved.not_deckable.is_empty() {
        return Err(format!(
            "these cannot go in a deck: {}",
            crate::names::list(&resolved.not_deckable)
        ));
    }
    let assumed_format = resolved.format.is_none();
    let format = resolved.format.unwrap_or(Formats::STANDARD);
    let format_name = super::state::format_name(format);
    let field = app.gauntlet(format_name);
    if field.is_empty() {
        return Err(format!(
            "no gauntlet for {format_name} ({})",
            app.gauntlet_path(format_name).display()
        ));
    }
    Ok(Loaded {
        resolved,
        format_name,
        format,
        field,
        assumed_format,
    })
}

/// The cache key for a deck's telemetry: the bare code, so the same deck
/// pasted with and without its comment block is one deck.
fn deck_key(code: &str) -> String {
    deckstring::extract(code).unwrap_or(code.trim()).to_string()
}

fn write_rates(o: &mut Out, rates: &gauntlet::Rates) {
    o.obj(|o| {
        for (name, rate) in &rates.per_deck {
            o.field(name, |v| v.round(*rate, 4));
        }
    });
}

/// The gauntlet decks that could not be fielded, and why — attached to every
/// answer that averages over the field.
fn write_field_note(o: &mut tavernlab_json::ObjOut<'_>, field: &[MetaDeck]) {
    let played = field.iter().filter(|d| d.playable()).count();
    o.int_field("field_decks", field.len() as i64);
    o.int_field("field_played", played as i64);
    o.field("field_skipped", |v| {
        v.arr(|a| {
            for deck in field.iter().filter(|d| !d.playable()) {
                a.item(|v| write_unfieldable(v, deck));
            }
        })
    });
}

/// One deck that is not fielded, and which of the two reasons it is.
///
/// `cards` means the engine cannot play something in the list — a coverage
/// gap. `size` means the list is not thirty cards — an incomplete entry in
/// the gauntlet file, which implementing cards will never fix. The front end
/// says them differently because they are different news.
fn write_unfieldable(o: &mut Out, deck: &MetaDeck) {
    o.obj(|o| {
        o.str_field("deck", &deck.name);
        o.str_field(
            "why",
            match deck.problem() {
                Some(Unfieldable::Size) => "size",
                _ => "cards",
            },
        );
        o.int_field("listed", deck.listed as i64);
        o.field("cards", |v| {
            v.arr(|a| {
                for (name, n) in &deck.missing {
                    a.item(|v| {
                        v.arr(|a| {
                            a.str_item(name);
                            a.item(|v| v.int(*n as i64));
                        })
                    });
                }
            })
        });
    });
}

// ---------------------------------------------------------------- handlers

/// `POST /api/resolve` — can we simulate this list, and if not, why not.
pub fn resolve(app: &App, req: &Request) -> Response {
    let payload = body(req);
    let code = field_str(&payload, "code");
    let name = deckstring::deck_name(code);
    let r = match deckstring::resolve(code) {
        Ok(r) => r,
        Err(e) => {
            return Response::json(
                200,
                to_string(|o| {
                    o.obj(|o| {
                        o.bool_field("ok", false);
                        // The key is what the front end translates; the text
                        // is the same thing in English, for anything that
                        // logs the answer rather than showing it.
                        o.str_field("error_code", e.code());
                        o.str_field("error", &e.to_string());
                        o.field("name", |v| v.opt(name, |v, n| v.str(n)));
                    })
                }),
            );
        }
    };
    let format_name = r.format.map(super::state::format_name);
    let has_gauntlet = format_name
        .map(|f| !app.gauntlet(f).is_empty())
        .unwrap_or(false);
    Response::json(
        200,
        to_string(|o| {
            o.obj(|o| {
                o.bool_field("ok", r.playable());
                o.field("name", |v| v.opt(name, |v, n| v.str(n)));
                o.str_field("cls", class_name(r.class));
                o.int_field("total", r.total() as i64);
                o.field("format", |v| v.opt(format_name, |v, f| v.str(f)));
                o.bool_field("has_gauntlet", has_gauntlet);
                o.field("cards", |v| {
                    v.arr(|a| {
                        for (name, n) in &r.cards {
                            a.item(|v| {
                                v.arr(|a| {
                                    a.str_item(name);
                                    a.item(|v| v.int(*n as i64));
                                })
                            });
                        }
                    })
                });
                for (key, list) in [
                    ("unimplemented", &r.unimplemented),
                    ("illegal", &r.illegal),
                    ("not_deckable", &r.not_deckable),
                ] {
                    o.field(key, |v| {
                        v.arr(|a| {
                            for name in list.iter() {
                                a.str_item(name);
                            }
                        })
                    });
                }
                o.field("missing", |v| {
                    v.arr(|a| {
                        for dbf in &r.missing {
                            a.item(|v| v.int(*dbf as i64));
                        }
                    })
                });
            })
        }),
    )
}

/// `POST /api/analyze` — win rate against the gauntlet. A job, because the
/// UI shows its progress; the run itself takes well under a second.
pub fn analyze(app: &Arc<App>, req: &Request) -> Response {
    let payload = body(req);
    let code = field_str(&payload, "code").to_string();
    let games = clamped(&payload, "games", 1000, ANALYZE_MIN, ANALYZE_MAX);
    let app = Arc::clone(app);
    let id = app.clone().jobs.start(move |p| {
        let loaded = load(&app, &code)?;
        if loaded.assumed_format {
            p.say("the deck code names no format; scoring it as Standard");
        }
        p.say(format!(
            "{}, {} cards [{}] — {} games against each of {} field decks",
            class_name(loaded.resolved.class),
            loaded.resolved.total(),
            loaded.format_name,
            games,
            loaded.playable_field()
        ));
        let rates = gauntlet::evaluate(
            loaded.contender(),
            &loaded.field,
            games,
            app.threads,
            17,
        );
        app.count_games((games * rates.per_deck.len()) as u64);
        let Some(avg) = rates.average() else {
            return Err(format!(
                "not one deck in the {} gauntlet could be fielded, so there is nothing to score against",
                loaded.format_name
            ));
        };
        if !rates.skipped.is_empty() {
            p.say(format!(
                "{} deck(s) not fielded: {}",
                rates.skipped.len(),
                crate::names::list(&rates.skipped)
            ));
        }
        p.say(format!("done: {:.1}%", avg * 100.0));
        Ok(to_string(|o| {
            o.obj(|o| {
                o.str_field("cls", class_name(loaded.resolved.class));
                o.str_field("format", loaded.format_name);
                o.bool_field("format_assumed", loaded.assumed_format);
                o.field("avg", |v| v.round(avg, 4));
                o.int_field("games", games as i64);
                o.field("rates", |v| write_rates(v, &rates));
                write_field_note(o, &loaded.field);
            })
        }))
    });
    started(&id)
}

/// `POST /api/optimize` — measured single-card swaps.
pub fn optimize_deck(app: &Arc<App>, req: &Request) -> Response {
    let payload = body(req);
    let code = field_str(&payload, "code").to_string();
    let app = Arc::clone(app);
    let id = app.clone().jobs.start(move |p| {
        let loaded = load(&app, &code)?;
        let budget = Budget {
            threads: app.threads,
            ..Budget::default()
        };
        let report = optimize::optimize(
            &loaded.resolved.ids,
            loaded.resolved.class,
            loaded.format,
            loaded.style(),
            &loaded.field,
            budget,
            |line| p.say(line),
        );
        app.count_games(report.games);
        let pasted = deck_key(&code);
        let decoded = deckstring::decode(&pasted).ok();
        let hero = decoded
            .as_ref()
            .and_then(|d| d.heroes.first().copied())
            .or_else(|| hero_dbf(loaded.resolved.class));
        let format_byte = decoded
            .as_ref()
            .map(|d| d.format)
            .unwrap_or(if loaded.format.has(Formats::STANDARD) {
                2
            } else {
                1
            });
        let emit_code = |ids: &[CardId]| hero.map(|h| deckstring::encode_ids(h, ids, format_byte));
        let improved = emit_code(&report.deck);
        let kept_codes: Vec<Option<String>> = report
            .kept
            .iter()
            .map(|s| emit_code(&apply_swap(&loaded.resolved.ids, s.out, s.inn)))
            .collect();
        let near_codes: Vec<Option<String>> = report
            .near
            .iter()
            .map(|s| emit_code(&apply_swap(&loaded.resolved.ids, s.out, s.inn)))
            .collect();
        Ok(to_string(|o| {
            o.obj(|o| {
                o.field("base", |v| v.round(report.base, 4));
                o.field("new_avg", |v| v.round(report.best, 4));
                o.int_field("games", report.games as i64);
                o.int_field("confirm_games", budget.confirm_games as i64);
                o.field("code", |v| v.opt(improved.as_deref(), |v, c| v.str(c)));
                o.field("swaps", |v| {
                    v.arr(|a| {
                        for (s, code) in report.kept.iter().zip(&kept_codes) {
                            a.item(|v| {
                                write_swap(v, s, s.confirmed_delta.unwrap_or(0.0), code.as_deref())
                            });
                        }
                    })
                });
                o.field("near", |v| {
                    v.arr(|a| {
                        for (s, code) in report.near.iter().zip(&near_codes) {
                            a.item(|v| {
                                write_swap(
                                    v,
                                    s,
                                    s.confirmed_delta.unwrap_or(s.screen_delta),
                                    code.as_deref(),
                                )
                            });
                        }
                    })
                });
                write_field_note(o, &loaded.field);
            })
        }))
    });
    started(&id)
}

fn started(id: &str) -> Response {
    Response::json(200, to_string(|o| o.obj(|o| o.str_field("job", id))))
}

/// `POST /api/mull` — keep or throw, per card, measured.
pub fn mulligan(app: &App, req: &Request) -> Response {
    let payload = body(req);
    let code = field_str(&payload, "code");
    let loaded = match load(app, code) {
        Ok(l) => l,
        Err(e) => return Response::error(400, &e),
    };
    let Some(class) = gauntlet::class_by_name(field_str(&payload, "opp")) else {
        return Response::error(
            400,
            &format!("unknown class: {}", field_str(&payload, "opp")),
        );
    };
    let Some(opp) = loaded
        .field
        .iter()
        .find(|d| d.class == class && d.playable())
    else {
        return Response::error(
            400,
            &format!(
                "the {} gauntlet has no {} deck the simulator can field",
                loaded.format_name,
                class_name(class)
            ),
        );
    };

    let hand = strings(&payload, "hand");
    let mut cards: Vec<CardId> = Vec::new();
    for name in &hand {
        match find_card(name) {
            Some(c) => cards.push(c),
            None => return Response::error(400, &format!("unknown card: {name}")),
        }
    }

    let telemetry = app.telemetry(
        &deck_key(code),
        &loaded.resolved.ids,
        loaded.resolved.class,
        loaded.style(),
        &loaded.field,
    );
    let Some((_, matchup)) = telemetry.matchups.iter().find(|(n, _)| *n == opp.name) else {
        return Response::error(500, "the telemetry run holds no record for that opponent");
    };
    let base = matchup.base();

    Response::json(
        200,
        to_string(|o| {
            o.obj(|o| {
                o.str_field("opp_deck", &opp.name);
                o.str_field("opp_cls", class_name(opp.class));
                o.field("base", |v| v.round(base, 4));
                o.int_field("games", matchup.games as i64);
                o.int_field("min_n", MIN_APPEARANCES as i64);
                o.field("cards", |v| {
                    v.arr(|a| {
                        for card in &cards {
                            let stat = matchup.stat(*card).unwrap_or_default();
                            let delta = stat.opening_delta(base, MIN_APPEARANCES);
                            let cost = card.def().cost;
                            // Three answers. A card whose difference does not
                            // clear its own error bar falls back to the
                            // curve, the same way a card with too few games
                            // does.
                            let verdict = stat.opening_verdict(base, MIN_APPEARANCES);
                            let keep = match verdict {
                                Verdict::Keep => true,
                                Verdict::Toss => false,
                                Verdict::NoDifference | Verdict::TooFew => cost <= 3,
                            };
                            a.item(|v| {
                                v.obj(|o| {
                                    o.str_field("card", card.name());
                                    o.int_field("cost", cost as i64);
                                    o.bool_field("keep", keep);
                                    // Named so the front end can say "no
                                    // measurable difference" rather than
                                    // showing a fallback as a measurement.
                                    o.str_field(
                                        "verdict",
                                        match verdict {
                                            Verdict::Keep => "keep",
                                            Verdict::Toss => "toss",
                                            Verdict::NoDifference => "flat",
                                            Verdict::TooFew => "few",
                                        },
                                    );
                                    o.field("delta", |v| v.opt(delta, |v, d| v.round(d, 4)));
                                    o.field("margin", |v| {
                                        v.opt(stat.opening_margin(), |v, m| v.round(m, 4))
                                    });
                                    o.int_field("n", stat.open_n as i64);
                                    o.field("why", |v| reasons(v, *card, opp.style, delta));
                                })
                            });
                        }
                    })
                });
            })
        }),
    )
}

/// Why the answer is what it is, as keys the front end translates.
///
/// Prose does not belong in an API that serves a bilingual UI: a
/// server-composed sentence is a sentence in one language, and the screen
/// showing it may be in the other. So this returns the *reasons* — a short
/// key and, where it has one, its number — and the front end writes them out
/// in the language it is running in.
fn reasons(o: &mut Out, card: CardId, opp: Style, delta: Option<f64>) {
    let def = card.def();
    o.arr(|a| {
        let mut reason = |key: &str, n: Option<f64>| {
            a.item(|v| {
                v.obj(|o| {
                    o.str_field("k", key);
                    if let Some(n) = n {
                        o.field("n", |v| v.round(n, 4));
                    }
                })
            });
        };
        if def.cost >= 6 {
            reason("expensive", Some(def.cost as f64));
            if opp == Style::Aggro {
                reason("vs_aggro_cheap", None);
            }
        } else if def.cost <= 2 {
            reason("cheap", None);
        }
        if def.kind() == Kind::Spell && card.info().text.contains("damage") {
            reason(
                match opp {
                    Style::Aggro => "removal_vs_aggro",
                    Style::Control => "burn_vs_control",
                    Style::Midrange => "damage_both_ways",
                },
                None,
            );
        }
        if def.kind() == Kind::Minion && (2..=4).contains(&def.cost) && opp != Style::Aggro {
            reason("curve_body", None);
        }
        match delta {
            Some(d) => reason("measured", Some(d)),
            None => reason("no_data", None),
        }
    });
}

/// The card a typed name means: exact first, then case-insensitive, then a
/// unique prefix.
///
/// Every answer goes back through [`by_name`], which is the one place that
/// decides *which printing* a name means — several names in the corpus belong
/// to a collectible card and to a token or an enchantment as well, and a
/// lookup that returns whichever it walked into first would hand the engine a
/// card that cannot be played.
fn find_card(name: &str) -> Option<CardId> {
    let name = name.trim();
    if let Some(c) = by_name(name) {
        return Some(c);
    }
    let low = name.to_lowercase();
    let mut prefix: Option<&'static str> = None;
    let mut prefixes = 0;
    for c in tavernlab_core::cards::all() {
        if !c.def().collectible {
            continue;
        }
        let n = c.name().to_lowercase();
        if n == low {
            return by_name(c.name());
        }
        if n.starts_with(&low) && prefix != Some(c.name()) {
            prefix = Some(c.name());
            prefixes += 1;
        }
    }
    // An ambiguous prefix is not a guess: two cards start with "Fire" and
    // picking one of them would silently answer about the wrong card.
    (prefixes == 1)
        .then_some(prefix)
        .flatten()
        .and_then(by_name)
}

/// `POST /api/coach` — which matchups hurt, and which of your own cards the
/// simulations like and dislike.
pub fn coach(app: &App, req: &Request) -> Response {
    let payload = body(req);
    let code = field_str(&payload, "code");
    let loaded = match load(app, code) {
        Ok(l) => l,
        Err(e) => return Response::error(400, &e),
    };
    let t = app.telemetry(
        &deck_key(code),
        &loaded.resolved.ids,
        loaded.resolved.class,
        loaded.style(),
        &loaded.field,
    );
    if t.matchups.is_empty() {
        return Response::error(400, "not one deck in the gauntlet could be fielded");
    }

    let mut weak: Vec<(&str, f64)> = t
        .matchups
        .iter()
        .map(|(name, m)| (name.as_str(), m.base()))
        .collect();
    weak.sort_by(|a, b| a.1.total_cmp(&b.1));

    // One number per card: the mean, across matchups, of "won when it showed
    // up" minus "won in this matchup at all".
    let mut per_card: Vec<(CardId, f64, u32, u32)> = Vec::new();
    for (_, m) in &t.matchups {
        let base = m.base();
        for (card, stat) in &m.cards {
            let Some(d) = stat.drawn_delta(base, MIN_DRAWS) else {
                continue;
            };
            match per_card.iter_mut().find(|(c, _, _, _)| c == card) {
                Some((_, sum, n, games)) => {
                    *sum += d;
                    *n += 1;
                    *games += stat.drawn_n;
                }
                None => per_card.push((*card, d, 1, stat.drawn_n)),
            }
        }
    }
    let mut ranked: Vec<(CardId, f64, u32)> = per_card
        .into_iter()
        .map(|(c, sum, n, games)| (c, sum / n as f64, games))
        .collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));

    let card_list = |a: &mut tavernlab_json::ArrOut<'_>, rows: &[(CardId, f64, u32)]| {
        for (card, delta, games) in rows {
            a.item(|v| {
                v.arr(|a| {
                    a.str_item(card.name());
                    a.item(|v| v.round(*delta, 4));
                    a.item(|v| v.int(*games as i64));
                })
            });
        }
    };
    let keep: Vec<_> = ranked.iter().take(5).cloned().collect();
    let cut: Vec<_> = ranked.iter().rev().take(5).cloned().collect();

    Response::json(
        200,
        to_string(|o| {
            o.obj(|o| {
                o.str_field("cls", class_name(loaded.resolved.class));
                o.str_field("format", loaded.format_name);
                o.int_field("games", t.games_per_opponent as i64);
                o.int_field("min_n", MIN_DRAWS as i64);
                o.field("weak", |v| {
                    v.arr(|a| {
                        for (name, rate) in weak.iter().take(3) {
                            a.item(|v| {
                                v.arr(|a| {
                                    a.str_item(name);
                                    a.item(|v| v.round(*rate, 4));
                                })
                            });
                        }
                    })
                });
                o.field("keep", |v| v.arr(|a| card_list(a, &keep)));
                o.field("cut", |v| v.arr(|a| card_list(a, &cut)));
            })
        }),
    )
}

/// `POST /api/predict` — which gauntlet deck the cards you have seen fit.
pub fn predict(app: &App, req: &Request) -> Response {
    let payload = body(req);
    let (format_name, _) = format_of(&payload);
    let Some(class) = gauntlet::class_by_name(field_str(&payload, "opp")) else {
        return Response::error(
            400,
            &format!("unknown class: {}", field_str(&payload, "opp")),
        );
    };
    let field = app.gauntlet(format_name);
    let decks: Vec<&MetaDeck> = field.iter().filter(|d| d.class == class).collect();
    if decks.is_empty() {
        return Response::error(
            400,
            &format!(
                "the {format_name} gauntlet has no {} decks",
                class_name(class)
            ),
        );
    }

    let mut seen: Vec<CardId> = Vec::new();
    for name in strings(&payload, "seen") {
        match find_card(&name) {
            Some(c) => seen.push(c),
            None => return Response::error(400, &format!("unknown card: {name}")),
        }
    }
    // The same read `tavernsim watch` prints, so the two never drift apart.
    let reads = gauntlet::read_opponent(&field, class, &seen);

    Response::json(
        200,
        to_string(|o| {
            o.obj(|o| {
                o.str_field("format", format_name);
                o.field("decks", |v| {
                    v.arr(|a| {
                        for read in &reads {
                            a.item(|v| {
                                v.obj(|o| {
                                    o.str_field("deck", &read.deck);
                                    o.int_field("hits", read.hits as i64);
                                    o.int_field("seen", read.seen as i64);
                                    o.field("frac", |v| v.round(read.frac, 2));
                                    o.field("threats", |v| {
                                        v.arr(|a| {
                                            for card in &read.threats {
                                                let card = *card;
                                                a.item(|v| {
                                                    v.obj(|o| {
                                                        o.str_field("card", card.name());
                                                        o.int_field(
                                                            "cost",
                                                            card.def().cost as i64,
                                                        );
                                                        o.str_field(
                                                            "text",
                                                            &short(card.info().text),
                                                        );
                                                    })
                                                });
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

/// The most expensive cards in the deck that have not been seen yet.
fn short(text: &str) -> String {
    let clean = text.replace('\n', " ");
    if clean.chars().count() <= 90 {
        return clean;
    }
    clean.chars().take(89).collect::<String>() + "…"
}

/// `POST /api/meta` — the field itself, in full.
pub fn meta(app: &App, req: &Request) -> Response {
    let payload = body(req);
    let (format_name, format) = format_of(&payload);
    let field = app.gauntlet(format_name);
    Response::json(
        200,
        to_string(|o| {
            o.obj(|o| {
                o.str_field("format", format_name);
                o.field("decks", |v| {
                    v.arr(|a| {
                        for deck in field.iter() {
                            a.item(|v| write_meta_deck(v, deck, format));
                        }
                    })
                });
            })
        }),
    )
}

fn write_meta_deck(o: &mut Out, deck: &MetaDeck, format: Formats) {
    let code = export(deck, format);
    o.obj(|o| {
        o.str_field("name", &deck.name);
        o.str_field("cls", class_name(deck.class));
        o.str_field("archetype", gauntlet::style_name(deck.style));
        o.bool_field("playable", deck.playable());
        o.field("why", |v| {
            v.opt(deck.problem(), |v, p| {
                v.str(match p {
                    Unfieldable::Size => "size",
                    Unfieldable::Cards => "cards",
                })
            })
        });
        o.int_field("listed", deck.listed as i64);
        o.int_field("total", deck.total() as i64);
        o.field("cards", |v| {
            v.arr(|a| {
                for (name, n) in &deck.cards {
                    a.item(|v| {
                        v.arr(|a| {
                            a.str_item(name);
                            a.item(|v| v.int(*n as i64));
                        })
                    });
                }
            })
        });
        // Everything the UI needs to draw the list: the string id for the
        // art tile, the cost for the curve, and whether the engine can
        // actually play the card.
        o.field("cardlist", |v| {
            v.arr(|a| {
                let mut rows: Vec<(&str, u32)> =
                    deck.cards.iter().map(|(n, c)| (n.as_str(), *c)).collect();
                rows.sort_by_key(|(n, _)| {
                    by_name(n).map(|c| (c.def().cost, *n)).unwrap_or((99, *n))
                });
                for (name, n) in rows {
                    let card = by_name(name);
                    a.item(|v| {
                        v.obj(|o| {
                            o.str_field("id", card.map(|c| c.info().id).unwrap_or(name));
                            o.str_field("card", name);
                            o.int_field("n", n as i64);
                            o.int_field("cost", card.map(|c| c.def().cost).unwrap_or(0) as i64);
                            o.str_field(
                                "type",
                                card.map(|c| kind_name(c.def().kind())).unwrap_or("UNKNOWN"),
                            );
                            o.bool_field("implemented", card.is_some_and(is_implemented));
                        })
                    });
                }
            })
        });
        o.field("missing", |v| {
            v.arr(|a| {
                for (name, n) in &deck.missing {
                    a.item(|v| {
                        v.arr(|a| {
                            a.str_item(name);
                            a.item(|v| v.int(*n as i64));
                        })
                    });
                }
            })
        });
        o.field("deckstring", |v| v.opt(code.as_ref(), |v, c| v.str(&c.0)));
        o.field("deckstring_complete", |v| {
            v.bool(code.as_ref().is_some_and(|c| c.1))
        });
    });
}

fn kind_name(k: Kind) -> &'static str {
    match k {
        Kind::Minion => "MINION",
        Kind::Spell => "SPELL",
        Kind::Weapon => "WEAPON",
        Kind::Location => "LOCATION",
        Kind::Hero => "HERO",
        Kind::HeroPower => "HERO_POWER",
    }
}

/// A deck code for a gauntlet deck, and whether it is the whole deck.
///
/// It is built from the file's own list rather than from what the engine
/// fielded: a sideboard folded in as ten copies is right for the simulator
/// and would be an illegal list in game, so such a deck is exported as
/// incomplete and the UI says so.
fn export(deck: &MetaDeck, format: Formats) -> Option<(String, bool)> {
    let hero = hero_dbf(deck.class)?;
    let mut cards: Vec<(u32, u32)> = Vec::new();
    let mut complete = true;
    for (name, n) in &deck.cards {
        match by_name(name) {
            // Ten copies of one card is the Beatrix sideboard, which no
            // legal deck code can carry.
            Some(c) if *n <= 2 => cards.push((c.info().dbf, *n)),
            _ => complete = false,
        }
    }
    if cards.is_empty() {
        return None;
    }
    let total: u32 = cards.iter().map(|(_, n)| n).sum();
    // The format of the gauntlet the deck came from, not a guess from its
    // cards: every Standard card is Wild-legal too, so reading it off the
    // list would tag most of the Wild field as Standard and the code would
    // come back illegal the moment it was pasted in.
    let format_byte = if format.has(Formats::STANDARD) { 2 } else { 1 };
    Some((
        deckstring::encode(hero, &cards, format_byte),
        complete && total == 30,
    ))
}

fn write_swap(o: &mut Out, s: &optimize::Swap, delta: f64, code: Option<&str>) {
    o.obj(|o| {
        o.str_field("out", s.out);
        o.str_field("inn", s.inn);
        o.field("delta", |v| v.round(delta, 4));
        o.field("code", |v| v.opt(code, |v, c| v.str(c)));
    })
}

/// The submitted list with one named card swapped for another.
fn apply_swap(deck: &[CardId], out: &str, inn: &str) -> Vec<CardId> {
    let mut next = deck.to_vec();
    let Some(pos) = next.iter().position(|c| c.name() == out) else {
        return next;
    };
    if let Some(id) = by_name(inn) {
        next[pos] = id;
    }
    next
}

/// The classic hero portrait for a class, which is what a deck code names.
fn hero_dbf(class: Class) -> Option<u32> {
    let id = match class {
        Class::Warrior => "HERO_01",
        Class::Shaman => "HERO_02",
        Class::Rogue => "HERO_03",
        Class::Paladin => "HERO_04",
        Class::Hunter => "HERO_05",
        Class::Druid => "HERO_06",
        Class::Warlock => "HERO_07",
        Class::Mage => "HERO_08",
        Class::Priest => "HERO_09",
        Class::DemonHunter => "HERO_10",
        Class::DeathKnight => "HERO_11",
        _ => return None,
    };
    tavernlab_core::cards::by_id(id).map(|c| c.info().dbf)
}

/// Which policy plays a tier table, by the name the API uses for it.
///
/// The tier list is the one screen that ranks decks against each other, and
/// it is measurably a claim about the policy as well as about the decks:
/// three of twelve decks change tier between these two, and Quest Hunter
/// crosses four bands. See the README.
///
/// An unknown name is greedy rather than an error -- the table it produces
/// is the one this endpoint always produced, so an old client keeps working.
fn tiers_policy(name: &str) -> (&'static str, Policy) {
    match name {
        "search" => (
            "search",
            Policy::Plan {
                budget: 4000,
                depth: 4,
                samples: 1,
                iterative: true,
                weights: tavernlab_core::planner::Weights::default(),
            },
        ),
        _ => ("greedy", Policy::Greedy),
    }
}

/// `GET /api/tiers?format=&policy=` — the cached table, or `null`. Never a
/// computation on a GET: the matrix is quadratic.
pub fn tiers_read(app: &App, req: &Request) -> Response {
    let format_name = req.param("format").unwrap_or("standard");
    let Some((format_name, _)) = format_by_name(format_name) else {
        return Response::error(400, "unknown format");
    };
    let (policy_name, _) = tiers_policy(req.param("policy").unwrap_or("greedy"));
    match std::fs::read_to_string(app.tiers_path(format_name, policy_name)) {
        Ok(body) if Json::parse(&body).is_ok() => Response::json(200, body),
        _ => Response::json(
            200,
            to_string(|o| {
                o.obj(|o| {
                    o.str_field("format", format_name);
                    o.str_field("policy", policy_name);
                    o.field("decks", |v| v.null());
                })
            }),
        ),
    }
}

/// `POST /api/tiers` — play the field against itself.
pub fn tiers_start(app: &Arc<App>, req: &Request) -> Response {
    let payload = body(req);
    let (format_name, _) = format_of(&payload);
    let games = clamped(&payload, "games", 200, TIERS_MIN, TIERS_MAX);
    let (policy_name, policy) = tiers_policy(field_str(&payload, "policy"));
    let app = Arc::clone(app);
    let id = app.clone().jobs.start(move |p| {
        let field = app.gauntlet(format_name);
        if field.is_empty() {
            return Err(format!("no gauntlet for {format_name}"));
        }
        if policy_name == "search" {
            // Two hundred times the work of the greedy table, and the page
            // that started it has a progress log to read while it runs.
            p.say("рахую пошуком — це надовго, хвилини замість секунд".to_string());
        }
        let table = tiers::build_with(&field, [policy; 2], games, app.threads, |line| p.say(line));
        if table.rows.is_empty() {
            return Err("not one deck in the gauntlet could be fielded".into());
        }
        let played = table.rows.len();
        app.count_games((played.saturating_sub(1) * played * games) as u64);
        // Unix seconds, so a cached table can say how old it is. The
        // engine itself never reads a clock — a simulation that depended
        // on one could not be reproduced — but a cache entry has to.
        let computed_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let body = to_string(|o| {
            o.obj(|o| {
                o.str_field("format", format_name);
                // Stamped into the table itself, not just the file name: a
                // ranking that does not say what played it can be read as
                // policy-free, and this one is not.
                o.str_field("policy", policy_name);
                o.int_field("computed_at", computed_at);
                o.int_field("games_per_pair", table.games_per_pair as i64);
                o.field("margin", |v| v.round(table.margin, 4));
                o.field("tiers", |v| {
                    v.arr(|a| {
                        for (name, floor) in tiers::BANDS {
                            a.item(|v| {
                                v.obj(|o| {
                                    o.str_field("tier", name);
                                    o.field("floor", |v| v.num(floor));
                                })
                            });
                        }
                    })
                });
                o.field("skipped", |v| {
                    v.arr(|a| {
                        for name in &table.skipped {
                            a.str_item(name);
                        }
                    })
                });
                o.field("decks", |v| {
                    v.arr(|a| {
                        for row in &table.rows {
                            a.item(|v| {
                                v.obj(|o| {
                                    o.str_field("name", &row.name);
                                    o.str_field("cls", class_name(row.class));
                                    o.str_field("archetype", gauntlet::style_name(row.style));
                                    o.str_field("tier", row.tier);
                                    o.field("winrate", |v| v.round(row.winrate, 4));
                                    o.field("vs", |v| {
                                        v.obj(|o| {
                                            for (name, rate) in &row.vs {
                                                o.field(name, |v| v.round(*rate, 4));
                                            }
                                        })
                                    });
                                })
                            });
                        }
                    })
                });
            })
        });
        // Cached so nobody has to re-run a quadratic matrix to look at a
        // table they already computed.
        let path = app.tiers_path(format_name, policy_name);
        if let Err(e) = std::fs::write(&path, &body) {
            p.say(format!("could not save {}: {e}", path.display()));
        }
        Ok(body)
    });
    started(&id)
}

/// `POST /api/cardnames` — names for the autocomplete boxes.
pub fn cardnames(req: &Request) -> Response {
    let payload = body(req);
    let want_all = payload.get("all").and_then(Json::as_bool).unwrap_or(true);
    let class = gauntlet::class_by_name(field_str(&payload, "cls"));
    let mut names: Vec<&'static str> = tavernlab_core::cards::all()
        .filter(|c| {
            let d = c.def();
            // Deliberately not restricted to what the engine implements
            // when `all` is set: a player types the name of the card they
            // are actually holding, and a box that cannot spell it is worse
            // than one that answers "not simulated".
            d.collectible
                && d.deckable()
                && (want_all || is_implemented(*c))
                && class.is_none_or(|k| d.playable_by(k))
        })
        .map(|c| c.name())
        .collect();
    names.sort_unstable();
    names.dedup();
    Response::json(
        200,
        to_string(|o| {
            o.obj(|o| {
                o.bool_field("all", want_all);
                o.field("names", |v| {
                    v.arr(|a| {
                        for n in &names {
                            a.str_item(n);
                        }
                    })
                });
            })
        }),
    )
}

/// `GET /api/settings` and `POST /api/settings`.
pub fn settings(app: &App, req: &Request) -> Response {
    if req.method == "POST" {
        let payload = body(req);
        let mut patch: Vec<(String, String)> = Vec::new();
        for (k, v) in payload.as_object().unwrap_or(&[]) {
            let value = match v {
                Json::Str(s) => s.clone(),
                Json::Null => String::new(),
                other => other.as_i64().map(|n| n.to_string()).unwrap_or_default(),
            };
            // A pasted deck block is stored as the bare code, with its
            // `### Name` title lifted into `deck_name`: the same deck
            // pasted twice, once with comments, must be one deck.
            if k == "deckstring" && !value.trim().is_empty() {
                if let Some(name) = deckstring::deck_name(&value) {
                    patch.push(("deck_name".into(), name.to_string()));
                }
                let bare = deckstring::extract(&value)
                    .map(str::to_string)
                    .unwrap_or_else(|_| value.trim().to_string());
                patch.push((k.clone(), bare));
                continue;
            }
            patch.push((k.clone(), value));
        }
        if let Err(e) = app.set_settings(&patch) {
            return Response::error(500, &e);
        }
    }
    Response::json(
        200,
        to_string(|o| {
            o.obj(|o| {
                o.field("settings", |v| {
                    v.obj(|o| {
                        for (k, val) in app.settings() {
                            o.str_field(&k, &val);
                        }
                    })
                });
            })
        }),
    )
}

/// `GET /api/metrics` — local counters and what this build can actually do.
/// Nothing is sent anywhere; this is the whole of the observability story.
/// `GET /api/history` — the games you have played, and what they add up to.
///
/// Read straight off the SQLite file every time rather than cached: the
/// watcher is a separate process writing it while this one serves, and a
/// cache here would show a stale record of the game that just ended.
pub fn history(_app: &App) -> Response {
    let path = crate::history::default_path();
    let games = match crate::history::read(&path) {
        Ok(g) => g,
        Err(e) => {
            return Response::json(
                200,
                to_string(|o| {
                    o.obj(|o| {
                        o.str_field("error", &e);
                        o.str_field("path", &path.display().to_string());
                    })
                }),
            );
        }
    };
    let summary = crate::history::summarise(&games);
    fn tally(o: &mut tavernlab_json::ObjOut<'_>, name: &str, rows: &[crate::history::Tally]) {
        o.field(name, |v| {
            v.arr(|a| {
                for t in rows {
                    a.item(|v| {
                        v.obj(|o| {
                            o.str_field("key", &t.key);
                            o.int_field("games", t.games as i64);
                            o.int_field("wins", t.wins as i64);
                            // The rate is absent below the sample floor rather
                            // than printed small: four wins out of four is not
                            // a hundred per cent, and the UI must not be able
                            // to render it as one.
                            o.field("rate", |v| v.opt(t.rate(), |v, r| v.num(r)));
                        })
                    });
                }
            })
        });
    }

    Response::json(
        200,
        to_string(|o| {
            o.obj(|o| {
                o.str_field("path", &path.display().to_string());
                o.int_field("games", summary.games as i64);
                o.int_field("resolved", summary.resolved as i64);
                o.int_field("wins", summary.wins as i64);
                tally(o, "by_opponent", &summary.by_opponent);
                tally(o, "by_my_class", &summary.by_my_class);
                tally(o, "by_opponent_deck", &summary.by_opponent_deck);
                o.field("rows", |v| {
                    v.arr(|a| {
                        // Newest first: the game you want to look at is the
                        // one you just played.
                        for g in games.iter().rev() {
                            a.item(|v| {
                                v.obj(|o| {
                                    o.int_field("played_at", g.played_at);
                                    o.str_field("my_class", &g.my_class);
                                    o.str_field("opponent_class", &g.opponent_class);
                                    o.field("won", |v| v.opt(g.won, |v, w| v.bool(w)));
                                    o.int_field("turns", g.turns);
                                    o.field("coin", |v| v.opt(g.coin, |v, c| v.bool(c)));
                                    o.str_field("opponent_deck", &g.opponent_deck);
                                    o.int_field("opponent_hits", g.opponent_hits);
                                    o.int_field("opponent_seen", g.opponent_seen);
                                    o.field("opening", |v| {
                                        v.arr(|a| {
                                            for c in &g.opening {
                                                a.str_item(c);
                                            }
                                        })
                                    });
                                    o.field("opponent_cards", |v| {
                                        v.arr(|a| {
                                            for c in &g.opponent_cards {
                                                a.str_item(c);
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

pub fn metrics(app: &App) -> Response {
    let counts = |fmt: Formats| -> (usize, usize) {
        let deckable: Vec<CardId> = tavernlab_core::cards::all()
            .filter(|c| {
                let d = c.def();
                d.collectible && d.deckable() && d.formats.has(fmt)
            })
            .collect();
        let done = deckable.iter().filter(|c| is_implemented(**c)).count();
        (deckable.len(), done)
    };
    let (std_total, std_done) = counts(Formats::STANDARD);
    let (wild_total, wild_done) = counts(Formats::WILD);

    Response::json(
        200,
        to_string(|o| {
            o.obj(|o| {
                o.int_field("cards", tavernlab_core::cards::DEFS.len() as i64);
                o.int_field("standard_deckable", std_total as i64);
                o.int_field("standard_implemented", std_done as i64);
                o.int_field("wild_deckable", wild_total as i64);
                o.int_field("wild_implemented", wild_done as i64);
                for format_name in ["standard", "wild"] {
                    let field = app.gauntlet(format_name);
                    o.field(&format!("gauntlet_{format_name}"), |v| {
                        v.obj(|o| {
                            o.int_field("decks", field.len() as i64);
                            o.int_field(
                                "playable",
                                field.iter().filter(|d| d.playable()).count() as i64,
                            );
                        })
                    });
                }
                o.int_field("games_simulated", app.games_simulated() as i64);
                o.int_field("jobs_started", app.jobs.started_total() as i64);
                o.int_field("threads", app.threads as i64);
                o.int_field("uptime_s", app.started.elapsed().as_secs() as i64);
                o.str_field("data_home", &app.home.display().to_string());
                o.str_field("root", &app.root.display().to_string());
            })
        }),
    )
}

/// `GET /api/job/{id}` — how a background run is doing.
pub fn job(app: &App, id: &str) -> Response {
    let Some(body) = app.jobs.with(id, |job| {
        to_string(|o| {
            o.obj(|o| {
                o.str_field("status", job.status.as_str());
                o.field("progress", |v| {
                    v.arr(|a| {
                        for line in &job.progress {
                            a.str_item(line);
                        }
                    })
                });
                o.field("error", |v| v.opt(job.error.as_deref(), |v, e| v.str(e)));
                o.field("result", |v| match &job.result {
                    Some(r) => v.raw(r),
                    None => v.null(),
                });
                o.field("elapsed_ms", |v| {
                    v.int(
                        job.finished
                            .unwrap_or_else(|| job.started.elapsed())
                            .as_millis() as i64,
                    )
                });
            })
        })
    }) else {
        return Response::error(404, "no such job");
    };
    Response::json(200, body)
}

// --------------------------------------------------------------------- live

/// What the log watcher can currently say.
///
/// A poll rather than a stream: the page asks once a second and gets the
/// position as it stands, which is what a reader glancing at it beside the
/// client wants. Nothing here is cached — building the answer is reading a
/// struct the watcher's thread already filled in.
pub fn live_read(app: &Arc<App>) -> Response {
    let snap = app.live.snapshot();
    let dir = super::live::logs_dir(app).map(|p| p.display().to_string());
    Response::json(
        200,
        to_string(|o| {
            o.obj(|o| {
                o.bool_field("running", snap.running);
                o.str_field("logs_dir", dir.as_deref().unwrap_or(""));
                o.str_field("watching", snap.watching.as_deref().unwrap_or(""));
                match &snap.note {
                    Some(n) => o.field("note", |v| write_line(v, n)),
                    None => o.field("note", |v| v.null()),
                }
                o.int_field("recorded", snap.recorded as i64);
                // Keys and their values, not sentences: the page writes the
                // words out in whatever language it is running in. See
                // `watch::advice`.
                match &snap.advice {
                    Some(a) => {
                        o.field("title", |v| {
                            v.arr(|arr| {
                                for part in &a.title {
                                    arr.item(|v| write_line(v, part));
                                }
                            })
                        });
                        o.field("sections", |v| {
                            v.arr(|arr| {
                                for s in a.sections.iter().filter(|s| !s.lines.is_empty()) {
                                    arr.item(|o| {
                                        o.obj(|o| {
                                            o.str_field("heading", s.key);
                                            o.field("lines", |v| {
                                                v.arr(|arr| {
                                                    for line in &s.lines {
                                                        arr.item(|v| write_line(v, line));
                                                    }
                                                })
                                            });
                                        });
                                    });
                                }
                            })
                        });
                    }
                    None => {
                        o.field("title", |v| v.arr(|_| {}));
                        o.field("sections", |v| v.arr(|_| {}));
                    }
                }
            })
        }),
    )
}

/// Start or stop the watcher. Anything else is refused rather than guessed
/// at: there are two states and a request naming neither means something the
/// page did not intend.
pub fn live_write(app: &Arc<App>, req: &Request) -> Response {
    let payload = body(req);
    match field_str(&payload, "action") {
        "start" => match super::live::start(app, "standard") {
            Ok(_) => live_read(app),
            Err(e) => Response::error(400, &e),
        },
        "stop" => {
            app.live.stop();
            live_read(app)
        }
        other => Response::error(400, &format!("невідома дія: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_typed_card_name_resolves_exactly_case_insensitively_and_by_prefix() {
        assert_eq!(find_card("Fireball"), by_name("Fireball"));
        // Several names in the corpus are shared by a collectible card and a
        // token; the collectible one is what a player means.
        assert_eq!(find_card("  fireball "), by_name("Fireball"));
        assert_eq!(find_card("not a card at all"), None);
        // A prefix that matches exactly one card resolves; one that matches
        // several is refused rather than guessed.
        assert_eq!(find_card("Goldshire Foot"), by_name("Goldshire Footman"));
        assert_eq!(find_card("The "), None);
    }

    #[test]
    fn card_text_is_shortened_without_cutting_a_character_in_half() {
        let long = "ї".repeat(200);
        let s = short(&long);
        assert_eq!(s.chars().count(), 90);
        assert!(s.ends_with('…'));
        assert_eq!(short("short one"), "short one");
        assert_eq!(short("two\nlines"), "two lines");
    }

    #[test]
    fn every_playable_class_has_a_hero_portrait_for_export() {
        for c in tavernlab_core::cards::PLAYABLE_CLASSES {
            assert!(hero_dbf(c).is_some(), "{c:?} has no hero portrait");
        }
        assert_eq!(hero_dbf(Class::Neutral), None);
    }
}
