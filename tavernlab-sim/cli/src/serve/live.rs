//! The watcher, running inside the app rather than beside it.
//!
//! [`crate::watch`] reads the client's own log and says what to keep, what
//! the opponent is playing and what to do this turn. It was a second command
//! in a second terminal, with the deck code and the log directory passed as
//! flags; this owns the same [`Runner`](crate::watch::Runner) on a thread of
//! the server's, so the answer arrives on the page the deck was pasted into.
//!
//! What it publishes is a snapshot, not a stream. A poll of `/api/live`
//! returns whatever the last tick built, which is the same thing the terminal
//! would have printed, and a page that missed three ticks is not behind — it
//! is looking at the position as it stands.
//!
//! Starting and stopping is the user's, not the server's: the watcher reads
//! the game you are playing, and something that reads your game should run
//! because you asked it to.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::watch_mod::{self as watch, Advice, Line, Runner, Tick};

use super::state::App;

/// Everything `/api/live` can say.
#[derive(Clone, Default)]
pub struct Snapshot {
    pub running: bool,
    /// The Power.log being followed, once one has been found.
    pub watching: Option<String>,
    /// Why there is nothing to show, when there is nothing to show. A note
    /// rather than an error: waiting for the client to start is the normal
    /// state of a watcher that was switched on first.
    ///
    /// A key and its values, like every other line the watcher produces --
    /// the page writes it out in its own language.
    pub note: Option<Line>,
    /// The advice as it stands, or `None` before the first game is seen.
    pub advice: Option<Advice>,
    /// Games written to the history since this watcher started.
    pub recorded: u64,
}

/// The watcher's thread and the last thing it produced.
#[derive(Default)]
pub struct Live {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// Set while a thread is running. Raising it asks that thread to stop,
    /// and the thread clears its own slot on the way out -- so a stop that
    /// races a crash cannot leave the app claiming to be watching.
    stop: Option<Arc<AtomicBool>>,
    snapshot: Snapshot,
}

impl Live {
    pub fn snapshot(&self) -> Snapshot {
        self.inner.lock().expect("live lock").snapshot.clone()
    }

    pub fn running(&self) -> bool {
        self.inner.lock().expect("live lock").stop.is_some()
    }

    /// Ask the watcher to stop. A watcher that is not running is not an
    /// error: the button says stop and the answer is that it is stopped.
    pub fn stop(&self) {
        let mut inner = self.inner.lock().expect("live lock");
        if let Some(flag) = inner.stop.take() {
            flag.store(true, Ordering::Relaxed);
        }
        inner.snapshot.running = false;
    }

    fn publish(&self, f: impl FnOnce(&mut Snapshot)) {
        let mut inner = self.inner.lock().expect("live lock");
        f(&mut inner.snapshot);
    }
}

/// Where to look for the client's logs: what the user set, else what the
/// environment says, else the usual place on this platform.
pub fn logs_dir(app: &App) -> Option<PathBuf> {
    let set = app.settings().get("logs_dir").cloned().unwrap_or_default();
    let dir = if !set.is_empty() {
        PathBuf::from(set)
    } else {
        std::env::var_os("HS_LOGS")
            .map(PathBuf::from)
            .or_else(watch::default_logs_dir)?
    };
    Some(watch::resolve_logs_dir(dir))
}

/// Start watching, and answer with the directory it settled on.
///
/// The three things it needs are all already on the site: the deck comes from
/// the lab's own settings, the battletag is learned from the log (and can be
/// overridden in Settings when the log never spells it), and the directory is
/// guessed and then editable.
/// Which gauntlet the mulligan and the opponent read are measured against.
///
/// The deck code says so itself, so nothing has to be chosen twice: a Wild
/// list is read against the Wild field. Standard where the code did not say,
/// which is what every other screen assumes for the same reason.
///
/// Hardcoding Standard here meant a Wild deck was advised against a field it
/// does not play in, silently.
fn format_of(app: &App) -> &'static str {
    let deck = app.settings().get("deckstring").cloned().unwrap_or_default();
    tavernlab_core::deckstring::resolve(&deck)
        .ok()
        .and_then(|r| r.format)
        .map(super::state::format_name)
        .unwrap_or("standard")
}

pub fn start(app: &Arc<App>) -> Result<PathBuf, String> {
    if app.live.running() {
        return Err("вже стежу".into());
    }
    let Some(dir) = logs_dir(app) else {
        return Err(
            "не знаю, де логи гри — вкажіть теку в Налаштуваннях (поруч із теками \
             Logs лежить log.config, яким вмикається детальне логування)"
                .into(),
        );
    };
    if !dir.is_dir() {
        return Err(format!("{} — такої теки немає", dir.display()));
    }
    let deck = app.settings().get("deckstring").cloned().unwrap_or_default();
    let me = app.settings().get("battletag").cloned().unwrap_or_default();
    let me = (!me.is_empty()).then_some(me);

    // Claimed before the thread exists, so that two starts a millisecond
    // apart cannot both find the watcher stopped.
    let stop = Arc::new(AtomicBool::new(false));
    {
        let mut inner = app.live.inner.lock().expect("live lock");
        inner.stop = Some(Arc::clone(&stop));
        inner.snapshot = Snapshot {
            running: true,
            note: Some(Line::new("live.note.reading").with("dir", dir.display().to_string())),
            ..Snapshot::default()
        };
    }

    let worker = Arc::clone(app);
    let format = format_of(app).to_string();
    let here = dir.clone();
    let token = Arc::clone(&stop);
    if let Err(e) = std::thread::Builder::new()
        .name("tavernlab-live".into())
        .spawn(move || run(&worker, &format, &deck, here, me, token))
    {
        let mut inner = app.live.inner.lock().expect("live lock");
        inner.stop = None;
        inner.snapshot = Snapshot::default();
        return Err(format!("не вдалося запустити стеження: {e}"));
    }
    Ok(dir)
}

/// The loop itself. One tick every [`watch::POLL`], the same cadence the
/// terminal command polls at.
fn run(
    app: &Arc<App>,
    format: &str,
    deck: &str,
    dir: PathBuf,
    me: Option<String>,
    stop: Arc<AtomicBool>,
) {
    let mut runner = Runner::new(dir.clone(), me);
    let mut recorded = 0u64;
    for e in runner.catch_up() {
        eprintln!("live: {e}");
    }
    recorded += write(app, format, deck, runner.take_finished());
    if runner.watching().is_some() {
        let advice = watch::build_advice(app, format, runner.tracker(), deck);
        app.live.publish(|s| {
            s.advice = Some(advice);
            s.watching = path_of(runner.watching());
            s.note = None;
            s.recorded = recorded;
        });
    } else {
        let note = Line::new("live.note.waiting").with("dir", dir.display().to_string());
        app.live.publish(|s| s.note = Some(note));
    }

    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(watch::POLL);
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let tick = runner.poll();
        recorded += write(app, format, deck, runner.take_finished());
        let rebuild = match tick {
            Tick::Quiet | Tick::Waiting => false,
            Tick::Lost(why) => {
                app.live.publish(|s| {
                    s.note = Some(why);
                    s.watching = None;
                });
                false
            }
            Tick::Session(_) => true,
            Tick::Read { changed } => changed,
        };
        if rebuild {
            let advice = watch::build_advice(app, format, runner.tracker(), deck);
            app.live.publish(|s| {
                s.advice = Some(advice);
                s.watching = path_of(runner.watching());
                s.note = None;
            });
        }
        app.live.publish(|s| s.recorded = recorded);
    }

    // The thread clears its own slot, so "running" is a fact about a live
    // thread rather than about the last button pressed -- and only its own,
    // so a thread on its way out cannot switch off the one that replaced it.
    let mut inner = app.live.inner.lock().expect("live lock");
    if inner.stop.as_ref().is_some_and(|s| Arc::ptr_eq(s, &stop)) {
        inner.stop = None;
        inner.snapshot.running = false;
        inner.snapshot.note = Some(Line::new("live.note.stopped"));
    }
}

fn path_of(p: Option<&Path>) -> Option<String> {
    p.map(|p| p.display().to_string())
}

/// Append finished games and count what was new. A history that cannot be
/// written costs the record, not the session.
fn write(
    app: &App,
    format: &str,
    deck: &str,
    finished: Vec<(i64, watch::tracker::Tracker)>,
) -> u64 {
    match watch::record_games(app, format, deck, None, finished) {
        Ok(n) => n as u64,
        Err(e) => {
            eprintln!("live: не вдалося записати історію: {e}");
            0
        }
    }
}
