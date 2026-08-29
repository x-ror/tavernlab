//! Everything the request handlers share.
//!
//! Three kinds of state, and they behave differently on purpose:
//!
//! * **settings** are the user's and are written to disk, so they survive a
//!   restart;
//! * **gauntlets** are read from `data/` once and then held, because parsing
//!   the same twelve deck lists on every request is work with no answer
//!   attached to it;
//! * **telemetry** is a pure function of a deck and is cached only as an
//!   optimisation. Nothing depends on it being there — the engine plays
//!   tens of thousands of games a second, so a cold cache costs a fraction
//!   of a second rather than the "analyse your deck first" gate the Python
//!   app had to put in front of the mulligan screen.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tavernlab_core::agent::Style;
use tavernlab_core::batch::{Contender, seeds};
use tavernlab_core::cards::{CardId, Formats};
use tavernlab_core::gauntlet::{MetaDeck, class_by_name, style_by_name};
use tavernlab_core::telemetry::{self, Matchup};
use tavernlab_json::Json;

use super::jobs::Jobs;

/// Appearances below which a card's mulligan delta is noise, not a number.
pub const MULLIGAN_MIN_N: u32 = 30;

/// Settings the app actually has. A key outside this list is refused rather
/// than stored: an unknown setting that silently persists is a setting
/// nobody can find again.
pub const SETTING_KEYS: [&str; 3] = ["deckstring", "deck_name", "language"];

/// Games per opponent behind the mulligan and coach screens.
///
/// Chosen for the width of the per-card confidence interval rather than for
/// a time budget: at 1500 games a matchup, a card that appears in half of
/// them carries roughly ±3.5 win-rate points, which is the resolution those
/// screens claim.
pub const TELEMETRY_GAMES: usize = 1500;

/// Telemetry sets kept in memory. One per deck under study is the normal
/// case; a few more covers comparing decks without re-running each time.
const TELEMETRY_CACHE: usize = 4;

pub struct App {
    /// Where `data/`, `locales/` and `web/dist` live.
    pub root: PathBuf,
    /// Where the user's settings and caches live.
    pub home: PathBuf,
    /// Threads a simulation batch may use.
    pub threads: usize,
    pub jobs: Jobs,
    pub started: Instant,
    /// Games simulated since start, for `/api/metrics`.
    games: AtomicU64,
    settings: Mutex<BTreeMap<String, String>>,
    gauntlets: Mutex<Vec<(String, Arc<Vec<MetaDeck>>)>>,
    telemetry: Mutex<Vec<(String, Arc<DeckTelemetry>)>>,
}

/// One deck's instrumented record against the field.
pub struct DeckTelemetry {
    pub games_per_opponent: usize,
    /// `(opponent deck name, record)`, playable opponents only.
    pub matchups: Vec<(String, Matchup)>,
}

impl App {
    pub fn new(root: PathBuf, home: PathBuf, threads: usize) -> App {
        let settings = read_settings(&home);
        App {
            root,
            home,
            threads,
            jobs: Jobs::default(),
            started: Instant::now(),
            games: AtomicU64::new(0),
            settings: Mutex::new(settings),
            gauntlets: Mutex::new(Vec::new()),
            telemetry: Mutex::new(Vec::new()),
        }
    }

    // ------------------------------------------------------------ settings

    pub fn settings(&self) -> BTreeMap<String, String> {
        let stored = self.settings.lock().expect("settings lock").clone();
        let mut out: BTreeMap<String, String> = SETTING_KEYS
            .iter()
            .map(|k| ((*k).to_string(), String::new()))
            .collect();
        out.extend(stored);
        out
    }

    /// Apply a patch and write it out. Unknown keys are ignored.
    pub fn set_settings(&self, patch: &[(String, String)]) -> Result<(), String> {
        {
            let mut s = self.settings.lock().expect("settings lock");
            for (k, v) in patch {
                if SETTING_KEYS.contains(&k.as_str()) {
                    s.insert(k.clone(), v.clone());
                }
            }
        }
        write_settings(&self.home, &self.settings())
    }

    // ------------------------------------------------------------ gauntlet

    /// The field for a format, read from `data/` on first use.
    ///
    /// An unknown format name, or a file that is not there, is an empty
    /// field — the caller says so rather than dividing by zero.
    pub fn gauntlet(&self, format: &str) -> Arc<Vec<MetaDeck>> {
        if let Some(hit) = self
            .gauntlets
            .lock()
            .expect("gauntlet lock")
            .iter()
            .find(|(f, _)| f == format)
        {
            return Arc::clone(&hit.1);
        }
        let decks = Arc::new(load_gauntlet(&self.gauntlet_path(format)));
        self.gauntlets
            .lock()
            .expect("gauntlet lock")
            .push((format.to_string(), Arc::clone(&decks)));
        decks
    }

    /// Keep-or-throw for an opening hand, against one class in the field.
    ///
    /// The same measurement the Mulligan tab prints, in one call so a caller
    /// outside the HTTP layer -- `tavernsim watch` -- reaches it without
    /// going through a request. Below `MULLIGAN_MIN_N` appearances there is
    /// no measurement, and the answer says so rather than dressing a cost
    /// heuristic up as one.
    pub fn mulligan_advice(
        &self,
        format: &str,
        code: &str,
        opp_class: tavernlab_core::cards::Class,
        hand: &[CardId],
    ) -> Result<Vec<String>, String> {
        let resolved =
            tavernlab_core::deckstring::resolve(code).map_err(|e| e.to_string())?;
        if !resolved.unimplemented.is_empty() {
            return Err(format!(
                "симулятор не грає цими картами: {}",
                resolved.unimplemented.join(", ")
            ));
        }
        let field = self.gauntlet(format);
        let Some(opp) = field
            .iter()
            .find(|d| d.class == opp_class && d.playable())
        else {
            return Err(format!(
                "у гаунтлеті {format} немає колоди класу {}, яку симулятор може виставити",
                tavernlab_core::gauntlet::class_name(opp_class)
            ));
        };
        let key = tavernlab_core::deckstring::extract(code)
            .unwrap_or(code.trim())
            .to_string();
        let style = self
            .settings()
            .get("style")
            .map(|s| style_by_name(s))
            .unwrap_or(Style::Midrange);
        let telemetry = self.telemetry(&key, &resolved.ids, resolved.class, style, &field);
        let Some((_, matchup)) = telemetry.matchups.iter().find(|(n, _)| *n == opp.name) else {
            return Err("телеметрія не має запису для цього суперника".into());
        };
        let base = matchup.base();
        let mut out = vec![format!(
            "проти «{}» — база {:.0}% на {} іграх",
            opp.name,
            base * 100.0,
            matchup.games
        )];
        for card in hand {
            let stat = matchup.stat(*card).unwrap_or_default();
            let delta = stat.opening_delta(base, MULLIGAN_MIN_N);
            let cost = card.def().cost;
            let keep = match delta {
                Some(d) => d > -0.01,
                None => cost <= 3,
            };
            let note = match delta {
                Some(d) => format!("{:+.1} в.п. на {} іграх", d * 100.0, stat.open_n),
                None => "мало даних, лишається крива".to_string(),
            };
            out.push(format!(
                "{} ({}) {:24} {note}",
                if keep { "ЛИШИТИ" } else { "СКИНУТИ" },
                cost,
                card.name()
            ));
        }
        Ok(out)
    }

    pub fn gauntlet_path(&self, format: &str) -> PathBuf {
        self.root.join(format!("data/gauntlet_{format}.json"))
    }

    // ----------------------------------------------------------- telemetry

    /// Instrumented games for `deck` against the field, computed on demand.
    pub fn telemetry(
        &self,
        key: &str,
        deck: &[CardId],
        class: tavernlab_core::cards::Class,
        style: Style,
        field: &[MetaDeck],
    ) -> Arc<DeckTelemetry> {
        if let Some(hit) = self
            .telemetry
            .lock()
            .expect("telemetry lock")
            .iter()
            .find(|(k, _)| k == key)
        {
            return Arc::clone(&hit.1);
        }
        let s = seeds(31, TELEMETRY_GAMES);
        let me = Contender {
            class,
            cards: deck,
            style,
        };
        let mut matchups = Vec::new();
        for opp in field.iter().filter(|d| d.playable()) {
            let m = telemetry::instrumented_parallel(me, opp.contender(), &s, self.threads);
            self.count_games(m.games as u64);
            matchups.push((opp.name.clone(), m));
        }
        let out = Arc::new(DeckTelemetry {
            games_per_opponent: TELEMETRY_GAMES,
            matchups,
        });
        let mut cache = self.telemetry.lock().expect("telemetry lock");
        cache.push((key.to_string(), Arc::clone(&out)));
        while cache.len() > TELEMETRY_CACHE {
            cache.remove(0);
        }
        out
    }

    // ------------------------------------------------------------- metrics

    pub fn count_games(&self, n: u64) {
        self.games.fetch_add(n, Ordering::Relaxed);
    }

    pub fn games_simulated(&self) -> u64 {
        self.games.load(Ordering::Relaxed)
    }

    /// Where the cached tier table for a format lives.
    /// Where a computed tier table is cached.
    ///
    /// Keyed by the policy as well as the format. A table played by the
    /// greedy policy and one played by the search are different answers to
    /// the same question -- three of twelve decks change tier between them --
    /// so one must not overwrite the other and be read as the other. The
    /// greedy table keeps the original path, so a cache written before this
    /// existed still reads.
    pub fn tiers_path(&self, format: &str, policy: &str) -> PathBuf {
        if policy == "greedy" {
            return self.home.join(format!("tiers_{format}.json"));
        }
        self.home.join(format!("tiers_{format}_{policy}.json"))
    }
}

/// The formats that have a card pool and a gauntlet.
pub fn format_by_name(name: &str) -> Option<(&'static str, Formats)> {
    match name {
        "standard" => Some(("standard", Formats::STANDARD)),
        "wild" => Some(("wild", Formats::WILD)),
        _ => None,
    }
}

pub fn format_name(f: Formats) -> &'static str {
    if f.has(Formats::STANDARD) {
        "standard"
    } else {
        "wild"
    }
}

fn settings_path(home: &std::path::Path) -> PathBuf {
    home.join("settings.json")
}

fn read_settings(home: &std::path::Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Ok(src) = std::fs::read_to_string(settings_path(home)) else {
        return out;
    };
    let Ok(doc) = Json::parse(&src) else {
        // A corrupt settings file is not worth refusing to start over: the
        // defaults are all empty strings anyway.
        return out;
    };
    for (k, v) in doc.as_object().unwrap_or(&[]) {
        if SETTING_KEYS.contains(&k.as_str())
            && let Some(s) = v.as_str()
        {
            out.insert(k.clone(), s.to_string());
        }
    }
    out
}

fn write_settings(
    home: &std::path::Path,
    settings: &BTreeMap<String, String>,
) -> Result<(), String> {
    let body = tavernlab_json::to_string(|o| {
        o.obj(|o| {
            for (k, v) in settings {
                o.str_field(k, v);
            }
        })
    });
    let path = settings_path(home);
    // Written through a temporary file: a half-written settings.json is a
    // file that loses the deck the user pasted.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body).map_err(|e| format!("writing {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("replacing {}: {e}", path.display()))
}

/// Read a gauntlet file into decks. A malformed entry is skipped rather than
/// taking the file down with it, and a missing file is an empty field.
pub fn load_gauntlet(path: &std::path::Path) -> Vec<MetaDeck> {
    let Ok(src) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(doc) = Json::parse(&src) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (name, deck) in doc.as_object().unwrap_or(&[]) {
        let Some(class) = class_by_name(deck.str_or_empty("class")) else {
            continue;
        };
        let style = style_by_name(deck.str_or_empty("archetype"));
        let cards = pairs(deck.get("cards"));
        if cards.is_empty() {
            continue;
        }
        out.push(MetaDeck::new(
            name.clone(),
            class,
            style,
            &cards,
            &pairs(deck.get("sideboard")),
        ));
    }
    out
}

/// The `[name, count]` pairs of a deck's `cards` or `sideboard` array.
fn pairs(v: Option<&Json>) -> Vec<(String, u32)> {
    v.and_then(Json::as_array)
        .unwrap_or(&[])
        .iter()
        .filter_map(|entry| {
            let a = entry.as_array()?;
            let name = a.first()?.as_str()?.to_string();
            let count = a.get(1)?.as_i64()?;
            Some((name, count.max(0) as u32))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::paths;
    use super::*;
    use tavernlab_core::gauntlet::Unfieldable;

    fn temp_home(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tavernlab-test-{tag}-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    #[test]
    fn settings_round_trip_through_disk() {
        let home = temp_home("settings");
        let _ = std::fs::remove_file(settings_path(&home));
        let root = paths::repo_root().expect("workspace root");
        let app = App::new(root.clone(), home.clone(), 2);
        app.set_settings(&[
            ("deckstring".to_string(), "AAECAaoI".to_string()),
            ("language".to_string(), "uk".to_string()),
            ("not_a_setting".to_string(), "x".to_string()),
        ])
        .expect("settings should save");

        let again = App::new(root, home.clone(), 2);
        assert_eq!(again.settings()["deckstring"], "AAECAaoI");
        assert_eq!(again.settings()["language"], "uk");
        assert!(!again.settings().contains_key("not_a_setting"));
        // Every known key is present even when it has never been set.
        for k in SETTING_KEYS {
            assert!(again.settings().contains_key(k), "{k} missing");
        }
        let _ = std::fs::remove_file(settings_path(&home));
    }

    #[test]
    fn the_standard_gauntlet_loads_and_reports_what_it_cannot_field() {
        let root = paths::repo_root().expect("workspace root");
        let app = App::new(root, temp_home("gauntlet"), 2);
        let field = app.gauntlet("standard");
        assert_eq!(field.len(), 12, "the Standard gauntlet is twelve decks");
        assert!(
            field.iter().any(|d| d.playable()),
            "no Standard deck could be fielded at all"
        );
        // An unplayable deck has to name its reason. There are two, and they
        // are not the same problem: a card the engine cannot play is a gap in
        // the engine, while a list of fewer than thirty cards is an
        // incomplete entry in the gauntlet file. Asserting on `missing` alone
        // would treat the second as a silent failure -- which is what
        // happened to Thief Priest, a twenty-card entry that only stopped
        // reporting a missing card once its last unimplemented one was
        // implemented.
        for deck in field.iter().filter(|d| !d.playable()) {
            match deck.problem() {
                Some(Unfieldable::Cards) => assert!(
                    !deck.missing.is_empty(),
                    "{} is unplayable on cards without naming one",
                    deck.name
                ),
                Some(Unfieldable::Size) => assert_ne!(
                    deck.total(),
                    30,
                    "{} is called the wrong size at thirty cards",
                    deck.name
                ),
                None => unreachable!("an unplayable deck has a problem"),
            }
        }
        // The same call twice is the same allocation, not a second parse.
        assert!(Arc::ptr_eq(&field, &app.gauntlet("standard")));
    }

    #[test]
    fn the_standard_gauntlet_is_fully_playable() {
        // Every card in every Standard deck is implemented, and every list is
        // a legal size -- twenty for the one built around Azalina Soulsever,
        // thirty for the rest. This is the field the ratings are measured
        // against, so a deck dropping out of it silently would move every
        // published number.
        let root = paths::repo_root().expect("workspace root");
        let app = App::new(root, temp_home("standard-playable"), 2);
        for deck in app.gauntlet("standard").iter() {
            assert!(
                deck.playable(),
                "{} cannot be fielded: {:?}, {} cards",
                deck.name,
                deck.problem(),
                deck.total()
            );
        }
    }

    #[test]
    fn the_wild_gauntlet_is_fully_playable() {
        // It is generated from this engine's own implemented pool, so
        // anything less means the generator and the table have drifted
        // apart — which is exactly what happened to the Python-generated
        // one this replaced.
        let root = paths::repo_root().expect("workspace root");
        let app = App::new(root, temp_home("wild"), 2);
        let field = app.gauntlet("wild");
        assert!(!field.is_empty());
        for deck in field.iter() {
            assert!(
                deck.playable(),
                "{} cannot be fielded: {:?}",
                deck.name,
                deck.missing
            );
            assert_eq!(deck.total(), 30, "{} is not a 30-card deck", deck.name);
        }
    }

    #[test]
    fn an_unknown_format_is_an_empty_field_not_a_panic() {
        let root = paths::repo_root().expect("workspace root");
        let app = App::new(root, temp_home("unknown"), 2);
        assert!(app.gauntlet("twist").is_empty());
        assert_eq!(format_by_name("twist"), None);
    }
}
