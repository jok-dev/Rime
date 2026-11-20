//! What a run says once instead of once per `require`.
//!
//! A `require` Rime does not follow is not a defect in the code: a string
//! handed to the project's own loader, an alias no `.luaurc` in the
//! checkout defines, a file that rebinds `require`. Reporting each call
//! would bury a codebase written in that style under one error per
//! module. The run counts them and says each kind once, in a sentence,
//! with where the first one is.

use rime_check::project::RequireNote;
use std::collections::{BTreeMap, BTreeSet};

/// The kinds a run reports, in the order it prints them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    BareString,
    UnknownAlias,
    AliasNotOnDisk,
    Rebound,
    InitAmbiguous,
    InitRelative,
}

#[derive(Default)]
struct Tally {
    requires: usize,
    files: usize,
    /// `path:line` of the first one, in report order.
    first: Option<String>,
    /// The alias names involved, for the two alias kinds.
    aliases: BTreeSet<String>,
}

/// Counts of the requires a run did not follow (or followed with a
/// remark), by kind.
#[derive(Default)]
pub struct RequireNotes {
    tallies: BTreeMap<Kind, Tally>,
}

/// How many alias names a line spells out before "and N more".
const ALIASES_SHOWN: usize = 4;

impl RequireNotes {
    /// Add one file's notes. `path` is the file as the run displays it;
    /// `src` its text, for the line of the first note.
    pub fn add_file(&mut self, path: &str, src: &str, notes: &[(rime_diag::Span, RequireNote)]) {
        let mut seen: BTreeSet<Kind> = BTreeSet::new();
        for (span, note) in notes {
            let (kind, alias) = match note {
                RequireNote::BareString => (Kind::BareString, None),
                RequireNote::UnknownAlias(alias) => (Kind::UnknownAlias, Some(alias)),
                RequireNote::AliasNotOnDisk(alias) => (Kind::AliasNotOnDisk, Some(alias)),
                RequireNote::Rebound => (Kind::Rebound, None),
                RequireNote::InitAmbiguous => (Kind::InitAmbiguous, None),
                RequireNote::InitRelative => (Kind::InitRelative, None),
            };
            let tally = self.tallies.entry(kind).or_default();
            tally.requires += 1;
            if seen.insert(kind) {
                tally.files += 1;
            }
            if tally.first.is_none() {
                let upto = span.start.min(src.len());
                let line = src.as_bytes()[..upto].iter().filter(|b| **b == b'\n').count() + 1;
                tally.first = Some(format!("{path}:{line}"));
            }
            if let Some(alias) = alias {
                tally.aliases.insert(alias.clone());
            }
        }
    }

    /// One sentence per kind seen, in a fixed order.
    pub fn lines(&self) -> Vec<String> {
        self.tallies
            .iter()
            .map(|(kind, tally)| {
                let n = tally.requires;
                let count = format!(
                    "{n} require{} in {} file{}",
                    if n == 1 { "" } else { "s" },
                    tally.files,
                    if tally.files == 1 { "" } else { "s" },
                );
                let was = if n == 1 { "was" } else { "were" };
                let first = tally.first.as_deref().unwrap_or("?");
                let aliases = alias_list(&tally.aliases);
                match kind {
                    Kind::BareString => format!(
                        "{count} {was} not followed: a string that does not start with ./, ../ or @ \
                         (such as require(\"Name\")) is not a Luau module path, so only the project's own \
                         loader knows what it names. Calls into those modules are not checked across \
                         files. First: {first}"
                    ),
                    Kind::UnknownAlias => format!(
                        "{count} {was} not followed: no .luaurc at or above the file defines {aliases}. \
                         Calls into those modules are not checked across files. First: {first}"
                    ),
                    Kind::AliasNotOnDisk => format!(
                        "{count} {was} not followed: .luaurc points {aliases} at a path that is not on \
                         disk here (packages not installed, or a library built into another runtime). \
                         Calls into those modules are not checked across files. First: {first}"
                    ),
                    Kind::Rebound => format!(
                        "{count} {was} not followed: the file gives the name `require` a new meaning \
                         (`local require = ...`, or a parameter), so those calls go to the project's own \
                         function. Calls into those modules are not checked across files. First: {first}"
                    ),
                    Kind::InitAmbiguous => format!(
                        "{count} in init files {was} not followed: \"./x\" there names a module beside \
                         the folder under current Luau and one inside the folder under darklua and \
                         older Luau, and both exist. \"@self/x\" always means the one inside. \
                         First: {first}"
                    ),
                    Kind::InitRelative => format!(
                        "{count} in init files {was} followed from the init file's own folder, which is \
                         how darklua and older Luau read \"./x\". Current Luau reads it from the folder's \
                         parent and uses \"@self/x\" for the folder itself. First: {first}"
                    ),
                }
            })
            .collect()
    }
}

/// `@a`, `@a and @b`, `@a, @b, @c, @d and 3 more`.
fn alias_list(aliases: &BTreeSet<String>) -> String {
    let shown: Vec<String> = aliases.iter().take(ALIASES_SHOWN).map(|a| format!("@{a}")).collect();
    let more = aliases.len().saturating_sub(shown.len());
    let noun = if aliases.len() == 1 { "the alias" } else { "the aliases" };
    let list = match (shown.len(), more) {
        (0, _) => String::new(),
        (1, 0) => shown[0].clone(),
        (_, 0) => format!("{} and {}", shown[..shown.len() - 1].join(", "), shown[shown.len() - 1]),
        (_, more) => format!("{} and {more} more", shown.join(", ")),
    };
    format!("{noun} {list}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start: usize) -> rime_diag::Span {
        start..start + 1
    }

    #[test]
    fn one_line_per_kind_with_counts_and_the_first_place() {
        let mut notes = RequireNotes::default();
        let src = "local a = require(\"A\")\nlocal b = require(\"B\")\n";
        notes.add_file("src/a.luau", src, &[(span(10), RequireNote::BareString), (span(33), RequireNote::BareString)]);
        notes.add_file("src/b.luau", src, &[(span(10), RequireNote::BareString)]);
        let lines = notes.lines();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("3 requires in 2 files were not followed"), "{}", lines[0]);
        assert!(lines[0].ends_with("First: src/a.luau:1"), "{}", lines[0]);
    }

    #[test]
    fn alias_names_are_listed_and_capped() {
        let mut notes = RequireNotes::default();
        let all: Vec<(rime_diag::Span, RequireNote)> = ["f", "e", "d", "c", "b", "a"]
            .iter()
            .map(|a| (span(0), RequireNote::UnknownAlias((*a).to_string())))
            .collect();
        notes.add_file("x.luau", "", &all);
        notes.add_file("y.luau", "\n\n", &[(span(2), RequireNote::AliasNotOnDisk("pkg".into()))]);
        let lines = notes.lines();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("the aliases @a, @b, @c, @d and 2 more"), "{}", lines[0]);
        assert!(lines[0].starts_with("6 requires in 1 file were"), "{}", lines[0]);
        assert!(lines[1].contains("the alias @pkg at a path"), "{}", lines[1]);
        assert!(lines[1].starts_with("1 require in 1 file was"), "{}", lines[1]);
        assert!(lines[1].ends_with("First: y.luau:3"), "{}", lines[1]);
    }
}
