//! Where TavernLab reads its data from, and where it writes yours.
//!
//! Two different places, and keeping them apart is the point:
//!
//! * the **repository root** holds what ships with the program — the card
//!   corpora, the gauntlets, the translations and the built web bundle;
//! * the **data home** holds what belongs to the person running it — their
//!   settings and any cached tier table. A checkout is not a place to keep
//!   somebody's data, and a binary that gets replaced should not take it
//!   with it.

use std::path::{Path, PathBuf};

pub const APP_NAME: &str = "TavernLab";

/// The per-user data directory, created if missing.
///
/// `TAVERNLAB_HOME` overrides it, for a portable install or a test that
/// must not touch the real one.
pub fn data_home() -> PathBuf {
    let home = match std::env::var_os("TAVERNLAB_HOME") {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => default_home(),
    };
    let _ = std::fs::create_dir_all(&home);
    home
}

fn default_home() -> PathBuf {
    let home_dir = || {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
    };
    if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(home_dir)
            .join(APP_NAME)
    } else if cfg!(target_os = "macos") {
        home_dir()
            .join("Library")
            .join("Application Support")
            .join(APP_NAME)
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir().join(".local").join("share"))
            .join(APP_NAME)
    }
}

/// The directory holding `data/` and `locales/`.
///
/// `TAVERNLAB_ROOT` overrides it. Otherwise it is found by walking up from
/// the current directory and from the executable, which covers running
/// `cargo run` from anywhere in the workspace and running an installed
/// binary from beside its data.
pub fn repo_root() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("TAVERNLAB_ROOT") {
        let p = PathBuf::from(p);
        return looks_like_root(&p).then_some(p);
    }
    let mut starts: Vec<PathBuf> = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        starts.push(cwd);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        starts.push(dir.to_path_buf());
    }
    for start in starts {
        for dir in start.ancestors() {
            if looks_like_root(dir) {
                return Some(dir.to_path_buf());
            }
        }
    }
    None
}

/// A root is a directory that has the data the server cannot invent.
fn looks_like_root(dir: &Path) -> bool {
    dir.join("data/gauntlet_standard.json").is_file() && dir.join("locales/en.json").is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_repo_root_is_found_from_inside_the_workspace() {
        // The tests run with the crate directory as the working directory,
        // which is two levels below the root — the exact case the ancestor
        // walk exists for.
        let root = repo_root().expect("the workspace root should be found from the cli crate");
        assert!(root.join("data/gauntlet_standard.json").is_file());
        assert!(root.join("locales/en.json").is_file());
    }

    #[test]
    fn a_directory_without_the_data_is_not_a_root() {
        assert!(!looks_like_root(Path::new("/")));
        assert!(!looks_like_root(Path::new("/nonexistent-directory-xyz")));
    }

    #[test]
    fn the_data_home_follows_the_environment() {
        // Not `data_home()` itself: it creates the directory, and a test
        // that depends on the ambient environment would be testing the
        // machine rather than the code.
        let home = default_home();
        assert!(home.ends_with(APP_NAME), "{}", home.display());
    }
}
