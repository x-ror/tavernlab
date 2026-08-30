//! The server, end to end.
//!
//! Everything else about `serve` is unit-tested at the piece level — path
//! joining, `Host` checks, job book-keeping. This starts the real binary on a
//! real socket and asks it the questions the front end asks, because the
//! failures that actually reach a user live in the wiring: a route that is
//! never reached, a body that is not read, a response the browser cannot
//! parse.
//!
//! The client is hand-rolled for the same reason the server is: no
//! dependencies. It speaks exactly the dialect the server does.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A server process, stopped when the test ends.
struct Server {
    child: Child,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Server {
    fn start() -> Server {
        Server::start_in(None)
    }

    /// Start with a prepared data home, for a test that needs a file the
    /// server reads rather than one it writes.
    fn start_with_home(home: &std::path::Path) -> Server {
        Server::start_in(Some(home.to_path_buf()))
    }

    fn start_in(prepared: Option<std::path::PathBuf>) -> Server {
        // Ask the OS for a free port and hand it straight over: a fixed port
        // makes the test fail when the developer has the app open.
        let port = TcpListener::bind(("127.0.0.1", 0))
            .expect("a free port")
            .local_addr()
            .expect("its address")
            .port();
        let home = prepared
            .unwrap_or_else(|| std::env::temp_dir().join(format!("tavernlab-serve-test-{port}")));
        let _ = std::fs::create_dir_all(&home);

        let child = Command::new(env!("CARGO_BIN_EXE_tavernsim"))
            .args(["serve", &port.to_string(), "--no-open"])
            .env("TAVERNLAB_HOME", &home)
            // The repo root is found by walking up from the working
            // directory, which for a test is the crate directory.
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("the server binary should start");
        let server = Server { child, port };

        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if TcpStream::connect(("127.0.0.1", port)).is_ok() {
                return server;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("the server never started listening on {port}");
    }

    fn request(&self, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(60)))
            .expect("timeout");
        let body = body.unwrap_or("");
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n",
            self.port,
            body.len()
        );
        stream.write_all(head.as_bytes()).expect("write head");
        stream.write_all(body.as_bytes()).expect("write body");

        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).expect("read response");
        let text = String::from_utf8_lossy(&raw).into_owned();
        let (head, body) = text.split_once("\r\n\r\n").expect("a complete response");
        let status = head
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .expect("a status line");
        (status, body.to_string())
    }

    fn get(&self, path: &str) -> (u16, String) {
        self.request("GET", path, None)
    }

    fn post(&self, path: &str, body: &str) -> (u16, String) {
        self.request("POST", path, Some(body))
    }

    /// Poll a job until it stops running.
    fn finish(&self, start_body: &str) -> tavernlab_json::Json {
        let id = json(start_body).str_or_empty("job").to_string();
        assert!(!id.is_empty(), "no job id in {start_body}");
        let deadline = Instant::now() + Duration::from_secs(120);
        while Instant::now() < deadline {
            let (_, body) = self.get(&format!("/api/job/{id}"));
            let doc = json(&body);
            match doc.str_or_empty("status") {
                "running" => std::thread::sleep(Duration::from_millis(100)),
                _ => return doc,
            }
        }
        panic!("job {id} never finished");
    }
}

fn json(body: &str) -> tavernlab_json::Json {
    tavernlab_json::Json::parse(body).unwrap_or_else(|e| panic!("not JSON: {e}\n{body}"))
}

/// A deck the engine can actually field: the first playable gauntlet deck,
/// taken through the same export the UI's "use as mine" button uses.
fn playable_code(server: &Server) -> String {
    let (_, body) = server.post("/api/meta", r#"{"format":"standard"}"#);
    let doc = json(&body);
    for deck in doc.arr_or_empty("decks") {
        if deck.bool_or_false("playable") {
            return deck.str_or_empty("deckstring").to_string();
        }
    }
    panic!("no playable deck in the Standard gauntlet");
}

#[test]
fn the_server_answers_every_route_the_front_end_calls() {
    let server = Server::start();

    // --- reads
    let (status, body) = server.get("/api/metrics");
    assert_eq!(status, 200);
    let metrics = json(&body);
    assert!(metrics.i64_or_zero("cards") > 10_000, "{body}");
    assert!(metrics.i64_or_zero("threads") >= 1);

    let (status, body) = server.get("/api/settings");
    assert_eq!(status, 200);
    assert!(json(&body).get("settings").is_some(), "{body}");

    let (status, body) = server.get("/locales/uk.json");
    assert_eq!(status, 200);
    assert!(!json(&body).str_or_empty("app.title").is_empty(), "{body}");
    assert_eq!(server.get("/locales/../../../etc/passwd.json").0, 404);

    // A tier table nobody has computed is `null`, never a computation on a
    // GET: the matrix is quadratic.
    let (status, body) = server.get("/api/tiers?format=standard");
    assert_eq!(status, 200);
    assert_eq!(json(&body).get("decks"), Some(&tavernlab_json::Json::Null));

    // --- settings round trip
    let (status, body) = server.post(
        "/api/settings",
        r#"{"language":"uk","not_a_setting":"ignored"}"#,
    );
    assert_eq!(status, 200);
    let settings = json(&body);
    let stored = settings.get("settings").expect("settings object");
    assert_eq!(stored.str_or_empty("language"), "uk");
    assert!(stored.get("not_a_setting").is_none(), "{body}");

    // --- the deck screens
    let code = playable_code(&server);
    let ask = format!("{{\"code\":{}}}", tavernlab_json::escape(&code));

    let (status, body) = server.post("/api/resolve", &ask);
    assert_eq!(status, 200);
    let resolved = json(&body);
    assert!(resolved.bool_or_false("ok"), "{body}");
    assert_eq!(resolved.i64_or_zero("total"), 30);
    assert_eq!(resolved.str_or_empty("format"), "standard");

    let (_, body) = server.post(
        "/api/analyze",
        &format!(
            "{{\"code\":{},\"games\":120}}",
            tavernlab_json::escape(&code)
        ),
    );
    let job = server.finish(&body);
    assert_eq!(job.str_or_empty("status"), "done", "{body}");
    let result = job.get("result").expect("a result");
    let avg = result.get("avg").and_then(tavernlab_json::Json::as_f64);
    assert!(matches!(avg, Some(v) if (0.0..=1.0).contains(&v)), "{body}");
    // Every average over the field says how much of the field it covers.
    assert!(result.i64_or_zero("field_played") >= 1);
    assert!(result.i64_or_zero("field_decks") >= result.i64_or_zero("field_played"));

    // Mulligan and coach notes run from a cold start: there is no "analyse
    // first" gate any more, and this is what would catch one coming back.
    let (status, body) = server.post(
        "/api/mull",
        &format!(
            "{{\"code\":{},\"opp\":\"DRUID\",\"hand\":[\"Fireball\"]}}",
            tavernlab_json::escape(&code)
        ),
    );
    assert_eq!(status, 200, "{body}");
    let mull = json(&body);
    let card = &mull.arr_or_empty("cards")[0];
    assert_eq!(card.str_or_empty("card"), "Fireball");
    assert!(
        card.get("keep")
            .and_then(tavernlab_json::Json::as_bool)
            .is_some()
    );
    // Reasons travel as keys, never as a sentence: the UI is bilingual.
    assert!(!card.arr_or_empty("why").is_empty(), "{body}");
    assert!(!card.arr_or_empty("why")[0].str_or_empty("k").is_empty());

    let (status, body) = server.post("/api/coach", &ask);
    assert_eq!(status, 200, "{body}");
    let coach = json(&body);
    assert!(!coach.arr_or_empty("weak").is_empty(), "{body}");

    let (status, body) = server.post("/api/predict", r#"{"opp":"DRUID","seen":["Innervate"]}"#);
    assert_eq!(status, 200, "{body}");
    assert!(!json(&body).arr_or_empty("decks").is_empty());

    let (status, body) = server.post("/api/cardnames", r#"{"all":true}"#);
    assert_eq!(status, 200);
    assert!(json(&body).arr_or_empty("names").len() > 1000, "{body}");
}

#[test]
fn a_deck_the_engine_cannot_field_is_refused_by_name() {
    // The rule the whole product rests on: never quietly approximate. A list
    // with an unimplemented card must come back naming it, not scored.
    //
    // The offending card is chosen at run time rather than frozen into a
    // deck code: a hard-coded list stops testing anything the moment its one
    // unimplemented card gets implemented, and the test then passes because
    // the deck became legal.
    let server = Server::start();
    let base = "AAECAaoICsmeBsODB9C/B/nDB4LUB5vUB8/bB9DbB4jdB9/lBwqe1ATt5gbgnQexsAePvge1wAfJwAfJ2wfI5Qfm/QcAAA==";
    let mut deck = tavernlab_core::deckstring::decode(base).expect("the base list decodes");
    let offender = (0..u16::MAX)
        .map(tavernlab_core::cards::CardId)
        .find(|c| {
            let d = c.def();
            d.collectible
                && d.deckable()
                && d.class() == tavernlab_core::cards::Class::Neutral
                && d.formats.has(tavernlab_core::cards::Formats::STANDARD)
                && !tavernlab_core::cards::is_implemented(*c)
        })
        .expect("some Standard Neutral card is still unimplemented");
    // Neutral, so it is legal in whatever class the base list is.
    deck.cards[0] = (offender.info().dbf, 1);
    let code = tavernlab_core::deckstring::encode(deck.heroes[0], &deck.cards, deck.format);

    let (status, body) = server.post("/api/resolve", &format!("{{\"code\":\"{code}\"}}"));
    assert_eq!(status, 200);
    let doc = json(&body);
    assert!(!doc.bool_or_false("ok"), "{body}");
    assert!(!doc.arr_or_empty("unimplemented").is_empty(), "{body}");

    let (_, body) = server.post("/api/analyze", &format!("{{\"code\":\"{code}\"}}"));
    let job = server.finish(&body);
    assert_eq!(job.str_or_empty("status"), "error");
    assert!(
        job.str_or_empty("error").contains("cannot play"),
        "the error should name the problem: {}",
        job.str_or_empty("error")
    );
}

#[test]
fn an_exported_deck_code_carries_its_own_gauntlets_format() {
    // Every Standard card is Wild-legal too, so a format read off the card
    // list tags a Wild deck as Standard — and the code then comes back
    // "not legal in standard" the moment the player pastes it in.
    let server = Server::start();
    for (format, want) in [("standard", 2u8), ("wild", 1u8)] {
        let (_, body) = server.post("/api/meta", &format!("{{\"format\":\"{format}\"}}"));
        let doc = json(&body);
        let code = doc
            .arr_or_empty("decks")
            .iter()
            .find_map(|d| {
                let c = d.str_or_empty("deckstring");
                (!c.is_empty()).then(|| c.to_string())
            })
            .expect("the gauntlet exports deck codes");
        let decoded = tavernlab_core::deckstring::decode(&code).expect("our own code decodes");
        assert_eq!(decoded.format, want as u32, "{format} deck code");

        // And the round trip holds: pasted back, it resolves in the same
        // format with nothing illegal in it.
        let (_, body) = server.post(
            "/api/resolve",
            &format!("{{\"code\":{}}}", tavernlab_json::escape(&code)),
        );
        let resolved = json(&body);
        assert_eq!(resolved.str_or_empty("format"), format);
        assert!(resolved.arr_or_empty("illegal").is_empty(), "{body}");
    }
}

#[test]
fn a_paste_that_is_not_a_deck_code_comes_back_as_a_key_to_translate() {
    let server = Server::start();
    // Written with escapes rather than a raw string: the paste itself
    // contains `"#`, which would end one.
    let paste = "{\"code\":\"### a title\\n# and comments\"}";
    let (status, body) = server.post("/api/resolve", paste);
    assert_eq!(status, 200);
    let doc = json(&body);
    assert!(!doc.bool_or_false("ok"));
    assert_eq!(doc.str_or_empty("error_code"), "only_comments");
    assert!(
        !doc.str_or_empty("error").is_empty(),
        "an English fallback too"
    );
}

#[test]
fn a_request_from_a_rebound_dns_name_is_refused() {
    // The one thing between this API and a page on the internet driving it
    // through the user's own browser.
    let server = Server::start();
    let mut stream = TcpStream::connect(("127.0.0.1", server.port)).expect("connect");
    stream
        .write_all(
            b"GET /api/metrics HTTP/1.1\r\nHost: evil.example.com\r\nConnection: close\r\n\r\n",
        )
        .expect("write");
    let mut raw = String::new();
    stream.read_to_string(&mut raw).expect("read");
    assert!(raw.starts_with("HTTP/1.1 403"), "{raw}");
}

#[test]
fn unknown_routes_and_methods_are_refused_rather_than_guessed() {
    let server = Server::start();
    assert_eq!(server.get("/api/not-a-route").0, 404);
    assert_eq!(server.post("/api/metrics", "{}").0, 404);
    // A GET of the UI with no build present is still an answer, not a hang.
    let (status, _) = server.get("/");
    assert!(status == 200 || status == 404, "unexpected status {status}");
}

/// The history the watcher wrote is the history the front end reads.
///
/// The two halves are separate processes writing and reading one SQLite file,
/// which is exactly where a format bug would hide: the writer agreeing with
/// itself proves nothing about the reader on the other side of a restart.
#[test]
fn the_history_written_by_watch_is_served_to_the_ui() {
    const LOG: &str = "\
D 09:00:00.0 [Power] GameState.DebugPrintPower() - CREATE_GAME
D 09:00:00.1 [Zone] ZoneChangeList.ProcessChanges() - id=1 local=False [entityName=Jaina Proudmoore id=64 zone=PLAY zonePos=0 cardId=HERO_08 player=1] zone from  -> FRIENDLY PLAY (Hero)
D 09:00:00.1 [Zone] ZoneChangeList.ProcessChanges() - id=2 local=False [entityName=Garrosh Hellscream id=65 zone=PLAY zonePos=0 cardId=HERO_01 player=2] zone from  -> OPPOSING PLAY (Hero)
D 09:00:00.2 [Zone] ZoneChangeList.ProcessChanges() - id=3 local=False [entityName=The Coin id=11 zone=DECK zonePos=0 cardId=GAME_005 player=1] zone from  -> FRIENDLY HAND
D 09:00:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=TURN value=12
D 09:05:00.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=Me#1 tag=PLAYSTATE value=WON
D 09:06:00.0 [Power] GameState.DebugPrintPower() - CREATE_GAME
D 09:06:00.1 [Zone] ZoneChangeList.ProcessChanges() - id=1 local=False [entityName=Jaina Proudmoore id=64 zone=PLAY zonePos=0 cardId=HERO_08 player=1] zone from  -> FRIENDLY PLAY (Hero)
D 09:06:00.1 [Zone] ZoneChangeList.ProcessChanges() - id=2 local=False [entityName=Uther Lightbringer id=65 zone=PLAY zonePos=0 cardId=HERO_04 player=2] zone from  -> OPPOSING PLAY (Hero)
D 09:06:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=TURN value=8
D 09:09:00.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=Them#2 tag=PLAYSTATE value=WON
";
    let home = std::env::temp_dir().join(format!("tavernlab-history-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).expect("home");
    let log = home.join("session.log");
    std::fs::write(&log, LOG).expect("write the log");

    let watch = Command::new(env!("CARGO_BIN_EXE_tavernsim"))
        .args(["watch", "--log"])
        .arg(&log)
        .args(["--me", "Me#1", "--once"])
        .env("TAVERNLAB_HOME", &home)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("watch runs");
    assert!(watch.status.success(), "watch failed");
    assert!(
        home.join("history.sqlite").exists(),
        "the watcher created the database"
    );

    let s = Server::start_with_home(&home);
    let (status, body) = s.get("/api/history");
    assert_eq!(status, 200, "{body}");
    let doc = json(&body);
    assert_eq!(doc.i64_or_zero("games"), 2, "{body}");
    assert_eq!(doc.i64_or_zero("resolved"), 2, "{body}");
    assert_eq!(doc.i64_or_zero("wins"), 1, "one of the two was mine: {body}");
    assert!(
        doc.str_or_empty("path").ends_with("history.sqlite"),
        "the page shows where the file is: {body}"
    );

    let rows = doc.arr_or_empty("rows");
    assert_eq!(rows.len(), 2);
    // Newest first, which is the game you just played.
    assert_eq!(rows[0].str_or_empty("opponent_class"), "PALADIN", "{body}");
    assert_eq!(rows[1].str_or_empty("opponent_class"), "WARRIOR", "{body}");
    assert_eq!(rows[1].i64_or_zero("turns"), 12);

    // Below the sample floor the rate is absent, not small: the UI must not
    // be able to render one game as a hundred per cent.
    for row in doc.arr_or_empty("by_opponent") {
        assert!(
            matches!(row.get("rate"), None | Some(tavernlab_json::Json::Null)),
            "{body}"
        );
    }
}
