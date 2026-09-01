//! `/api/memory` — a thin, read-only window onto `memreader --snapshot`.
//!
//! Deliberately **not** part of [`super::live`]: CLAUDE.md's hard rule #4
//! keeps `watch::tracker` a log-only reconstruction with no memory reads at
//! all, and this stays a second, separate source rather than a silent
//! fallback merged into it. A page can ask for both and reconcile them
//! itself; nothing here decides which one wins.
//!
//! `memreader` is a sibling binary of `tavernsim`, not a library this crate
//! links against — it needs its own process to call `process_vm_readv`
//! against Hearthstone.exe, and workspace builds already place both
//! binaries in the same `target/{debug,release}` directory, so it is found
//! next to `tavernsim`'s own executable rather than searched for on `PATH`.

use std::env;
use std::path::PathBuf;
use std::process::Command;

use tavernlab_json::Json;

use super::http::Response;

/// `memreader`'s own `cardId` is the game's internal card code
/// (`"HERO_09dbp"`, `"CS2_182"`) -- correct for cross-checking against
/// `Power.log`, useless for a person reading the Live tab. This endpoint is
/// the one place both the raw snapshot and the card corpus (`core`, already
/// linked into this binary) are in scope at once, so it is where the two
/// meet: every entity object gets a `name` field alongside its `cardId`,
/// resolved through the same lookup `/api/resolve` and friends already use.
pub fn snapshot() -> Response {
    let Some(bin) = sibling_binary() else {
        return Response::error(
            500,
            "не знайшов бінарник memreader поруч з tavernsim — зібрано лише один з двох (`cargo build -p memreader`)?",
        );
    };

    let output = Command::new(&bin).arg("--snapshot").output();
    let output = match output {
        Ok(o) => o,
        Err(e) => {
            return Response::error(500, &format!("не зміг запустити {}: {e}", bin.display()));
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Response::error(
            502,
            &format!(
                "memreader завершився з помилкою (найімовірніше — Hearthstone.exe не запущений): {}",
                stderr.lines().last().unwrap_or("(без діагностики)")
            ),
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let doc = match Json::parse(&stdout) {
        Ok(d) => d,
        Err(e) => {
            return Response::error(
                502,
                &format!("вивід memreader --snapshot не є валідним JSON: {}", e.msg),
            );
        }
    };

    let mut out = tavernlab_json::Out::new();
    with_names(&doc, &mut out);
    Response::json(200, out.finish())
}

/// A card's display name for the game's own internal code, or `None` for
/// codes with no corpus entry (this build's card table is Standard/Wild's
/// collectible+token set, not every internal id Hearthstone itself uses —
/// see `core/src/cards/table.rs`'s generation note).
fn card_name(card_id: &str) -> Option<&'static str> {
    tavernlab_core::cards::by_id(card_id).map(|c| c.info().name)
}

/// A *hero* card's class (`"DEATHKNIGHT"`, ...), for the one thing `name`
/// alone can't do: the web UI's hero portrait has real art cached by class
/// (`/api/art/hero/{class}`, drawn from the same set `Class::` selectors
/// already use) but not by the hero's own card id -- every hero skin is a
/// separate id (`HERO_11`, `HERO_11bp`, ...) the art cache was never built
/// against, so a portrait keyed on `cardId` 404s for nearly every hero.
/// `None` for anything that isn't a hero card, so a minion never grows a
/// spurious `class` field it has no use for.
fn hero_class(card_id: &str) -> Option<&'static str> {
    let card = tavernlab_core::cards::by_id(card_id)?;
    (card.def().kind() == tavernlab_core::cards::Kind::Hero)
        .then(|| tavernlab_core::gauntlet::class_name(card.def().class()))
}

/// Copy `v` through `out` unchanged, except: any object carrying a
/// `cardId` key gains a `name` field resolved from it. Recurses into every
/// array and object so it doesn't need to know the snapshot's exact shape
/// (top-level `entities`, or `sides[].hero`/`play`/`hand`/... -- all of
/// them are entity-shaped objects with a `cardId`).
fn with_names(v: &Json, out: &mut tavernlab_json::Out) {
    match v {
        Json::Null => out.null(),
        Json::Bool(b) => out.bool(*b),
        Json::Num(n) => out.num(*n),
        Json::Str(s) => out.str(s),
        Json::Arr(items) => out.arr(|a| {
            for item in items {
                a.item(|v| with_names(item, v));
            }
        }),
        Json::Obj(kvs) => out.obj(|o| {
            for (k, val) in kvs {
                o.field(k, |v| with_names(val, v));
            }
            if let Some(card_id) = v.get("cardId").and_then(Json::as_str) {
                o.field("name", |v| v.opt(card_name(card_id), |o, s| o.str(s)));
                if let Some(class) = hero_class(card_id) {
                    o.str_field("class", class);
                }
            }
        }),
    }
}

/// `memreader` next to the currently running `tavernsim` binary. `None`
/// when `current_exe()` itself fails (exotic sandboxing) or the file simply
/// isn't there — both reported the same way by the caller, since neither
/// is actionable beyond "build it".
fn sibling_binary() -> Option<PathBuf> {
    let exe = env::current_exe().ok()?;
    let dir = exe.parent()?;
    let candidate = dir.join(if cfg!(windows) { "memreader.exe" } else { "memreader" });
    candidate.exists().then_some(candidate)
}
