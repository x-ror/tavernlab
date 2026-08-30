//! Card names in a row, without the row reading as more names than it holds.
//!
//! Hearthstone names carry commas -- `Husk, Eternal Reaper`, `Deathwing,
//! Worldbreaker`, `Sir Finley, Sea Guide` -- so a list joined with `", "`
//! cannot be read back. `Arisen Onyxia, Hematurge, Husk, Eternal Reaper` is
//! three cards and reads as four, and nothing in the line says which.
//!
//! So the separator is escaped by quoting: when any name holds a comma, every
//! name is wrapped. Every, not just the offender -- a list where some entries
//! are quoted and some are not makes the reader work out why, and the answer
//! ("that one has a comma in it") is the thing being hidden. When no name
//! holds one the quotes would be noise, and are left out.

/// The names, in the order given.
pub fn list<S: AsRef<str>>(names: &[S]) -> String {
    let ambiguous = names.iter().any(|n| n.as_ref().contains(','));
    let mut out = String::new();
    for (i, n) in names.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        if ambiguous {
            out.push('«');
            out.push_str(n.as_ref());
            out.push('»');
        } else {
            out.push_str(n.as_ref());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_list_is_left_alone() {
        assert_eq!(list(&["Fireball", "Chillwind Yeti"]), "Fireball, Chillwind Yeti");
    }

    #[test]
    fn one_comma_quotes_the_whole_list() {
        // The case that started this: three cards that read as four.
        assert_eq!(
            list(&["Arisen Onyxia", "Hematurge", "Husk, Eternal Reaper"]),
            "«Arisen Onyxia», «Hematurge», «Husk, Eternal Reaper»"
        );
    }

    #[test]
    fn one_name_needs_no_separator_and_no_quotes() {
        assert_eq!(list(&["Fireball"]), "Fireball");
        assert_eq!(list(&["Husk, Eternal Reaper"]), "«Husk, Eternal Reaper»");
    }

    #[test]
    fn nothing_is_nothing() {
        let empty: [&str; 0] = [];
        assert_eq!(list(&empty), "");
    }
}
