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

use super::http::Response;

/// `memreader`'s own JSON is already exactly the document this endpoint
/// wants to serve — see `memreader/README.md`'s `--snapshot` section — so
/// this validates it parses (never forward a truncated or garbled write
/// straight to a browser) and then echoes the original bytes rather than
/// walking the tree and re-emitting it field by field.
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
    match tavernlab_json::Json::parse(&stdout) {
        Ok(_) => Response::json(200, stdout),
        Err(e) => Response::error(
            502,
            &format!("вивід memreader --snapshot не є валідним JSON: {}", e.msg),
        ),
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
