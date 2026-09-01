//! `tavernsim serve` — the local app.
//!
//! One process serves the built React front end out of `web/dist` and answers
//! the `/api` calls it makes, straight from the simulator in this workspace.
//! It replaces a Python HTTP server that fronted a second, slower engine; the
//! shape of the API is largely that server's, because the front end was
//! written against it and a rewrite of both at once would have made every
//! difference impossible to attribute.
//!
//! What the app can answer is now exactly what this engine can do, which is
//! narrower than what the Python one claimed and honest about the edges:
//! rating a deck against the gauntlet, looking for better cards, mulligan and
//! opponent reads from instrumented simulations, and the tier list the field
//! produces against itself.
//!
//! It also holds the log watcher ([`live`]), so the deck pasted on one tab is
//! the deck advised on during the game and recorded afterwards. That is the
//! whole reason it lives here rather than in a second command: three screens
//! about one deck should not need three processes and a flag apiece.
//!
//! Nothing here talks to the network. There is no card art fetcher and no
//! telemetry: art is served from a local cache if the user has filled one,
//! and `/api/metrics` is a set of counters that never leave the machine.

mod api;
mod draft;
pub mod http;
mod jobs;
mod live;
mod memory;
pub mod paths;
pub mod state;

use std::net::TcpListener;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use http::{Request, Response};
use state::App;

/// The default port. `TAVERNLAB_PORT` overrides it, so a second instance can
/// run beside the first instead of silently failing to bind and leaving the
/// first one to answer every request.
pub const DEFAULT_PORT: u16 = 8765;

/// File types the built front end is made of. Anything else is served as a
/// download rather than guessed at.
const WEB_TYPES: [(&str, &str); 11] = [
    ("html", "text/html; charset=utf-8"),
    ("js", "text/javascript; charset=utf-8"),
    ("css", "text/css; charset=utf-8"),
    ("json", "application/json; charset=utf-8"),
    ("svg", "image/svg+xml"),
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("woff2", "font/woff2"),
    ("woff", "font/woff"),
    ("ico", "image/x-icon"),
];

const NOT_BUILT: &str = r#"<!doctype html>
<html lang="uk"><head><meta charset="utf-8"><title>TavernLab</title>
<style>body{background:#14100e;color:#efe6db;font:16px/1.6 system-ui,sans-serif;
display:grid;place-items:center;height:100vh;margin:0}div{max-width:34rem;padding:2rem}
code{background:#000;padding:.15em .4em;border-radius:4px;color:#e8c65a}</style></head>
<body><div><h1>Інтерфейс не зібрано</h1>
<p>Зберіть його один раз:</p>
<p><code>cd web &amp;&amp; npm install &amp;&amp; npm run build</code></p>
<p>Потім перезавантажте цю сторінку. API вже працює.</p>
</div></body></html>
"#;

/// Start the server and block until the process is stopped.
pub fn run(port: u16, threads: usize, open_browser: bool) -> Result<(), String> {
    let Some(root) = paths::repo_root() else {
        return Err(concat!(
            "cannot find the data directory: expected `data/gauntlet_standard.json` and ",
            "`locales/en.json` under the working directory, beside the binary, or under ",
            "TAVERNLAB_ROOT"
        )
        .to_string());
    };
    let home = paths::data_home();
    let app = Arc::new(App::new(root.clone(), home.clone(), threads));

    let listener = TcpListener::bind(("127.0.0.1", port))
        .map_err(|e| format!("cannot listen on 127.0.0.1:{port}: {e}"))?;
    let url = format!("http://127.0.0.1:{port}/");
    println!("TavernLab — {url}");
    println!("  data      {}", root.display());
    println!("  your data {}", home.display());
    if !root.join("web/dist/index.html").is_file() {
        println!("  the web UI is not built: run `npm install && npm run build` in web/");
    }
    // The watcher, for the session that opens the lab and then plays. Off
    // unless it was switched on before, because something that reads your
    // game should start because you asked it to and go on starting because
    // you asked it to stay on.
    if app.settings().get("live_auto").map(String::as_str) == Some("on") {
        match live::start(&app) {
            Ok(dir) => println!("  стежу за {}", dir.display()),
            Err(e) => println!("  стеження не почалося: {e}"),
        }
    }
    if open_browser {
        open(&url);
    }

    http::serve(listener, move |req| route(&app, req)).map_err(|e| e.to_string())
}

fn route(app: &Arc<App>, req: &Request) -> Response {
    let path = req.path.as_str();
    let get = req.method == "GET";
    let post = req.method == "POST";

    match (req.method.as_str(), path) {
        ("GET", "/api/settings") | ("POST", "/api/settings") => return api::settings(app, req),
        ("GET", "/api/metrics") => return api::metrics(app),
        ("GET", "/api/history") => return api::history(app),
        ("GET", "/api/advice-history") => return api::advice_history(app),
        ("GET", "/api/tiers") => return api::tiers_read(app, req),
        ("POST", "/api/tiers") => return api::tiers_start(app, req),
        ("POST", "/api/resolve") => return api::resolve(app, req),
        ("POST", "/api/analyze") => return api::analyze(app, req),
        ("POST", "/api/optimize") => return api::optimize_deck(app, req),
        ("POST", "/api/mull") => return api::mulligan(app, req),
        ("POST", "/api/coach") => return api::coach(app, req),
        ("POST", "/api/predict") => return api::predict(app, req),
        ("POST", "/api/meta") => return api::meta(app, req),
        ("POST", "/api/cardnames") => return api::cardnames(req),
        ("POST", "/api/arena/draft") => return draft::draft(app, req),
        ("POST", "/api/arena/pick") => return draft::pick(app, req),
        ("GET", "/api/live") => return api::live_read(app),
        ("POST", "/api/live") => return api::live_write(app, req),
        ("GET", "/api/memory") => return memory::snapshot(),
        _ => {}
    }

    if let Some(id) = path.strip_prefix("/api/job/") {
        return if get {
            api::job(app, id)
        } else {
            Response::error(405, "GET only")
        };
    }
    if let Some(rest) = path.strip_prefix("/api/art/") {
        return if get {
            art(app, rest)
        } else {
            Response::error(405, "GET only")
        };
    }
    if let Some(lang) = path
        .strip_prefix("/locales/")
        .and_then(|l| l.strip_suffix(".json"))
    {
        return if get {
            locale(app, lang)
        } else {
            Response::error(405, "GET only")
        };
    }
    if path.starts_with("/api/") {
        return Response::error(404, "no such route");
    }
    if get {
        return web(app, path);
    }
    if post {
        return Response::error(404, "no such route");
    }
    Response::error(405, "unsupported method")
}

/// One translation file. The front end merges its own strings on top.
fn locale(app: &App, lang: &str) -> Response {
    // A language tag is a file name here, so it is checked rather than
    // trusted: two letters, optionally with a region.
    let ok = matches!(lang.len(), 2 | 5)
        && lang
            .bytes()
            .all(|b| b.is_ascii_alphabetic() || b == b'-' || b == b'_');
    if !ok {
        return Response::error(404, "no such locale");
    }
    match std::fs::read(app.root.join("locales").join(format!("{lang}.json"))) {
        Ok(body) => Response::text(200, "application/json; charset=utf-8", body),
        Err(_) => Response::error(404, "no such locale"),
    }
}

/// One prefetched illustration, or 404.
///
/// There is deliberately no network fallback: a lazy download would tell a
/// CDN which card the player is looking at, which is the whole thing this
/// app's posture exists to avoid. A missing file is a 404 and the UI draws
/// its own class crest instead.
fn art(app: &App, rest: &str) -> Response {
    let Some((kind, name)) = rest.split_once('/') else {
        return Response::error(404, "no such art");
    };
    if !matches!(kind, "hero" | "tile" | "art")
        || name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Response::error(404, "no such art");
    }
    for dir in [app.home.join("art_cache"), app.root.join("art_cache")] {
        for (ext, ctype) in [("jpg", "image/jpeg"), ("png", "image/png")] {
            let target = dir.join(kind).join(format!("{name}.{ext}"));
            if let Ok(body) = std::fs::read(&target) {
                return Response {
                    status: 200,
                    content_type: ctype,
                    body,
                    // Immutable by card id: a browser should never ask twice.
                    cache: Some("public, max-age=31536000, immutable"),
                };
            }
        }
    }
    Response::error(404, "no cached art: see `tavernsim art-urls`")
}

/// The built front end.
fn web(app: &App, path: &str) -> Response {
    let root = app.root.join("web/dist");
    // `/app` is where the Python server put the UI; keep it working so an
    // old bookmark does not 404.
    let rel = path
        .strip_prefix("/app")
        .unwrap_or(path)
        .trim_start_matches('/');
    let rel = if rel.is_empty() { "index.html" } else { rel };
    let Some(target) = safe_join(&root, rel) else {
        return Response::error(404, "not found");
    };
    match std::fs::read(&target) {
        Ok(body) => Response::text(200, content_type(&target), body),
        Err(_) if rel == "index.html" => {
            // A missing build is the whole product missing, so the browser
            // gets a page it can read rather than a JSON blob.
            Response::text(404, "text/html; charset=utf-8", NOT_BUILT)
        }
        Err(_) => Response::error(404, "not found"),
    }
}

/// `root/rel`, or `None` if `rel` climbs out of `root`.
///
/// This server binds to loopback, but a path traversal is still a path
/// traversal, and the check is on the *components* rather than on the string:
/// a `..` that has already been percent-decoded is not visible to a substring
/// test.
fn safe_join(root: &Path, rel: &str) -> Option<PathBuf> {
    let mut out = root.to_path_buf();
    for part in Path::new(rel).components() {
        match part {
            Component::Normal(p) => out.push(p),
            Component::CurDir => {}
            _ => return None,
        }
    }
    Some(out)
}

fn content_type(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    WEB_TYPES
        .iter()
        .find(|(e, _)| *e == ext)
        .map(|(_, t)| *t)
        .unwrap_or("application/octet-stream")
}

/// Ask the desktop to open the app. Best effort: a failure here is a printed
/// URL the user can click, not an error.
fn open(url: &str) {
    let (cmd, args): (&str, &[&str]) = if cfg!(target_os = "macos") {
        ("open", &[])
    } else if cfg!(windows) {
        ("cmd", &["/C", "start", ""])
    } else {
        ("xdg-open", &[])
    };
    let _ = std::process::Command::new(cmd)
        .args(args)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// The URLs a card-art cache is filled from, and where each file goes.
///
/// The Python build downloaded these itself. Nothing in this workspace speaks
/// HTTPS — that would take a TLS dependency, and the whole point of the
/// zero-dependency rule is that the binary cannot make a network request by
/// accident — so this prints the list instead and the user fetches it with
/// whatever they already have:
///
/// ```text
/// tavernsim art-urls --heroes | while read url dest; do
///     mkdir -p "$(dirname "$dest")" && curl -sSfo "$dest" "$url"
/// done
/// ```
pub fn art_urls(heroes_only: bool) {
    use tavernlab_core::cards::{PLAYABLE_CLASSES, all, by_id, is_implemented};

    const BASE: &str = "https://art.hearthstonejson.com/v1";
    let home = paths::data_home();
    let cache = home.join("art_cache");
    for class in PLAYABLE_CLASSES {
        let Some(hero) = hero_id(class) else { continue };
        if by_id(hero).is_none() {
            continue;
        }
        println!(
            "{BASE}/512x/{hero}.jpg\t{}",
            cache
                .join("hero")
                .join(format!(
                    "{}.jpg",
                    tavernlab_core::gauntlet::class_name(class)
                ))
                .display()
        );
    }
    if heroes_only {
        return;
    }
    let mut ids: Vec<&str> = all()
        .filter(|c| {
            let d = c.def();
            d.collectible && d.deckable() && is_implemented(*c)
        })
        .map(|c| c.info().id)
        .collect();
    ids.sort_unstable();
    ids.dedup();
    for id in ids {
        println!(
            "{BASE}/tiles/{id}.png\t{}",
            cache.join("tile").join(format!("{id}.png")).display()
        );
    }
}

/// The classic portrait each class's art is filed under.
fn hero_id(class: tavernlab_core::cards::Class) -> Option<&'static str> {
    use tavernlab_core::cards::Class;
    Some(match class {
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_cannot_climb_out_of_the_web_root() {
        let root = Path::new("/srv/web/dist");
        assert_eq!(
            safe_join(root, "assets/app.js"),
            Some(PathBuf::from("/srv/web/dist/assets/app.js"))
        );
        assert_eq!(
            safe_join(root, "./index.html"),
            Some(root.join("index.html"))
        );
        assert_eq!(safe_join(root, "../../etc/passwd"), None);
        assert_eq!(safe_join(root, "assets/../../secret"), None);
        // An absolute path would otherwise replace the root entirely.
        assert_eq!(safe_join(root, "/etc/passwd"), None);
    }

    #[test]
    fn content_types_cover_what_vite_emits() {
        assert_eq!(
            content_type(Path::new("a/index.html")),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            content_type(Path::new("a/app.JS")),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(
            content_type(Path::new("a/style.css")),
            "text/css; charset=utf-8"
        );
        assert_eq!(content_type(Path::new("a/font.woff2")), "font/woff2");
        assert_eq!(
            content_type(Path::new("a/thing.bin")),
            "application/octet-stream"
        );
    }
}
