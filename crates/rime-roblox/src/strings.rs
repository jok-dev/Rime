//! Requires written as strings: `require("./x")`, `require("@alias/x")`.
//!
//! The rules are Luau's own (RFCs `require-by-string-relative-paths`,
//! `require-by-string-aliases`, `abstract-module-paths-and-init-dot-luau`;
//! the reference navigator in `luau/Require`), read onto the instance model
//! the rest of this crate already resolves through:
//!
//!   "./x", "../x"   relative to the PARENT of the requiring module. The
//!                   path starts at the requirer, steps up once, then takes
//!                   each part: `..` up, a name down, `.` and empty skipped.
//!                   In Roblox terms `./x` is `script.Parent.x`.
//!   "@self/x"       a child of the requiring module (`script.x`).
//!   "@game/A/B"     Roblox: from the DataModel root (`game.A.B`).
//!   "@name/x"       `name` from the nearest `.luaurc` that defines it,
//!                   searching up from the requiring MODULE's parent
//!                   folder. Names ignore case. A value starting `./` or
//!                   `../` is relative to that `.luaurc`'s folder; a value
//!                   starting `@` is another alias.
//!
//! `foo/init.luau` IS the module `foo`, so `./x` inside it names a sibling
//! of the `foo` folder and `@self/x` names something inside it. That falls
//! out of the instance model: `instance_of` already maps an init file to
//! its folder. For the same reason the `.luaurc` search from an init file
//! starts one folder above `foo`: `foo/.luaurc` is for the modules inside
//! `foo`, not for `foo` itself.
//!
//! A path with none of those prefixes is not a Luau path. Nothing here
//! guesses a meaning for it: it comes back `Unfollowed`, which the run
//! reports once, in words, instead of an error per call.
//!
//! Three things are accepted that Luau itself refuses, because the common
//! bundler for string requires (darklua's `path` mode) and Luau before
//! `@self` accept them, and code written for those is everywhere:
//!
//!   - a relative path that finds nothing under the rule above is tried
//!     relative to the requiring FILE's folder. The two differ only for
//!     `init` files. A hit there is followed and flagged
//!     (`RequireNote::InitRelative`), so the run can say the current Luau
//!     rule reads the path differently. When BOTH readings find a module
//!     and they are different files, nothing in the file says which
//!     toolchain it was written for, so the require is not followed
//!     (`RequireNote::InitAmbiguous`) rather than checked against a module
//!     that may be the wrong one.
//!   - `"."` and `".."` alone read as `"./"` and `"../"`.
//!   - a trailing `.luau` / `.lua` on the last part.
//!
//! Not read: `.config.luau` (the Luau-syntax config), darklua's own
//! `sources` table.
//!
//! A require string says how many parts a path has, so the work here is
//! kept in proportion to that number (see `crate::child`), and a string
//! longer than any path can be is not walked at all (`MAX_PATH_TEXT`).

use crate::{as_spelled, child, literal_text, RojoResolver};
use rime_check::project::{Loc, RequireNote, Resolution, Resolver};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

/// How many aliases an alias may be defined through (`"a": "@b/x"`). Luau
/// reports a cycle; a cap gives the same answer without tracking one.
const ALIAS_CHAIN_DEPTH: usize = 8;

/// The longest path an operating system takes, in characters: 32,767 on
/// Windows, 4,096 on Linux. A require string or an alias value longer
/// than this names nothing that can be on disk.
const MAX_PATH_TEXT: usize = 32_767;

/// How much of an over-long require string R0501 shows.
const SHOWN_OF_TOO_LONG: usize = 40;

/// Where an alias points.
enum AliasTarget {
    /// The instance path the alias names (extension stripped), on disk.
    At(PathBuf),
    /// Defined, and nothing is at the path it names.
    NotOnDisk,
    /// No `.luaurc` in reach defines it.
    Undefined,
}

impl RojoResolver {
    /// The string-literal require target in `target`, if it is one.
    pub(crate) fn string_target(target: &rime_hir::Expr) -> Option<String> {
        literal_text(target)
    }

    /// Resolve the string `text` as written in a `require` in `from`. The
    /// second half is a remark about a require that resolved (see
    /// `Resolver::note`). Memoized per (file, string): the checker asks
    /// about every site several times.
    pub fn resolve_string(&self, from: &Path, text: &str) -> (Resolution, Option<RequireNote>) {
        let key = (from.to_path_buf(), text.to_string());
        let cached = self.strings.read().unwrap().get(&key).cloned();
        if let Some(cached) = cached {
            return cached;
        }
        let out = self.resolve_string_uncached(from, text);
        self.strings.write().unwrap().insert(key, out.clone());
        out
    }

    fn resolve_string_uncached(&self, from: &Path, text: &str) -> (Resolution, Option<RequireNote>) {
        let path = text.replace('\\', "/");
        let path = match path.as_str() {
            "." => "./",
            ".." => "../",
            other => other,
        };
        let relative = path.starts_with("./") || path.starts_with("../");
        if path.len() > MAX_PATH_TEXT && (relative || path.starts_with('@')) {
            return (Resolution::Missing { described: too_long(text) }, None);
        }
        if relative {
            return self.resolve_relative(from, path, text);
        }
        let Some(aliased) = path.strip_prefix('@') else {
            return (Resolution::Unfollowed(RequireNote::BareString), None);
        };
        let (alias, rest) = aliased.split_once('/').unwrap_or((aliased, ""));
        let alias = alias.to_ascii_lowercase();
        let rest = parts(rest);
        match alias.as_str() {
            "" => (Resolution::Unfollowed(RequireNote::BareString), None),
            "self" => {
                let Some(start) = self.instance_of(from).map(Loc::Fs) else {
                    return (Resolution::Dynamic, None);
                };
                (self.resolve_from(start, &rest, text), None)
            }
            "game" => (self.resolve_from(Loc::Dm(Vec::new()), &rest, text), None),
            _ => {
                // The module's parent: the file's folder, or for an init
                // file (which is its folder) the folder above that.
                let dir = from.parent();
                let dir = if is_init(from) { dir.and_then(Path::parent) } else { dir };
                let Some(dir) = dir else {
                    return (Resolution::Dynamic, None);
                };
                match self.alias_target(dir, &alias, 0) {
                    AliasTarget::Undefined => {
                        (Resolution::Unfollowed(RequireNote::UnknownAlias(alias)), None)
                    }
                    AliasTarget::NotOnDisk => {
                        (Resolution::Unfollowed(RequireNote::AliasNotOnDisk(alias)), None)
                    }
                    AliasTarget::At(base) => (self.resolve_from(Loc::Fs(base), &rest, text), None),
                }
            }
        }
    }

    /// `./x` and `../x`: Luau's rule first, then the file's own folder.
    fn resolve_relative(&self, from: &Path, path: &str, text: &str) -> (Resolution, Option<RequireNote>) {
        let rest = parts(path);
        let start = self
            .instance_of(from)
            .map(Loc::Fs)
            .and_then(|script| self.step_norm(script, "Parent"));
        let by_rule = match start {
            Some(start) => self.resolve_from(start, &rest, text),
            None => Resolution::Dynamic,
        };
        let init = is_init(from);
        match by_rule {
            // A plain file: both readings start in the same folder.
            Resolution::Found(_) if !init => (by_rule, None),
            Resolution::Found(by_luau) => match self.beside_file(from, &rest) {
                Some(by_file) if by_file != by_luau => {
                    (Resolution::Unfollowed(RequireNote::InitAmbiguous), None)
                }
                _ => (Resolution::Found(by_luau), None),
            },
            by_rule => match self.beside_file(from, &rest) {
                Some(file) => (Resolution::Found(file), init.then_some(RequireNote::InitRelative)),
                None => (by_rule, None),
            },
        }
    }

    /// Walk `rest` from `start` and resolve what it lands on. A miss is
    /// retried once with a written-out `.luau` / `.lua` taken off the last
    /// part.
    fn resolve_from(&self, start: Loc, rest: &[&str], text: &str) -> Resolution {
        let first = self.resolve_walk(start.clone(), rest, text);
        if matches!(first, Resolution::Found(_)) {
            return first;
        }
        if let Some((last, lead)) = rest.split_last() {
            if let Some(stem) = last.strip_suffix(".luau").or_else(|| last.strip_suffix(".lua")) {
                if !stem.is_empty() {
                    let mut again: Vec<&str> = lead.to_vec();
                    again.push(stem);
                    let second = self.resolve_walk(start, &again, text);
                    if matches!(second, Resolution::Found(_)) {
                        return second;
                    }
                }
            }
        }
        first
    }

    fn resolve_walk(&self, start: Loc, rest: &[&str], text: &str) -> Resolution {
        let mut loc = start;
        for part in rest {
            loc = if *part == ".." {
                match self.step_norm(loc, "Parent") {
                    Some(up) => up,
                    // Above the top: nothing can be there.
                    None => return Resolution::Missing { described: text.to_string() },
                }
            } else {
                // A name is always a child, whatever it is called: `Parent`
                // in a string path is a module named Parent.
                match loc {
                    Loc::Fs(p) => Loc::Fs(child(p, part)),
                    Loc::Dm(mut segs) => {
                        segs.push((*part).to_string());
                        Loc::Dm(segs)
                    }
                }
            };
        }
        match Resolver::resolve_loc(self, &loc) {
            Resolution::Found(file) => {
                if self.names(&file, rest.last().copied()) {
                    Resolution::Found(file)
                } else {
                    Resolution::Missing { described: describe(self, text, &loc) }
                }
            }
            Resolution::Missing { .. } => Resolution::Missing { described: describe(self, text, &loc) },
            other => other,
        }
    }

    /// The file-relative reading: `rest` walked from the requiring file's
    /// own folder, on the filesystem, no project in the way. This is where
    /// darklua and pre-`@self` Luau look.
    fn beside_file(&self, from: &Path, rest: &[&str]) -> Option<PathBuf> {
        let mut dir = from.parent()?.to_path_buf();
        let mut name: Option<&str> = None;
        for (i, part) in rest.iter().enumerate() {
            let last = i + 1 == rest.len();
            if *part == ".." {
                if !dir.pop() {
                    return None;
                }
            } else if last {
                name = Some(part);
            } else {
                dir = child(dir, part);
            }
        }
        match name {
            Some(name) => {
                let stripped = name.strip_suffix(".luau").or_else(|| name.strip_suffix(".lua"));
                let found = self.project.module_file(&dir, name).filter(|f| self.names(f, Some(name)));
                found.or_else(|| {
                    let stem = stripped.filter(|s| !s.is_empty())?;
                    self.project.module_file(&dir, stem).filter(|f| self.names(f, Some(stem)))
                })
            }
            // The path ends on a folder (`".."`, `"./"`): the module is
            // that folder's init file.
            None => {
                let parent = dir.parent()?;
                let name = dir.file_name()?.to_string_lossy().into_owned();
                self.project.module_file(parent, &name)
            }
        }
    }

    /// Whether `file` is a module called `name`, to the letter. A Windows
    /// or macOS disk finds `config.luau` for `./Config`; Roblox and Luau
    /// on Linux do not. `module_file` already refuses a path that is on
    /// disk under another letter case, in any part. This is the same
    /// question for the last name alone, asked of the file that was found,
    /// so it also holds where the path and the file are not the same text
    /// (a folder reached through a link). A name that matches only when
    /// case is ignored is a miss. A name that does not match at all is
    /// left alone: a nested project can mount a folder under another name,
    /// and that is not this check's business.
    fn names(&self, file: &Path, name: Option<&str>) -> bool {
        let Some(name) = name.filter(|n| *n != "..") else {
            return true;
        };
        let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let stem = stem.strip_suffix(".client").or_else(|| stem.strip_suffix(".server")).unwrap_or(&stem);
        let plain = if stem == "init" {
            file.parent().and_then(Path::file_name).map(|n| n.to_string_lossy().into_owned())
        } else {
            Some(stem.to_string())
        };
        let collapsed = self
            .instance_of(file)
            .and_then(|inst| inst.file_name().map(|n| n.to_string_lossy().into_owned()));
        let candidates = [plain, collapsed];
        let mut candidates = candidates.iter().flatten();
        if candidates.clone().any(|c| c.as_str() == name) {
            return true;
        }
        !candidates.any(|c| c.eq_ignore_ascii_case(name))
    }

    /// Find `alias` (lower-cased) in the nearest `.luaurc` at or above
    /// `dir`, and the instance path its value names.
    fn alias_target(&self, dir: &Path, alias: &str, depth: usize) -> AliasTarget {
        if depth > ALIAS_CHAIN_DEPTH {
            return AliasTarget::Undefined;
        }
        for config_dir in dir.ancestors() {
            let Some(aliases) = self.luaurc_aliases(config_dir) else {
                continue;
            };
            let Some(value) = aliases.get(alias) else {
                continue;
            };
            let value = value.replace('\\', "/");
            if value.len() > MAX_PATH_TEXT {
                return AliasTarget::NotOnDisk;
            }
            if let Some(chained) = value.strip_prefix('@') {
                let (next, rest) = chained.split_once('/').unwrap_or((chained, ""));
                return match self.alias_target(config_dir, &next.to_ascii_lowercase(), depth + 1) {
                    AliasTarget::At(mut base) => {
                        for part in parts(rest) {
                            if part == ".." {
                                base.pop();
                            } else {
                                base = child(base, part);
                            }
                        }
                        self.on_disk(base)
                    }
                    other => other,
                };
            }
            // `~` is the shell's, not Luau's or the filesystem's: such a
            // path points into a home folder Rime has no business reading.
            if value.starts_with('~') {
                return AliasTarget::NotOnDisk;
            }
            return self.on_disk(anchor(config_dir, &value));
        }
        AliasTarget::Undefined
    }

    /// An alias value as an instance path: a written-out module file
    /// (`lib/init.luau`, `lib.luau`) names its module. `NotOnDisk` when
    /// neither a folder nor a module is there under that spelling: a value
    /// that is right only when letter case is ignored names nothing on
    /// Linux, so it names nothing here.
    fn on_disk(&self, path: PathBuf) -> AliasTarget {
        let mut inst = path;
        let file_name = inst.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if let Some(stem) = file_name.strip_suffix(".luau").or_else(|| file_name.strip_suffix(".lua")) {
            if stem == "init" {
                inst.pop();
            } else if !stem.is_empty() {
                inst.set_file_name(stem);
            }
        }
        let is_module = match (inst.parent(), inst.file_name()) {
            (Some(parent), Some(name)) => {
                self.project.module_file(parent, &name.to_string_lossy()).is_some()
            }
            _ => false,
        };
        // Canonical when it can be, so everything walked from here is in
        // the same form as the project's own file listing.
        if is_module {
            return AliasTarget::At(inst.canonicalize().unwrap_or(inst));
        }
        match as_spelled(&inst) {
            Some(folder) if folder.is_dir() => AliasTarget::At(folder),
            _ => AliasTarget::NotOnDisk,
        }
    }

    /// The `aliases` of `dir/.luaurc`, keys lower-cased. `None` when there
    /// is no such file, it does not parse, or it defines no alias. Cached
    /// per folder: every string require in a file walks the same folders.
    fn luaurc_aliases(&self, dir: &Path) -> Option<Arc<HashMap<String, String>>> {
        let cached = self.luaurc.read().unwrap().get(dir).cloned();
        if let Some(cached) = cached {
            return cached;
        }
        let aliases = (|| {
            let text = std::fs::read_to_string(dir.join(".luaurc")).ok()?;
            let value = crate::jsonc::parse(&text).ok()?;
            let table = value.get("aliases")?.as_object()?;
            let map: HashMap<String, String> = table
                .iter()
                .filter_map(|(k, v)| {
                    // Luau writes the key bare; `"@name"` is seen in the
                    // wild and can only mean the same thing.
                    let key = k.trim_start_matches('@').to_ascii_lowercase();
                    Some((key, v.as_str()?.to_string()))
                })
                .collect();
            (!map.is_empty()).then(|| Arc::new(map))
        })();
        self.luaurc.write().unwrap().insert(dir.to_path_buf(), aliases.clone());
        aliases
    }

    /// `Resolver::note` for a string target.
    pub(crate) fn string_note(&self, from: &Path, target: &rime_hir::Expr) -> Option<RequireNote> {
        let text = Self::string_target(target)?;
        self.resolve_string(from, &text).1
    }
}

/// The parts of a path that mean something: empty parts and `.` are
/// skipped, as Luau's navigator does. `..` stays.
fn parts(path: &str) -> Vec<&str> {
    path.split('/').filter(|p| !p.is_empty() && *p != ".").collect()
}

/// Whether `file` is an `init` file (`init.luau`, `init.server.lua`, ...).
fn is_init(file: &Path) -> bool {
    file.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .is_some_and(|n| n.split('.').next() == Some("init"))
}

/// An alias value (separators already `/`) as a path: absolute as it
/// stands, anything else from `config_dir`. Parts are added one at a
/// time: a Windows verbatim path (`\\?\C:\...`, which is what a canonical
/// folder is) does not treat `/` as a separator or `..` as going up.
fn anchor(config_dir: &Path, value: &str) -> PathBuf {
    let native: String =
        value.chars().map(|c| if c == '/' { std::path::MAIN_SEPARATOR } else { c }).collect();
    if Path::new(&native).is_absolute() {
        return normalize(Path::new(&native));
    }
    let mut out = config_dir.to_path_buf();
    for part in value.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            name => out = child(out, name),
        }
    }
    out
}

/// `a/./b/../c` to `a/c`, on the path alone. No disk access, so a path
/// that does not exist normalizes too.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// What R0501 shows for a string past `MAX_PATH_TEXT`: how it starts, and
/// why nothing was looked for. Same backtick shape as `describe`.
fn too_long(text: &str) -> String {
    let shown: String = text.chars().take(SHOWN_OF_TOO_LONG).collect();
    format!("{shown}…` → `no path is {} characters long", text.chars().count())
}

/// What R0501 shows for a string that resolved to nothing: the string as
/// written, then where that is. The label wraps the whole in backticks, so
/// the middle closes and reopens them.
fn describe(resolver: &RojoResolver, text: &str, loc: &Loc) -> String {
    let place = match loc {
        Loc::Dm(segs) => crate::dm_expression(segs),
        Loc::Fs(inst) => {
            let shown = inst.strip_prefix(&resolver.project.root).unwrap_or(inst);
            let shown = shown.to_string_lossy();
            shown.strip_prefix(r"\\?\").unwrap_or(&shown).replace('\\', "/")
        }
    };
    format!("{text}` → `{place}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RojoProject;

    fn root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/string_requires")
            .canonicalize()
            .unwrap()
    }

    fn resolver() -> RojoResolver {
        RojoResolver::new(RojoProject::load(&root()).unwrap())
    }

    /// A requiring file. It does not have to exist: only its path matters.
    fn from(rel: &str) -> PathBuf {
        root().join(rel)
    }

    #[track_caller]
    fn found(r: &RojoResolver, file: &str, text: &str, expected: &str) -> Option<RequireNote> {
        let (resolution, note) = r.resolve_string(&from(file), text);
        match resolution {
            Resolution::Found(f) => assert_eq!(f, root().join(expected), "{text} from {file}"),
            other => panic!("{text} from {file}: expected {expected}, got {other:?}"),
        }
        note
    }

    #[track_caller]
    fn missing(r: &RojoResolver, file: &str, text: &str) -> String {
        match r.resolve_string(&from(file), text).0 {
            Resolution::Missing { described } => described,
            other => panic!("{text} from {file}: expected Missing, got {other:?}"),
        }
    }

    #[track_caller]
    fn unfollowed(r: &RojoResolver, file: &str, text: &str) -> RequireNote {
        match r.resolve_string(&from(file), text).0 {
            Resolution::Unfollowed(note) => note,
            other => panic!("{text} from {file}: expected Unfollowed, got {other:?}"),
        }
    }

    #[test]
    fn relative_paths_start_at_the_requirers_parent() {
        let r = resolver();
        assert_eq!(found(&r, "src/shared/beta.luau", "./alpha", "src/shared/alpha.luau"), None);
        assert_eq!(found(&r, "src/shared/util/helper.luau", "../alpha", "src/shared/alpha.luau"), None);
        assert_eq!(found(&r, "src/shared/util/helper.luau", "./strings", "src/shared/util/strings.luau"), None);
        // A folder with an init file is a module.
        assert_eq!(found(&r, "src/shared/alpha.luau", "./util", "src/shared/util/init.luau"), None);
        // `.` and empty parts are skipped; `..` anywhere goes up.
        assert_eq!(found(&r, "src/shared/alpha.luau", "./util/./../util//helper", "src/shared/util/helper.luau"), None);
        // Across mounts: the walk is over folders, not the DataModel.
        assert_eq!(found(&r, "src/server/main.server.luau", "../shared/alpha", "src/shared/alpha.luau"), None);
    }

    #[test]
    fn an_init_file_is_its_folder() {
        let r = resolver();
        // Luau: `util/init.luau` is the module `util`; `./alpha` is beside
        // the `util` folder, `@self/helper` inside it.
        assert_eq!(found(&r, "src/shared/util/init.luau", "./alpha", "src/shared/alpha.luau"), None);
        assert_eq!(found(&r, "src/shared/util/init.luau", "@self/helper", "src/shared/util/helper.luau"), None);
        assert_eq!(found(&r, "src/shared/util/init.luau", "../server/job", "src/server/job.luau"), None);
    }

    #[test]
    fn an_init_file_written_for_the_older_rule_is_followed_and_flagged() {
        let r = resolver();
        // Nothing named `piece` is beside the `legacy` folder; one is
        // inside it, which is where darklua and Luau before `@self` look.
        assert_eq!(
            found(&r, "src/shared/legacy/init.luau", "./piece", "src/shared/legacy/piece.luau"),
            Some(RequireNote::InitRelative)
        );
        // The same string from a plain file needs no remark: both rules
        // agree there.
        assert_eq!(found(&r, "src/shared/legacy/other.luau", "./piece", "src/shared/legacy/piece.luau"), None);
    }

    #[test]
    fn an_init_require_both_rules_resolve_differently_is_not_guessed() {
        let r = resolver();
        // `./alpha` from `twice/init.luau`: Luau says `shared/alpha.luau`
        // (beside the folder), the older rule says `twice/alpha.luau`
        // (inside it). Both exist.
        assert_eq!(unfollowed(&r, "src/shared/twice/init.luau", "./alpha"), RequireNote::InitAmbiguous);
        // `@self` has one reading.
        assert_eq!(found(&r, "src/shared/twice/init.luau", "@self/alpha", "src/shared/twice/alpha.luau"), None);
        // A plain file in the same folder has one reading too.
        assert_eq!(found(&r, "src/shared/twice/other.luau", "./alpha", "src/shared/twice/alpha.luau"), None);
    }

    #[test]
    fn self_is_the_requiring_module() {
        let r = resolver();
        assert_eq!(found(&r, "src/shared/util/init.luau", "@self/strings", "src/shared/util/strings.luau"), None);
        // A plain file has no children.
        missing(&r, "src/shared/alpha.luau", "@self/helper");
    }

    #[test]
    fn game_is_the_datamodel_root() {
        let r = resolver();
        assert_eq!(
            found(&r, "src/server/job.luau", "@game/ReplicatedStorage/Shared/alpha", "src/shared/alpha.luau"),
            None
        );
        assert!(matches!(
            r.resolve_string(&from("src/server/job.luau"), "@game/ReplicatedStorage/Shared/nope").0,
            Resolution::Missing { .. }
        ));
        assert!(matches!(
            r.resolve_string(&from("src/server/job.luau"), "@game/Workspace/Thing").0,
            Resolution::OutsideView
        ));
    }

    #[test]
    fn aliases_come_from_the_nearest_luaurc() {
        let r = resolver();
        assert_eq!(found(&r, "src/server/job.luau", "@Shared/alpha", "src/shared/alpha.luau"), None);
        // Alias names ignore case.
        assert_eq!(found(&r, "src/server/job.luau", "@SHARED/util/helper", "src/shared/util/helper.luau"), None);
        // The alias alone is the module it names.
        assert_eq!(found(&r, "src/server/job.luau", "@Util", "src/shared/util/init.luau"), None);
        // An alias defined through another alias.
        assert_eq!(found(&r, "src/server/job.luau", "@Chain/helper", "src/shared/util/helper.luau"), None);
        // An alias whose value is a module file, extension written out.
        assert_eq!(found(&r, "src/server/job.luau", "@File", "src/shared/alpha.luau"), None);
        // A nearer `.luaurc` wins for the files under it, and only for the
        // names it defines.
        assert_eq!(found(&r, "src/server/nested/task.luau", "@Shared/helper", "src/shared/util/helper.luau"), None);
        assert_eq!(found(&r, "src/server/nested/task.luau", "@Util/strings", "src/shared/util/strings.luau"), None);
    }

    #[test]
    fn a_missing_module_is_missing_and_says_where_it_looked() {
        let r = resolver();
        assert_eq!(missing(&r, "src/shared/alpha.luau", "./nope"), "./nope` → `src/shared/nope");
        assert_eq!(missing(&r, "src/server/job.luau", "@Shared/nope"), "@Shared/nope` → `src/shared/nope");
        assert_eq!(missing(&r, "src/shared/util/init.luau", "@self/nope"), "@self/nope` → `src/shared/util/nope");
    }

    #[test]
    fn what_luau_gives_no_meaning_is_unfollowed_not_guessed() {
        let r = resolver();
        assert_eq!(unfollowed(&r, "src/shared/alpha.luau", "alpha"), RequireNote::BareString);
        assert_eq!(unfollowed(&r, "src/shared/alpha.luau", "shared/alpha"), RequireNote::BareString);
        assert_eq!(unfollowed(&r, "src/shared/alpha.luau", ""), RequireNote::BareString);
        assert_eq!(
            unfollowed(&r, "src/shared/alpha.luau", "@nowhere/thing"),
            RequireNote::UnknownAlias("nowhere".into())
        );
        // Defined, and its folder is not there (packages not installed).
        assert_eq!(
            unfollowed(&r, "src/shared/alpha.luau", "@Gone/thing"),
            RequireNote::AliasNotOnDisk("gone".into())
        );
        // A home-folder path is never read.
        assert_eq!(
            unfollowed(&r, "src/shared/alpha.luau", "@Home/thing"),
            RequireNote::AliasNotOnDisk("home".into())
        );
        // An alias chain that never ends.
        assert_eq!(
            unfollowed(&r, "src/shared/alpha.luau", "@Loop/thing"),
            RequireNote::UnknownAlias("loop".into())
        );
    }

    #[test]
    fn lenient_forms_other_tools_accept() {
        let r = resolver();
        // `..` and `.` alone.
        assert_eq!(found(&r, "src/shared/util/helper.luau", ".", "src/shared/util/init.luau"), None);
        assert_eq!(found(&r, "src/shared/util/sub/deep.luau", "..", "src/shared/util/init.luau"), None);
        // A written-out extension.
        assert_eq!(found(&r, "src/shared/beta.luau", "./alpha.luau", "src/shared/alpha.luau"), None);
        assert_eq!(found(&r, "src/server/job.luau", "@Shared/alpha.luau", "src/shared/alpha.luau"), None);
        // Backslashes are separators.
        assert_eq!(found(&r, "src/shared/alpha.luau", r".\util\helper", "src/shared/util/helper.luau"), None);
    }

    #[test]
    fn a_name_that_matches_only_by_case_is_a_miss() {
        // Windows and macOS would open `alpha.luau` for `./Alpha`; Roblox
        // and Luau on Linux would not.
        let r = resolver();
        missing(&r, "src/shared/beta.luau", "./Alpha");
        missing(&r, "src/server/job.luau", "@Shared/ALPHA");
        missing(&r, "src/shared/legacy/init.luau", "./Piece");
    }

    #[test]
    fn a_part_named_parent_is_a_name() {
        // In an instance path `Parent` goes up. In a string it is a child.
        let r = resolver();
        assert_eq!(found(&r, "src/shared/alpha.luau", "./Parent/inside", "src/shared/Parent/inside.luau"), None);
    }

    /// The whole fixture through a `Session`, the way the CLI runs it: the
    /// strings in `consumer.luau` resolve, cross-file checks reach through
    /// them, and what is not followed produces notes and no diagnostics.
    #[test]
    fn a_session_follows_strings_and_notes_what_it_does_not() {
        let project = RojoProject::load(&root()).unwrap();
        let files: Vec<(PathBuf, String)> = project
            .mount_files()
            .into_iter()
            .map(|p| {
                let src = std::fs::read_to_string(&p).unwrap();
                (p, src)
            })
            .collect();
        let session = rime_check::session::Session::new(
            files,
            Some(Box::new(RojoResolver::new(project))),
            rime_std::StdNames::bundled().clone(),
            None,
        );
        let consumer = root().join("src/server/consumer.luau");
        let (diags, _) = session.report(&consumer).expect("consumer is mounted");
        let codes: Vec<String> = diags.iter().map(|d| d.code.to_string()).collect();
        assert_eq!(codes, vec!["R0301".to_string()], "{diags:#?}");
        let notes: Vec<&RequireNote> = session.require_notes(&consumer).iter().map(|(_, n)| n).collect();
        assert_eq!(
            notes,
            vec![&RequireNote::BareString, &RequireNote::UnknownAlias("nowhere".into())],
            "source order, one per call"
        );
        let legacy = root().join("src/shared/legacy/init.luau");
        let notes: Vec<&RequireNote> = session.require_notes(&legacy).iter().map(|(_, n)| n).collect();
        assert_eq!(notes, vec![&RequireNote::InitRelative]);
        assert!(session.report(&legacy).unwrap().0.iter().all(|d| !d.code.to_string().starts_with("R05")));
    }
}
