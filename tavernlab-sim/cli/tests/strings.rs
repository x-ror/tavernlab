//! Every key the watcher emits has words behind it, in both languages.
//!
//! A [`Line`](tavernsim::watch::advice::Line) that names a key the locale
//! files do not carry renders as the key itself: `live.plan.play` on the
//! screen where a play should be. Nothing else catches that -- the code
//! compiles, the server answers, the page draws a line -- so this reads the
//! source for the keys it builds and checks both files hold them.
//!
//! Scanning the source rather than a list kept by hand, because a list kept
//! by hand is the thing that goes stale the first time a line is added.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    // `cli/` -> `tavernlab-sim/` -> the checkout.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the checkout is two levels above the crate")
        .to_path_buf()
}

/// The keys `Line::new("...")` and `Arg::Key("...")` name, across the binary.
fn keys_in_source() -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut stack = vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read the source tree") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().is_some_and(|e| e == "rs") {
                let src = std::fs::read_to_string(&path).expect("read a source file");
                for call in ["Line::new(\"", "Arg::Key(\""] {
                    let mut rest = src.as_str();
                    while let Some(at) = rest.find(call) {
                        rest = &rest[at + call.len()..];
                        if let Some(end) = rest.find('"') {
                            found.insert(rest[..end].to_string());
                        }
                    }
                }
            }
        }
    }
    found
}

fn locale(lang: &str) -> BTreeSet<String> {
    let path = repo_root().join("locales").join(format!("{lang}.json"));
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let doc = tavernlab_json::Json::parse(&src).expect("the locale file parses");
    doc.as_object()
        .expect("a locale file is an object")
        .iter()
        .map(|(k, _)| k.clone())
        .collect()
}

#[test]
fn every_key_the_watcher_builds_has_words_in_both_languages() {
    let keys = keys_in_source();
    assert!(
        keys.len() > 40,
        "the scan found only {} keys, which means it stopped finding them",
        keys.len()
    );
    for lang in ["uk", "en"] {
        let have = locale(lang);
        let missing: Vec<&String> = keys.iter().filter(|k| !have.contains(*k)).collect();
        assert!(missing.is_empty(), "locales/{lang}.json is missing {missing:?}");
    }
}

#[test]
fn the_two_languages_carry_the_same_keys() {
    // A key in one file and not the other is a screen that is half
    // translated, which reads as a bug rather than as a missing string.
    let uk = locale("uk");
    let en = locale("en");
    let only_uk: Vec<&String> = uk.difference(&en).collect();
    let only_en: Vec<&String> = en.difference(&uk).collect();
    assert!(only_uk.is_empty(), "only in uk: {only_uk:?}");
    assert!(only_en.is_empty(), "only in en: {only_en:?}");
}
