//! rime-roblox: Rojo project resolution — DataModel require paths to files.
//!
//! Implements `rime_check::project::Resolver` for the require styles that
//! dominate real code:
//!
//!   local RS = game:GetService("ReplicatedStorage")
//!   local Modules = RS.Modules                      -- alias chains
//!   require(RS.Modules.X)                           -- the common case
//!   require(RS:WaitForChild("Packages").X)          -- WaitForChild steps
//!   require(script.Parent.X)                        -- file-relative
//!
//! The DataModel side comes from a Rojo project file ($path mounts, possibly
//! nested); the file side follows Rojo naming: `X.luau`, `X.lua`,
//! `X/init.luau`, `X/init.lua`. Misses are classified (see `Resolution`):
//! provably-absent files are `Missing` (R0501), uninterpretable shapes are
//! `Dynamic` (R0502), and static paths outside the mounts are `OutsideView` —
//! silence, never a guess.
//!
//! Requires written as strings (`require("./x")`, `require("@alias/x")`)
//! are in `strings.rs`.
//!
//! Which file is the project: Rojo calls any `*.project.json` (or
//! `.project.jsonc`) a project file, and looks for `default.project.json`
//! only when it is handed a folder. `find_project_file` is the search the
//! CLI and the language server share; a folder INSIDE a project's tree
//! nests through its `default.project.json`, which is Rojo's own rule.

mod jsonc;
mod strings;

use rime_check::project::{CtxSet, Loc, RequireNote, Resolution};
use rime_hir as hir;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// A Rojo project: DataModel-path mounts onto the filesystem.
pub struct RojoProject {
    /// (DataModel path segments, filesystem base), longest DM prefix wins.
    mounts: Vec<(Vec<String>, PathBuf)>,
    /// Tree nodes declared WITHOUT `$path` — virtual containers whose
    /// children the project does not own (plugin-provided folders, runtime
    /// assemblies). A miss under one of these deeper than any matched mount
    /// is unverifiable, not provably missing.
    virtual_nodes: Vec<Vec<String>>,
    /// Where the project file lives — file paths in diagnostics render
    /// relative to it.
    root: PathBuf,
    /// Source files per mount (index-aligned with `mounts`), walked once
    /// on first use: `mount_files`, the module-name index, and the
    /// `module_file` fast path all need the same listing.
    walked: OnceLock<Vec<Vec<PathBuf>>>,
    /// The same listing as a set — `module_file` answers from it instead
    /// of probing the filesystem for every candidate.
    known: OnceLock<HashSet<PathBuf>>,
}

impl RojoProject {
    /// Parse `<root>/default.project.json`. Errors only on unreadable or
    /// invalid JSON — an empty tree is a valid (useless) project.
    pub fn load(project_root: &Path) -> anyhow::Result<RojoProject> {
        Self::load_file(&project_root.join(DEFAULT_PROJECT_FILE))
    }

    /// Parse the project file at `file`, whatever it is called. `$path`
    /// entries are relative to the folder it is in. Same errors as `load`.
    pub fn load_file(file: &Path) -> anyhow::Result<RojoProject> {
        let text = std::fs::read_to_string(file)?;
        let value = jsonc::parse(&text)?;
        let project_root = match file.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => dir,
            _ => Path::new("."),
        };
        let mut mounts = Vec::new();
        let mut virtual_nodes = Vec::new();
        if let Some(tree) = value.get("tree") {
            let mut nesting = Nesting {
                chain: vec![file.canonicalize().unwrap_or_else(|_| file.to_path_buf())],
                read: 0,
            };
            collect_mounts(tree, &mut Vec::new(), project_root, &mut mounts, &mut virtual_nodes, &mut nesting);
        }
        // Longest DataModel prefix first, so nested mounts (Packages under
        // ReplicatedStorage) shadow their parent.
        mounts.sort_by_key(|(dm, _)| std::cmp::Reverse(dm.len()));
        let root = project_root
            .canonicalize()
            .unwrap_or_else(|_| project_root.to_path_buf());
        Ok(RojoProject {
            mounts,
            virtual_nodes,
            root,
            walked: OnceLock::new(),
            known: OnceLock::new(),
        })
    }

    /// The filesystem base of every mount, canonical. A walk that leaves
    /// directories out (the CLI skips what git ignores) must still list
    /// everything under these: `seed_walk` takes its listing as complete.
    pub fn mount_bases(&self) -> impl Iterator<Item = &Path> {
        self.mounts.iter().map(|(_, base)| base.as_path())
    }

    /// The per-mount source listing, walked once. Mount bases are canonical
    /// (see `collect_mounts`), so every path here is in canonical form
    /// without a per-file syscall — the same form `module_file` returns
    /// and the CLI's requested set is built in.
    ///
    /// Uses `walk::sources_serial`, not the parallel `walk::sources`: this
    /// `OnceLock::get_or_init` closure can run on a rayon worker thread
    /// while `resolve_in_package` holds `RojoResolver::nested`'s lock (a
    /// nested/vendored package's `RojoProject` is loaded lazily, on first
    /// resolve, from inside the per-file parallel pass) — rayon work in
    /// that position can deadlock (a worker parked in `collect()` steals
    /// other queued jobs onto itself, and a stolen job that re-enters the
    /// same lock or this same `OnceLock` never returns). `seed_walk` below
    /// is always called from `Workspace::load` on the caller's thread,
    /// outside rayon and outside any lock, so it keeps the parallel walker.
    fn mount_walk(&self) -> &[Vec<PathBuf>] {
        self.walked.get_or_init(|| {
            self.mounts
                .iter()
                .map(|(_, base)| rime_check::walk::sources_serial(base))
                .collect()
        })
    }

    /// Seed the mount listing from a walk the caller already did. The CLI
    /// walks its roots before it knows there is a project; a mount inside
    /// one of those (canonical) roots is filtered out of `files` instead
    /// of being walked a second time. Mounts outside every root still
    /// walk. Same listing either way — same walker, same skip rules, and a
    /// root's pre-order restricted to a subtree IS that subtree's
    /// pre-order. No-op once the listing exists.
    ///
    /// Callers MUST call this from outside rayon (the caller's own thread,
    /// not a worker) — it is what lets `mount_walk`'s lazy fallback stay
    /// serial: a project that got seeded never runs the `get_or_init`
    /// closure at all, and one that did not is by definition being resolved
    /// lazily, on a worker, where only the serial walker is safe.
    pub fn seed_walk(&self, roots: &[PathBuf], files: &[PathBuf]) {
        self.walked.get_or_init(|| {
            self.mounts
                .iter()
                .map(|(_, base)| {
                    let covered = roots.iter().any(|r| r.is_dir() && base.starts_with(r));
                    if covered {
                        files.iter().filter(|f| under(f, base)).cloned().collect()
                    } else {
                        rime_check::walk::sources(base)
                    }
                })
                .collect()
        });
    }

    fn known_files(&self) -> &HashSet<PathBuf> {
        self.known
            .get_or_init(|| self.mount_walk().iter().flatten().cloned().collect())
    }

    /// Rojo module naming: `X.luau`, `X.lua`, `X/init.luau`, `X/init.lua`,
    /// first hit wins. Answered from the mount listing when the candidate
    /// is in it — `dir` is built on a canonical mount base, so a file that
    /// exists under a mount is in the set under exactly this spelling —
    /// and only otherwise probed on disk (a genuinely missing module, or
    /// one outside every mount), canonicalized so the result keys into
    /// `check_project`'s interface map. Every distinct target used to cost
    /// up to four stats and a canonicalize; on Windows those do not scale
    /// across workers.
    ///
    /// Both ways answer for the path AS SPELLED, every part of it: the
    /// listing is compared to the letter, and a disk hit is taken only
    /// through `as_spelled`. Roblox and a Linux disk tell `Util` from
    /// `util`; a Windows or macOS disk does not, and without this the
    /// same project would check differently on the two.
    fn module_file(&self, dir: &Path, name: &str) -> Option<PathBuf> {
        let candidates = [
            dir.join(format!("{name}.luau")),
            dir.join(format!("{name}.lua")),
            dir.join(name).join("init.luau"),
            dir.join(name).join("init.lua"),
        ];
        let known = self.known_files();
        if let Some(hit) = candidates.iter().find(|c| known.contains(*c)) {
            return Some(hit.clone());
        }
        candidates
            .into_iter()
            .filter(|c| c.is_file())
            .find_map(|c| as_spelled(&c))
    }

    /// Every source file under any mount, in canonical form — what the CLI
    /// feeds to `check_project` so interfaces exist project-wide. Sorted and
    /// deduplicated (mounts may nest), so the order is the CLI's output order.
    pub fn mount_files(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = self.mount_walk().iter().flatten().cloned().collect();
        out.sort();
        out.dedup();
        out
    }

    /// Whether the project provably creates the instance at this DataModel
    /// path — the `Resolver::present` question (see there). Four ways to
    /// prove it, none of which `dm_to_file` answers:
    ///
    ///   - the path IS a mount point: whatever the declared `$path` names,
    ///     Rojo builds an instance there, so plain existence is proof;
    ///   - the path is UNDER a mount and names a DIRECTORY, which Rojo
    ///     syncs as a Folder (`ServerStorage.ServerPackages._Index`);
    ///   - the path is an ANCESTOR of a mount, so Rojo builds the
    ///     containers on the way down to it (`ServerStorage` itself, given
    ///     a `ServerStorage.ServerPackages` mount);
    ///   - the path is at or above a node the tree declares without a
    ///     `$path` — a virtual container Rojo still creates.
    ///
    /// `is_dir`, not `exists`, on the under-a-mount arm, and that is the
    /// whole point: a wally package directory is full of files Rojo does
    /// NOT sync (`wally.toml`, `LICENSE`, `README.md`), and proving those
    /// present would be proving an instance that never exists. Files that
    /// ARE instances are module files, which is `resolve_loc`'s question,
    /// asked first by the one caller (`eval::non_nil`) — so this arm never
    /// needs to answer for them.
    ///
    /// Known and deliberate: the `is_dir`/`exists` probes are the OS's, so
    /// on Windows they are case-insensitive while Roblox is not.
    /// `module_file` answers for the path as spelled (`as_spelled`); this
    /// proof of a folder does not yet.
    ///
    /// `false` is "no proof", never "absent" (`Resolver::present`'s
    /// contract): a runtime-parented child lands here too.
    fn dm_present(&self, dm: &[String]) -> bool {
        if dm.is_empty() {
            return true; // `game`
        }
        for (mount, base) in &self.mounts {
            if dm.len() >= mount.len() && dm[..mount.len()] == mount[..] {
                let rest = &dm[mount.len()..];
                if rest.is_empty() {
                    if base.exists() {
                        return true;
                    }
                } else {
                    let mut p = base.clone();
                    for seg in rest {
                        p = child(p, seg);
                    }
                    if p.is_dir() {
                        return true;
                    }
                }
            }
            if mount.len() > dm.len() && mount[..dm.len()] == dm[..] {
                return true;
            }
        }
        self.virtual_nodes
            .iter()
            .any(|n| n.len() >= dm.len() && n[..dm.len()] == dm[..])
    }

    /// A DataModel path (service, then children) to a module file, with the
    /// miss classified: `Missing` only when at least one mount covered the
    /// path (the project claims that subtree and the file is not there),
    /// `OutsideView` when no mount does (place-managed instances Rime
    /// cannot see — silence, never a guess).
    fn dm_to_file(&self, dm: &[String]) -> Resolution {
        // `game.Players.LocalPlayer.PlayerScripts.<rest>` — the engine
        // copies StarterPlayerScripts into each player's PlayerScripts at
        // join, so a static path through the runtime copy resolves against
        // the StarterPlayer.StarterPlayerScripts mount. A miss stays
        // OutsideView, never Missing: the engine injects its own defaults
        // (an un-forked PlayerModule) beside the copied files.
        if dm.len() > 3
            && dm[0] == "Players"
            && dm[1] == "LocalPlayer"
            && dm[2] == "PlayerScripts"
        {
            let mut rewritten =
                vec!["StarterPlayer".to_string(), "StarterPlayerScripts".to_string()];
            rewritten.extend_from_slice(&dm[3..]);
            return match self.dm_to_file(&rewritten) {
                f @ Resolution::Found(_) => f,
                _ => Resolution::OutsideView,
            };
        }
        let mut deepest_mount = 0usize;
        let mut covered = false;
        for (mount, base) in &self.mounts {
            if dm.len() >= mount.len() && dm[..mount.len()] == mount[..] {
                covered = true;
                deepest_mount = deepest_mount.max(mount.len());
                let rest = &dm[mount.len()..];
                if rest.is_empty() {
                    // The mount point itself. A module when `$path` names a
                    // source file or an init-carrying directory (nested
                    // projects splice to their tree root, so vendored
                    // packages land here); otherwise a service or folder.
                    if base.is_file()
                        && base
                            .extension()
                            .is_some_and(|x| x == "luau" || x == "lua")
                    {
                        if let Ok(f) = base.canonicalize() {
                            return Resolution::Found(f);
                        }
                    }
                    for init in ["init.luau", "init.lua"] {
                        let c = base.join(init);
                        if c.is_file() {
                            if let Ok(f) = c.canonicalize() {
                                return Resolution::Found(f);
                            }
                        }
                    }
                    continue;
                }
                let mut dir = base.clone();
                for seg in &rest[..rest.len() - 1] {
                    dir = child(dir, seg);
                }
                if let Some(f) = self.module_file(&dir, &rest[rest.len() - 1]) {
                    return Resolution::Found(f);
                }
                // Wrong mount (e.g. Packages shadowing ReplicatedStorage):
                // keep trying shorter prefixes.
            }
        }
        if !covered {
            return Resolution::OutsideView;
        }
        // A virtual node (declared, no $path) prefixing the target at least
        // as deeply as any mount means the project does not own the subtree
        // the target lives in — a plugin-provided folder, a runtime
        // assembly. Unverifiable, not provably missing.
        let virtual_depth = self
            .virtual_nodes
            .iter()
            .filter(|n| n.len() < dm.len() && dm[..n.len()] == n[..])
            .map(|n| n.len())
            .max()
            .unwrap_or(0);
        if virtual_depth >= deepest_mount {
            Resolution::OutsideView
        } else {
            Resolution::Missing { described: format!("game.{}", dm.join(".")) }
        }
    }
}

/// `dir/name`, by appending to the text of `dir`.
///
/// `PathBuf::push` and `join` do more than append: on a Windows `\\?\`
/// path, which is what a canonical path is, each one reads the whole path
/// again and rebuilds it, so building a path one part at a time costs the
/// square of the number of parts. A require string decides how many parts
/// there are, so a walk over its parts appends instead.
///
/// It also keeps a name a name: `push` treats an absolute `name` as a new
/// path and a `..` as a step up. Here both are a child called that, which
/// nothing on disk is.
pub(crate) fn child(dir: PathBuf, name: &str) -> PathBuf {
    let mut text = dir.into_os_string();
    let ends_in_separator = text
        .as_encoded_bytes()
        .last()
        .is_some_and(|b| std::path::is_separator(*b as char));
    if !text.is_empty() && !ends_in_separator {
        text.push(std::path::MAIN_SEPARATOR_STR);
    }
    text.push(name);
    PathBuf::from(text)
}

/// The canonical form of `path`, when `path` is on disk under the spelling
/// it was asked for.
///
/// A Windows or macOS disk opens `Util/helper.luau` for `util/helper.luau`.
/// The canonical path carries the names as the disk has them, so a path
/// that differs from its canonical form ONLY in letter case, in any part,
/// was found by a spelling Roblox and a Linux disk would not find: `None`,
/// the same as for a path that is not there. On Linux such a path is not
/// there in the first place, so the answer is the same on every system.
///
/// A path that differs by more than case (a symlink or junction resolved,
/// a short `8.3` name) is some other way of naming the file, not a
/// misspelling, and is taken.
pub(crate) fn as_spelled(path: &Path) -> Option<PathBuf> {
    let canonical = path.canonicalize().ok()?;
    let (asked, found) = (path_text(path), path_text(&canonical));
    if asked != found && asked.to_lowercase() == found.to_lowercase() {
        return None;
    }
    Some(canonical)
}

/// A path as the text `as_spelled` compares: without the `\\?\` a
/// canonical Windows path starts with, with one kind of separator, and
/// with the drive letter in capitals (a drive letter is not a name on
/// disk; `c:` and `C:` are one drive on every system that has them).
fn path_text(path: &Path) -> String {
    let text = path.to_string_lossy();
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
    let mut out: String = text
        .chars()
        .map(|c| if std::path::is_separator(c) { std::path::MAIN_SEPARATOR } else { c })
        .collect();
    if out.as_bytes().get(1) == Some(&b':') {
        out[..1].make_ascii_uppercase();
    }
    out
}

/// `path` is strictly inside directory `base`: same bytes up to `base`'s
/// length, then a separator. Both come from the same canonical walk, so a
/// byte prefix is exact — and far cheaper than `Path::starts_with`, which
/// re-parses the `\\?\` prefix and every component on each call.
fn under(path: &Path, base: &Path) -> bool {
    let (p, b) = (path.as_os_str().as_encoded_bytes(), base.as_os_str().as_encoded_bytes());
    p.len() > b.len() && p.starts_with(b) && std::path::is_separator(p[b.len()] as char)
}

/// Nested projects inside nested projects are legal (a vendored package can
/// vendor its own deps); the cap bounds how deep that goes.
const NESTED_PROJECT_DEPTH: usize = 8;

/// How many nested projects one load reads in all. Nesting multiplies: a
/// project mounted from four nodes, itself mounting another from four, is
/// sixteen expansions two levels down and 65,536 at the depth cap, with no
/// cycle anywhere. A real tree nests a handful; past this many a `$path`
/// is a plain mount.
const NESTED_PROJECT_LIMIT: usize = 1024;

/// Where `collect_mounts` is in the nesting of project files.
struct Nesting {
    /// The project files being expanded, outermost first, canonical. A
    /// `$path` that names one of them (the project mounting itself, or a
    /// nested project mounting one above it) is not expanded again: there
    /// is no end to that, and what Rojo builds from it is not a tree
    /// anything can be required through. It is a plain mount.
    chain: Vec<PathBuf>,
    /// Nested projects read so far, against `NESTED_PROJECT_LIMIT`.
    read: usize,
}

fn collect_mounts(
    node: &serde_json::Value,
    dm: &mut Vec<String>,
    project_root: &Path,
    out: &mut Vec<(Vec<String>, PathBuf)>,
    virtual_nodes: &mut Vec<Vec<String>>,
    nesting: &mut Nesting,
) {
    let Some(obj) = node.as_object() else { return };
    // `"$path": "src"`, or Rojo's `"$path": { "optional": "src" }` for a
    // path that may not be there. A missing optional path mounts nothing,
    // exactly like a missing required one does here.
    let path = obj
        .get("$path")
        .and_then(|v| v.as_str().or_else(|| v.get("optional").and_then(|o| o.as_str())));
    if let Some(p) = path {
        let base = project_root.join(p);
        // A `$path` to a directory that is itself a Rojo project (a vendored
        // package), or straight to a project FILE: Rojo builds the NESTED
        // project's tree at this mount point. Splice its mounts in under
        // our DataModel prefix so the mount table matches the real
        // DataModel; unparseable nested projects fall back to the raw
        // directory mount.
        match nested_project(&base, nesting) {
            Some((tree, nested_root, file)) => {
                nesting.chain.push(file);
                collect_mounts(&tree, dm, &nested_root, out, virtual_nodes, nesting);
                nesting.chain.pop();
            }
            // One canonicalize per mount, so everything walked beneath it
            // (`mount_walk`) is already canonical: on-disk casing, `\\?\`
            // form, no `..`. A missing base is kept as anchored — it walks
            // to nothing and resolves nothing, same as before.
            None => {
                let base = base.canonicalize().unwrap_or(base);
                out.push((dm.clone(), base))
            }
        }
    } else if !dm.is_empty() {
        // Declared in the tree with no filesystem backing: a virtual
        // container. Its unlisted children come from somewhere the project
        // does not manage.
        virtual_nodes.push(dm.clone());
    }
    for (key, child) in obj {
        if key.starts_with('$') {
            continue;
        }
        dm.push(key.clone());
        collect_mounts(child, dm, project_root, out, virtual_nodes, nesting);
        dm.pop();
    }
}

/// The `tree` of `dir/default.project.json`, when `dir` is a directory
/// carrying one. `None` on anything else, unreadable or invalid JSON
/// included.
fn nested_project_tree(dir: &Path) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(dir.join(DEFAULT_PROJECT_FILE)).ok()?;
    let value = jsonc::parse(&text).ok()?;
    value.get("tree").cloned()
}

/// The nested project a `$path` names: its `tree`, the folder its own
/// `$path` entries are relative to, and its file in canonical form. Either
/// `base` is a project file (`"$path": "kit/kit.project.json"`), or it is
/// a folder with a `default.project.json`. `None` means "treat as a plain
/// mount": anything else, unreadable or invalid JSON, a project already
/// being expanded (see `Nesting::chain`), and nesting past the depth cap
/// or the read limit.
fn nested_project(base: &Path, nesting: &mut Nesting) -> Option<(serde_json::Value, PathBuf, PathBuf)> {
    // The chain holds the outermost project too, so its length is the
    // depth plus one.
    if nesting.chain.len() > NESTED_PROJECT_DEPTH || nesting.read >= NESTED_PROJECT_LIMIT {
        return None;
    }
    let named_file = base
        .file_name()
        .is_some_and(|n| is_project_file_name(&n.to_string_lossy()));
    let (file, root) = if named_file && base.is_file() {
        (base.to_path_buf(), base.parent()?.to_path_buf())
    } else {
        (base.join(DEFAULT_PROJECT_FILE), base.to_path_buf())
    };
    let text = std::fs::read_to_string(&file).ok()?;
    let file = file.canonicalize().ok()?;
    if nesting.chain.contains(&file) {
        return None;
    }
    let tree = jsonc::parse(&text).ok()?.get("tree").cloned()?;
    nesting.read += 1;
    Some((tree, root, file))
}

/// The name Rojo looks for when it is handed a folder.
pub const DEFAULT_PROJECT_FILE: &str = "default.project.json";

/// Whether `name` is a Rojo project file name: `*.project.json`, or the
/// `.jsonc` spelling newer Rojo also takes.
pub fn is_project_file_name(name: &str) -> bool {
    name.ends_with(".project.json") || name.ends_with(".project.jsonc")
}

/// What a search for a project file found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectSearch {
    /// The project file to use.
    Found(PathBuf),
    /// `dir` holds several project files and no `default.project.json`.
    /// Nothing says which one the code under it belongs to, so none is
    /// picked. `files` are their names, sorted.
    Several { dir: PathBuf, files: Vec<String> },
    /// No folder at or above the start holds a project file.
    None,
}

/// Find the project file for code under `start` (a folder): the nearest
/// folder at or above it that holds any project file. In that folder
/// `default.project.json` wins; a single file under another name is used;
/// several with no default is `ProjectSearch::Several`.
pub fn find_project_file(start: &Path) -> ProjectSearch {
    for dir in start.ancestors() {
        // The common case first, as one stat and no listing.
        let default = dir.join(DEFAULT_PROJECT_FILE);
        if default.is_file() {
            return ProjectSearch::Found(default);
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut files: Vec<String> = entries
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| !t.is_dir()))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| is_project_file_name(name))
            .collect();
        files.sort();
        // `default.project.jsonc` is a default too.
        if let Some(default) = files.iter().find(|f| f.as_str() == "default.project.jsonc") {
            return ProjectSearch::Found(dir.join(default));
        }
        match files.len() {
            0 => {}
            1 => return ProjectSearch::Found(dir.join(&files[0])),
            _ => return ProjectSearch::Several { dir: dir.to_path_buf(), files },
        }
    }
    ProjectSearch::None
}

pub struct RojoResolver {
    project: RojoProject,
    /// Nested package projects (wally `_Index` layouts: a package instance
    /// is a directory whose own `default.project.json` redirects into it,
    /// e.g. sift's `"tree": {"$path": "src"}`). Loaded lazily per directory;
    /// `None` caches a load failure. `Arc`, not a bare `RojoProject`: the
    /// lock must be DROPPED before `resolve_in_package` touches the
    /// project, because touching it can trigger that project's own lazy
    /// `mount_walk` — a `OnceLock` init that itself runs rayon work (see
    /// `mount_walk`'s doc). Holding this `Mutex` across that call would be
    /// a self-deadlock: this thread already owns the (non-reentrant) lock;
    /// while it sits parked inside the nested `collect()` waiting on its
    /// own spawned subtasks, rayon can schedule another *unrelated* queued
    /// job onto this same OS thread, and if that job also calls
    /// `resolve_in_package` and tries to lock `nested` again, the thread is
    /// now waiting on a lock only itself (further up its own call stack)
    /// could ever release — it never will. `Arc::clone` under the lock,
    /// then dereferencing the clone after the guard drops, removes the
    /// project access from the locked section entirely.
    nested: Mutex<HashMap<PathBuf, Option<Arc<RojoProject>>>>,
    /// DataModel-path patterns the user vouches for (`[requires] trust` in
    /// rime.toml): `game.A.B` exact, `*` one segment, trailing `**` any
    /// subtree. Consulted only AFTER real resolution misses — a trusted
    /// path that exists still resolves normally, interface and all.
    trusted: Vec<String>,
    /// Evaluated-target -> final Resolution. Everything past `eval` is a
    /// deterministic function of the Loc (probes, canonicalize, trust), and
    /// popular targets are required from hundreds of files — this cache is
    /// what keeps resolution off the filesystem after first touch.
    ///
    /// The three memo caches are `RwLock`s: after warm-up every worker in
    /// the parallel per-file pass reads them and almost none writes, and a
    /// `Mutex` serialized those reads.
    resolved: RwLock<HashMap<Loc, Resolution>>,
    /// `present(loc)` per Loc — the evaluator asks it for the same one or
    /// two container paths (`ServerStorage.ServerPackages`) from every file
    /// that walks a shared package's chain. Same reason the others are
    /// memoized: on Windows the `exists` stats do not scale across workers.
    presence: RwLock<HashMap<Loc, bool>>,
    /// `instance_of(from)` per source file — the init-file case reads and
    /// parses the package's default.project.json, and every require in a
    /// file re-asks for the same instance.
    instances: RwLock<HashMap<PathBuf, Option<PathBuf>>>,
    /// `project_root_path(dir)` per directory (files share parents, so this
    /// dedupes JSON reads across the instance cache too).
    pkg_roots: RwLock<HashMap<PathBuf, Option<PathBuf>>>,
    /// module name -> DataModel-path descriptions, for the broken-require
    /// "did the module move?" note. Built lazily on the first miss (one
    /// mount walk), never on the happy path.
    name_index: OnceLock<HashMap<String, Vec<String>>>,
    /// `.luaurc` aliases per folder (keys lower-cased); `None` caches "no
    /// file here, or none that defines an alias". See `strings.rs`.
    luaurc: RwLock<HashMap<PathBuf, Option<Arc<HashMap<String, String>>>>>,
    /// (requiring file, string as written) -> its resolution and remark.
    /// The checker asks about each require site several times over.
    strings: RwLock<HashMap<(PathBuf, String), (Resolution, Option<RequireNote>)>>,
}

impl RojoResolver {
    pub fn new(project: RojoProject) -> Self {
        Self {
            project,
            nested: Mutex::new(HashMap::new()),
            trusted: Vec::new(),
            resolved: RwLock::new(HashMap::new()),
            presence: RwLock::new(HashMap::new()),
            instances: RwLock::new(HashMap::new()),
            pkg_roots: RwLock::new(HashMap::new()),
            name_index: OnceLock::new(),
            luaurc: RwLock::new(HashMap::new()),
            strings: RwLock::new(HashMap::new()),
        }
    }

    /// The lazily-built module-name index behind `candidates`. Built from
    /// the project's cached mount listing (no walk of its own — the first
    /// miss used to walk every mount while 31 workers waited on the
    /// OnceLock): a file's module name is its stem (script-class suffix
    /// stripped); `init.*` names the directory, whose instance is the
    /// directory's DataModel path itself.
    fn build_name_index(&self) -> HashMap<String, Vec<String>> {
        let mut index: HashMap<String, Vec<String>> = HashMap::new();
        let walked = self.project.mount_walk();
        for ((dm, base), files) in self.project.mounts.iter().zip(walked) {
            for file in files {
                let Ok(rel) = file.strip_prefix(base) else {
                    continue;
                };
                // Wally internals: a package's canonical requireable name is
                // its `Packages/<Name>` linker (indexed normally), not the
                // versioned `_Index` path — suggesting the latter would
                // recommend a require nobody should write.
                if rel.components().any(|c| c.as_os_str() == "_Index") {
                    continue;
                }
                let Some(stem) = file.file_stem() else {
                    continue;
                };
                let stem = stem.to_string_lossy();
                let stem = stem
                    .strip_suffix(".client")
                    .or_else(|| stem.strip_suffix(".server"))
                    .unwrap_or(&stem);
                let mut segs = dm.clone();
                segs.extend(
                    rel.parent()
                        .into_iter()
                        .flat_map(|d| d.components())
                        .map(|c| c.as_os_str().to_string_lossy().into_owned()),
                );
                let name = if stem == "init" {
                    // The directory IS the instance; its dm path is segs.
                    match segs.last() {
                        Some(n) => n.clone(),
                        None => continue,
                    }
                } else {
                    segs.push(stem.to_string());
                    stem.to_string()
                };
                index
                    .entry(name)
                    .or_default()
                    .push(dm_expression(&segs));
            }
        }
        for v in index.values_mut() {
            v.sort();
            v.dedup();
        }
        index
    }

    pub fn with_trusted(mut self, patterns: Vec<String>) -> Self {
        self.trusted = patterns;
        self
    }

    fn is_trusted(&self, dm: &[String]) -> bool {
        self.trusted.iter().any(|p| trust_matches(p, dm))
    }

    /// Resolve a virtual instance path whose plain file mapping missed, by
    /// finding the nearest enclosing directory that carries its own
    /// `default.project.json` (a wally package) and mapping the remaining
    /// segments through that project's tree. Depth one: packages inside
    /// packages resolve through their own linkers anyway.
    fn nested_resolve(&self, inst: &Path) -> Option<PathBuf> {
        // Split into the deepest EXISTING ancestor + the virtual tail.
        // Nothing exists below a folder that does not, so the split is
        // found by halving: a handful of probes, where stepping up one
        // level at a time is one probe per part of the path. Existing as
        // spelled: a folder a Windows disk finds under another letter case
        // is not the instance the path names.
        let ancestors: Vec<&Path> = inst.ancestors().collect();
        let existing = ancestors.partition_point(|a| as_spelled(a).is_none());
        let mut base = ancestors.get(existing)?.to_path_buf();
        let mut segs: Vec<String> = ancestors[..existing]
            .iter()
            .rev()
            .map(|a| a.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect::<Option<_>>()?;
        // Ascend a few levels looking for a package project — the redirect
        // may sit above the deepest existing directory.
        for _ in 0..4 {
            if base.join("default.project.json").is_file() {
                if let Some(f) = self.resolve_in_package(&base, &segs) {
                    return Some(f);
                }
            }
            segs.insert(0, base.file_name()?.to_string_lossy().into_owned());
            if !base.pop() {
                return None;
            }
        }
        None
    }

    fn resolve_in_package(&self, pkg: &Path, segs: &[String]) -> Option<PathBuf> {
        // Lock scope is get-or-insert-and-clone-the-`Arc` ONLY: `project`
        // below is touched after the guard is dropped, never while it is
        // held — see the deadlock this avoids on the `nested` field's doc.
        let project = {
            let mut cache = self.nested.lock().unwrap();
            cache.entry(pkg.to_path_buf()).or_insert_with(|| RojoProject::load(pkg).ok().map(Arc::new)).clone()
        }?;
        for (mount, mbase) in &project.mounts {
            if segs.len() >= mount.len() && segs[..mount.len()] == mount[..] {
                let rest = &segs[mount.len()..];
                if rest.is_empty() {
                    // The mounted instance itself is the module.
                    if mbase.is_file() {
                        return mbase.canonicalize().ok();
                    }
                    for init in ["init.luau", "init.lua"] {
                        let c = mbase.join(init);
                        if c.is_file() {
                            return c.canonicalize().ok();
                        }
                    }
                    continue;
                }
                let mut dir = mbase.clone();
                for seg in &rest[..rest.len() - 1] {
                    dir = child(dir, seg);
                }
                if let Some(f) = project.module_file(&dir, &rest[rest.len() - 1]) {
                    return Some(f);
                }
            }
        }
        None
    }

    /// One instance step with nested-package canonicalization. Popping to
    /// a wally package's own tree root (`{"tree":{"$path":"src"}}`) lands
    /// on a directory that IS the package instance — `src` is not a
    /// DataModel level. Rewrite it to the package directory so further
    /// `Parent` steps count real instance levels; without this every
    /// `script.Parent^n` walked from package internals lands one level
    /// short (the biggest class of false
    /// R0501 on a large codebase). The probe is one `default.project.json` read on `Parent`
    /// pops only; a missing file fails the read immediately.
    fn step_norm(&self, base: Loc, seg: &str) -> Option<Loc> {
        let out = step(base, seg)?;
        if seg == "Parent" {
            if let Loc::Fs(d) = &out {
                if let Some(pkg) = d.parent() {
                    let is_tree_root = nested_project_tree(pkg)
                        .and_then(|t| {
                            t.get("$path").and_then(|p| p.as_str()).map(|p| pkg.join(p))
                        })
                        .is_some_and(|root| root == *d);
                    if is_tree_root {
                        return Some(Loc::Fs(pkg.to_path_buf()));
                    }
                }
            }
        }
        Some(out)
    }

    fn eval(
        &self,
        from: &Path,
        e: &hir::Expr,
        env: &HashMap<String, &hir::Expr>,
        depth: usize,
    ) -> Option<Loc> {
        if depth > 16 {
            return None;
        }
        match e {
            hir::Expr::Name { name, .. } => {
                if name == "game" {
                    return Some(Loc::Dm(Vec::new()));
                }
                if name == "script" {
                    return Some(Loc::Fs(self.instance_of(from)?));
                }
                // Alias local: follow its top-level initializer.
                let init = env.get(name)?;
                self.eval(from, init, env, depth + 1)
            }
            hir::Expr::Index { base, key, name, .. } => {
                let base = self.eval(from, base, env, depth + 1)?;
                let seg = match (name, key) {
                    (Some(n), _) => n.clone(),
                    (None, Some(k)) => literal_text(k)?,
                    _ => return None,
                };
                self.step_norm(base, &seg)
            }
            hir::Expr::Call { base, args, method, .. } => match method.as_deref() {
                // game:GetService("X") — also legal on non-game, but only
                // game has services; require the base to be the DM root.
                Some("GetService") => {
                    let Loc::Dm(path) = self.eval(from, base, env, depth + 1)? else {
                        return None;
                    };
                    if !path.is_empty() {
                        return None;
                    }
                    let service = literal_text(args.first()?)?;
                    Some(Loc::Dm(vec![service]))
                }
                Some("WaitForChild") | Some("FindFirstChild") => {
                    let base = self.eval(from, base, env, depth + 1)?;
                    let child = literal_text(args.first()?)?;
                    self.step_norm(base, &child)
                }
                // `require(X:Clone())` — a clone is a fresh instance of the
                // SAME source (a separate require-cache entry, which is why
                // people do it). The edge is statically knowable; identity
                // is not resolution's concern.
                Some("Clone") if args.is_empty() => self.eval(from, base, env, depth + 1),
                _ => None,
            },
            _ => None,
        }
    }
}

impl rime_check::project::Resolver for RojoResolver {
    fn resolve(
        &self,
        from: &Path,
        target: &hir::Expr,
        env: &HashMap<String, &hir::Expr>,
    ) -> Resolution {
        // A string literal is a path in Luau's own require syntax, not an
        // instance expression: its rules are in `strings.rs`.
        if let Some(text) = Self::string_target(target) {
            return self.resolve_string(from, &text).0;
        }
        // eval is a pure AST walk (the one fs touch, `script`'s instance,
        // has its own cache); everything after it depends only on the Loc.
        let Some(loc) = self.eval(from, target, env, 0) else {
            return Resolution::Dynamic;
        };
        // Lock scopes are one statement each: `resolve_loc` may recurse
        // into `resolve` (linker chains), so no guard may outlive its line.
        let cached = self.resolved.read().unwrap().get(&loc).cloned();
        if let Some(cached) = cached {
            return cached;
        }
        let r = self.resolve_loc(&loc);
        self.resolved.write().unwrap().insert(loc, r.clone());
        r
    }

    fn note(&self, from: &Path, target: &hir::Expr) -> Option<RequireNote> {
        self.string_note(from, target)
    }

    fn candidates(&self, _from: &Path, name: &str) -> Vec<String> {
        // OnceLock, not a check-then-build under a Mutex: the build walks
        // the whole project, and with the per-file pass parallel, every
        // worker that hit a miss at once would otherwise build its own copy
        // (dozens of builds on a large project — seconds of CPU for a
        // 0.1s index).
        self.name_index
            .get_or_init(|| self.build_name_index())
            .get(name)
            .cloned()
            .unwrap_or_default()
    }

    /// ServerScriptService and ServerStorage do not replicate: a module
    /// under either can be required only by code that never runs on a
    /// client. Everything else is visible to both sides.
    fn candidate_reachable(&self, from: &Path, candidate: &str) -> bool {
        let server_only = ["game.ServerScriptService", "game.ServerStorage"].iter().any(|root| {
            candidate
                .strip_prefix(root)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with(['.', '[']))
        });
        !server_only || !self.context_of(from).client
    }

    fn step(&self, base: &Loc, seg: &str) -> Option<Loc> {
        self.step_norm(base.clone(), seg)
    }

    fn service(&self, name: &str) -> Option<Loc> {
        Some(Loc::Dm(vec![name.to_string()]))
    }

    fn instance_of(&self, file: &Path) -> Option<Loc> {
        self.instance_of(file).map(Loc::Fs) // existing inherent method
    }

    fn resolve_loc(&self, loc: &Loc) -> Resolution {
        // Same memo the whole-expr resolve() uses — one-statement lock scopes.
        let cached = self.resolved.read().unwrap().get(loc).cloned();
        if let Some(cached) = cached {
            return cached;
        }
        let r = self.resolve_loc(loc); // existing inherent method
        self.resolved.write().unwrap().insert(loc.clone(), r.clone());
        r
    }

    /// Presence, not requireability — see `Resolver::present`. A `Loc::Fs`
    /// instance path is virtual (`D/Name` for `D/Name.luau`), so a module
    /// spelling is `resolve_loc`'s answer and the evaluator asks that
    /// first; what is left for this to prove is a directory. Same evidence
    /// rule as `dm_present`, for the same reason: a mount base counts on
    /// bare existence (Rojo makes an instance of whatever the `$path`
    /// names), anything deeper must be a DIRECTORY, or `wally.toml` and
    /// `LICENSE` would read as instances. Outside every mount, nothing is
    /// proof — Rojo does not sync what it cannot see.
    fn present(&self, loc: &Loc) -> bool {
        let cached = self.presence.read().unwrap().get(loc).copied();
        if let Some(cached) = cached {
            return cached;
        }
        let p = match loc {
            Loc::Dm(dm) => self.project.dm_present(dm),
            Loc::Fs(inst) => self.project.mounts.iter().any(|(_, base)| {
                if inst == base {
                    base.exists()
                } else {
                    under(inst, base) && inst.is_dir()
                }
            }),
        };
        self.presence.write().unwrap().insert(loc.clone(), p);
        p
    }

    /// Stem rule first (`.server`/`.client` name their own context outright,
    /// regardless of mount); else the longest mount whose base contains the
    /// file, keyed off that mount's FIRST DataModel segment.
    fn context_of(&self, file: &Path) -> CtxSet {
        let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        if stem.ends_with(".server") {
            return CtxSet { server: true, client: false };
        }
        if stem.ends_with(".client") {
            return CtxSet { server: false, client: true };
        }
        let root = self
            .project
            .mounts
            .iter()
            .filter(|(_, base)| file.starts_with(base))
            .max_by_key(|(dm, _)| dm.len())
            .and_then(|(dm, _)| dm.first().cloned());
        match root.as_deref() {
            Some("ServerScriptService") | Some("ServerStorage") => {
                CtxSet { server: true, client: false }
            }
            Some(s) if s.starts_with("Starter") => CtxSet { server: false, client: true },
            _ => CtxSet::BOTH,
        }
    }
}

impl RojoResolver {
    /// The filesystem half of `resolve`: Loc to Resolution. Deterministic
    /// per Loc — memoized by the caller.
    fn resolve_loc(&self, loc: &Loc) -> Resolution {
        match loc {
            Loc::Dm(dm) => {
                let r = self.project.dm_to_file(dm);
                match r {
                    Resolution::Found(_) => r,
                    _ if self.is_trusted(dm) => Resolution::Trusted,
                    _ => r,
                }
            }
            Loc::Fs(inst) => {
                let (Some(name), Some(dir)) = (
                    inst.file_name().map(|n| n.to_string_lossy().into_owned()),
                    inst.parent().map(Path::to_path_buf),
                ) else {
                    return Resolution::Dynamic;
                };
                if let Some(f) = self.project.module_file(&dir, &name) {
                    return Resolution::Found(f);
                }
                // Wally package indirection: the path may cross a directory
                // whose own default.project.json redirects into it.
                if let Some(f) = self.nested_resolve(inst) {
                    return Resolution::Found(f);
                }
                // Nothing on disk. Runtime-parented instances land here too
                // (a loader cloning children in) — hence Missing is still a
                // conservative claim, not a proven one.
                let shown = inst.strip_prefix(&self.project.root).unwrap_or(inst);
                let shown = shown.to_string_lossy();
                let shown = shown.strip_prefix(r"\\?\").unwrap_or(&shown);
                Resolution::Missing {
                    described: format!("script-relative `{shown}`"),
                }
            }
        }
    }
}

impl RojoResolver {
    /// The instance a script file IS: `D/Name.luau` -> `D/Name` (virtual);
    /// `D/init.luau` -> `D`. Rojo strips the script-class suffix, so
    /// `Worker.client.luau` is the instance `Worker`. Cached per file —
    /// every `script`-relative require in a file re-asks.
    ///
    /// Package collapse: `P/src/init.luau` where `P/default.project.json`
    /// maps its tree root to `src` IS the package instance `P` — its
    /// DataModel parent is P's parent (where wally puts the sibling linker
    /// files), not `P` itself.
    fn instance_of(&self, file: &Path) -> Option<PathBuf> {
        let cached = self.instances.read().unwrap().get(file).cloned();
        if let Some(cached) = cached {
            return cached;
        }
        let inst = self.instance_of_uncached(file);
        self.instances.write().unwrap().insert(file.to_path_buf(), inst.clone());
        inst
    }

    fn instance_of_uncached(&self, file: &Path) -> Option<PathBuf> {
        let stem = file.file_stem()?.to_string_lossy();
        let stem = stem
            .strip_suffix(".client")
            .or_else(|| stem.strip_suffix(".server"))
            .unwrap_or(&stem);
        let dir = file.parent()?;
        if stem != "init" {
            return Some(dir.join(stem));
        }
        if let (Some(parent), Some(dir_name)) = (dir.parent(), dir.file_name()) {
            if self.project_root_path(parent).as_deref() == Some(Path::new(dir_name)) {
                return Some(parent.to_path_buf());
            }
        }
        Some(dir.to_path_buf())
    }

    /// The `tree.$path` of `dir/default.project.json`, when the tree root
    /// maps straight to a path (the wally package shape). Cached per dir —
    /// this reads and parses JSON on every miss otherwise.
    fn project_root_path(&self, dir: &Path) -> Option<PathBuf> {
        let cached = self.pkg_roots.read().unwrap().get(dir).cloned();
        if let Some(cached) = cached {
            return cached;
        }
        let root = (|| {
            let text = std::fs::read_to_string(dir.join("default.project.json")).ok()?;
            let value: serde_json::Value = serde_json::from_str(&text).ok()?;
            let p = value.get("tree")?.get("$path")?.as_str()?;
            Some(PathBuf::from(p))
        })();
        self.pkg_roots.write().unwrap().insert(dir.to_path_buf(), root.clone());
        root
    }
}

fn step(loc: Loc, seg: &str) -> Option<Loc> {
    match loc {
        Loc::Dm(mut path) => {
            if seg == "Parent" {
                path.pop()?;
                // popping past the root would be `game.Parent` — nonsense
                Some(Loc::Dm(path))
            } else {
                path.push(seg.to_string());
                Some(Loc::Dm(path))
            }
        }
        Loc::Fs(inst) => {
            if seg == "Parent" {
                Some(Loc::Fs(inst.parent()?.to_path_buf()))
            } else {
                Some(Loc::Fs(inst.join(seg)))
            }
        }
    }
}

/// Match one trust pattern against a DataModel path. `game.A.B` is exact,
/// `*` matches one segment, a trailing `**` matches any non-empty subtree.
fn trust_matches(pattern: &str, dm: &[String]) -> bool {
    let mut segs: Vec<&str> = pattern.split('.').collect();
    if segs.first() != Some(&"game") {
        return false;
    }
    segs.remove(0);
    let deep = segs.last() == Some(&"**");
    if deep {
        segs.pop();
        if dm.len() <= segs.len() {
            return false;
        }
    } else if dm.len() != segs.len() {
        return false;
    }
    segs.iter().zip(dm).all(|(p, s)| *p == "*" || *p == s.as_str())
}

fn literal_text(e: &hir::Expr) -> Option<String> {
    // Prim guard: numbers and booleans carry text too, and `t[1]` must not
    // resolve as a child named "1".
    let hir::Expr::Literal { prim: hir::Prim::String, text: Some(t), .. } = e else {
        return None;
    };
    Some(t.clone())
}

/// A DataModel path as the Luau expression that names it: `game`, then
/// `.Name` for a segment that is an identifier and `["Name"]` for one that
/// is not (`Car-V2`, `My Folder`, a keyword). R0501 writes this text into a
/// `require(...)`, so it has to mean the path: joined with dots, `Car-V2`
/// is a subtraction.
fn dm_expression(segs: &[String]) -> String {
    const KEYWORDS: [&str; 21] = [
        "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "if", "in",
        "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
    ];
    let mut out = String::from("game");
    for seg in segs {
        let mut chars = seg.chars();
        let identifier = chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !KEYWORDS.contains(&seg.as_str());
        if identifier {
            out.push('.');
            out.push_str(seg);
        } else {
            out.push_str("[\"");
            for c in seg.chars() {
                match c {
                    '"' | '\\' => {
                        out.push('\\');
                        out.push(c);
                    }
                    c if c.is_control() => out.push_str(&format!("\\u{{{:X}}}", c as u32)),
                    c => out.push(c),
                }
            }
            out.push_str("\"]");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> RojoProject {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        RojoProject::load(&root).unwrap()
    }

    fn resolver_for_fixture(name: &str) -> RojoResolver {
        RojoResolver::new(fixture(name))
    }

    /// A scratch folder holding `files` (all empty), removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str, files: &[&str]) -> Scratch {
            let dir = std::env::temp_dir().join(format!("rime-roblox-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            for file in files {
                let path = dir.join(file);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, "{}").unwrap();
            }
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    #[test]
    fn project_file_names() {
        assert!(is_project_file_name("default.project.json"));
        assert!(is_project_file_name("place.project.json"));
        assert!(is_project_file_name("place.project.jsonc"));
        assert!(!is_project_file_name("project.json"));
        assert!(!is_project_file_name("place.project.json.bak"));
        assert!(!is_project_file_name("tsconfig.json"));
    }

    #[test]
    fn the_search_takes_the_nearest_folder_with_a_project_file() {
        let s = Scratch::new("search-nearest", &["default.project.json", "pkg/pkg.project.json", "pkg/src/a.luau"]);
        // Nearest wins, whatever it is called.
        assert_eq!(find_project_file(&s.0.join("pkg/src")), ProjectSearch::Found(s.0.join("pkg/pkg.project.json")));
        assert_eq!(find_project_file(&s.0), ProjectSearch::Found(s.0.join("default.project.json")));
    }

    #[test]
    fn the_search_prefers_default_and_does_not_guess_between_others() {
        let s = Scratch::new(
            "search-default",
            &["a/default.project.json", "a/test.project.json", "b/one.project.json", "b/two.project.json", "b/src/x.luau"],
        );
        assert_eq!(find_project_file(&s.0.join("a")), ProjectSearch::Found(s.0.join("a/default.project.json")));
        assert_eq!(
            find_project_file(&s.0.join("b/src")),
            ProjectSearch::Several {
                dir: s.0.join("b"),
                files: vec!["one.project.json".to_string(), "two.project.json".to_string()],
            }
        );
    }

    #[test]
    fn the_search_takes_jsonc_and_skips_folders_named_like_project_files() {
        let s = Scratch::new("search-jsonc", &["a/default.project.jsonc", "a/other.project.json", "b/odd.project.json/x.txt"]);
        assert_eq!(find_project_file(&s.0.join("a")), ProjectSearch::Found(s.0.join("a/default.project.jsonc")));
        // `b/odd.project.json` is a folder: not a project file. Nothing
        // else is in reach inside the scratch folder, and what is above it
        // on this machine is not this test's to assert on.
        assert!(!matches!(find_project_file(&s.0.join("b")), ProjectSearch::Found(f) if f.starts_with(&s.0)));
    }

    #[test]
    fn optional_paths_and_comments_load() {
        let s = Scratch::new("optional", &["src/a.luau"]);
        std::fs::write(
            s.0.join("place.project.json"),
            r#"{
                // a comment
                "name": "x",
                "tree": {
                    "$className": "DataModel",
                    "ReplicatedStorage": {
                        "Shared": { "$path": { "optional": "src" } },
                        "Absent": { "$path": { "optional": "gone" } },
                    },
                },
            }"#,
        )
        .unwrap();
        let project = RojoProject::load_file(&s.0.join("place.project.json")).unwrap();
        let bases: Vec<&Path> = project.mount_bases().collect();
        assert!(bases.contains(&s.0.join("src").canonicalize().unwrap().as_path()), "{bases:?}");
    }

    /// A tree with `children` nodes that each mount `path`, plus `src`.
    fn tree_mounting(path: &str, children: usize) -> String {
        let nodes: Vec<String> = (0..children).map(|i| format!(r#""C{i}": {{ "$path": "{path}" }}"#)).collect();
        format!(
            r#"{{ "name": "x", "tree": {{ "$className": "DataModel", {}, "Src": {{ "$path": "src" }} }} }}"#,
            nodes.join(", ")
        )
    }

    /// A project whose tree mounts the project itself, by its file or as
    /// the folder it is in. Expanding it again at each such node never
    /// ends on its own: with N nodes and the depth cap of 8 that is N^8
    /// expansions (3 nodes: 6,561 mounts and seconds; 4 nodes: over a
    /// minute). A project already being expanded is a plain mount.
    #[test]
    fn a_project_that_mounts_itself_is_not_expanded_again() {
        for (name, path) in [("self-mount-file", "default.project.json"), ("self-mount-dot", ".")] {
            let s = Scratch::new(name, &["src/a.luau"]);
            std::fs::write(s.0.join("default.project.json"), tree_mounting(path, 3)).unwrap();
            let project = RojoProject::load(&s.0).unwrap();
            assert_eq!(project.mounts.len(), 4, "`$path`: \"{path}\": one mount per node");
            assert!(project.mount_bases().any(|b| b == s.0.join("src").canonicalize().unwrap()));
        }
    }

    /// The same through a second project: `pkg` mounts the folder above
    /// it, which is the project that mounted `pkg`.
    #[test]
    fn two_projects_that_mount_each_other_end() {
        let s = Scratch::new("mutual-mount", &["src/a.luau", "pkg/lib/b.luau"]);
        std::fs::write(s.0.join("default.project.json"), tree_mounting("pkg", 3)).unwrap();
        std::fs::write(
            s.0.join("pkg/default.project.json"),
            r#"{ "name": "pkg", "tree": { "$className": "Folder",
                "Up0": { "$path": ".." }, "Up1": { "$path": "../default.project.json" }, "Lib": { "$path": "lib" } } }"#,
        )
        .unwrap();
        let project = RojoProject::load(&s.0).unwrap();
        // Three `pkg` nodes, each with its three mounts, and `Src`.
        assert_eq!(project.mounts.len(), 10);
        // The nested project is still read: its `lib` is mounted.
        assert!(project.mount_bases().any(|b| b == s.0.join("pkg/lib").canonicalize().unwrap()));
    }

    /// Projects that nest without a cycle, each mounted several times by
    /// the one above, multiply: 4 levels of 6 is 1,296 expansions, 8
    /// levels of 4 is 65,536. One load reads a bounded number of nested
    /// projects; past it a `$path` is a plain mount.
    #[test]
    fn nested_projects_are_read_a_bounded_number_of_times() {
        let s = Scratch::new("nested-budget", &["l1/l2/l3/l4/l5/src/a.luau"]);
        let mut dir = s.0.clone();
        std::fs::write(dir.join("default.project.json"), tree_mounting("l1", 6)).unwrap();
        for level in 1..=5 {
            dir.push(format!("l{level}"));
            let below = if level == 5 { "src".to_string() } else { format!("l{}", level + 1) };
            std::fs::write(dir.join("default.project.json"), tree_mounting(&below, 6)).unwrap();
        }
        let project = RojoProject::load(&s.0).unwrap();
        assert!(
            project.mounts.len() <= 8 * NESTED_PROJECT_LIMIT,
            "{} mounts from 6^5 = 7,776 possible expansions",
            project.mounts.len()
        );
    }

    /// A path is on disk only under the spelling the disk has, in every
    /// part: the answer a Linux disk gives, given on every system.
    #[test]
    fn a_path_is_on_disk_only_as_spelled() {
        let s = Scratch::new("spelled", &["src/util/helper.luau"]);
        let root = s.0.canonicalize().unwrap();
        let at = |rel: &str| rel.split('/').fold(root.clone(), child);
        assert_eq!(as_spelled(&at("src/util/helper.luau")), Some(at("src/util/helper.luau")));
        assert_eq!(as_spelled(&at("src/util")), Some(at("src/util")));
        assert_eq!(as_spelled(&at("src/Util/helper.luau")), None, "a folder in the middle");
        assert_eq!(as_spelled(&at("SRC/util/helper.luau")), None, "the first part");
        assert_eq!(as_spelled(&at("src/util/Helper.luau")), None, "the last part");
        assert_eq!(as_spelled(&at("src/util/gone.luau")), None, "not there at all");
        // Another way to write the same path is not a misspelling.
        assert_eq!(as_spelled(&s.0.join("src/util/helper.luau")), Some(at("src/util/helper.luau")));
    }

    /// Walking UP from inside a wally package must collapse the package's
    /// own `default.project.json` tree root (`{"tree":{"$path":"src"}}`)
    /// onto the package instance: `src` is not a DataModel level. Without
    /// the collapse every `script.Parent^n` from package internals lands
    /// one level short.
    ///
    /// From `sdk-shared/src/Modules/Flags.lua` the DataModel parents
    /// are: Modules → sdk-shared (the package; its tree root `src`
    /// collapses) → `acme_sdk-shared@1.0.0` → `_Index` → Packages —
    /// five `Parent` steps, then `.SdkShared` names the linker file.
    #[test]
    fn nested_package_parent_walk_collapses_tree_root() {
        use rime_check::project::Resolver;
        let r = resolver_for_fixture("sdk_chain");
        let file = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/sdk_chain/Packages/_Index")
            .join("acme_sdk-shared@1.0.0/sdk-shared/src/Modules/Flags.lua")
            .canonicalize()
            .unwrap();
        let mut loc = Resolver::instance_of(&r, &file).expect("instance of Flags.lua");
        for _ in 0..5 {
            loc = Resolver::step(&r, &loc, "Parent").expect("Parent step");
        }
        let loc = Resolver::step(&r, &loc, "SdkShared").expect("child step");
        expect_found(
            Resolver::resolve_loc(&r, &loc),
            "sdk_chain/Packages/SdkShared.lua",
        );
    }

    fn dm(project: &RojoProject, segs: &[&str]) -> Resolution {
        let segs: Vec<String> = segs.iter().map(|s| s.to_string()).collect();
        project.dm_to_file(&segs)
    }

    fn expect_found(r: Resolution, fixture_rel: &str) {
        let expected = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(fixture_rel)
            .canonicalize()
            .unwrap();
        match r {
            Resolution::Found(f) => assert_eq!(f, expected),
            other => panic!("expected Found({fixture_rel}), got {other:?}"),
        }
    }

    // A `$path` mount whose directory is itself a Rojo project (vendored
    // package: own default.project.json with `tree.$path: src`, and src has
    // an init.lua) IS a ModuleScript at runtime — Rojo builds the nested
    // project's tree at the mount point. Requiring the mount point must
    // resolve to the nested tree root's init file.
    #[test]
    fn nested_project_mount_point_resolves() {
        let p = fixture("nested_mount");
        expect_found(
            dm(&p, &["ReplicatedStorage", "Widgets"]),
            "nested_mount/vendor/Widgets/src/init.lua",
        );
    }

    // A path THROUGH the nested project must land on the nested tree's
    // files, not on `vendor/Widgets/<child>`.
    #[test]
    fn nested_project_child_resolves() {
        let p = fixture("nested_mount");
        expect_found(
            dm(&p, &["ReplicatedStorage", "Widgets", "Client"]),
            "nested_mount/vendor/Widgets/src/Client.lua",
        );
    }

    // `game.Players.LocalPlayer.PlayerScripts.X` is the runtime copy of the
    // StarterPlayerScripts mount — a forked PlayerModule resolves to its
    // mounted source file.
    #[test]
    fn player_scripts_resolves_through_starter_mount() {
        let p = fixture("player_scripts");
        expect_found(
            dm(&p, &["Players", "LocalPlayer", "PlayerScripts", "PlayerModule", "ControlModule"]),
            "player_scripts/src/StarterPlayerScripts/PlayerModule/ControlModule/init.lua",
        );
    }

    // A PlayerScripts path NOT under the mount stays OutsideView, never
    // Missing: the engine injects its own defaults (an un-forked
    // PlayerModule) beside the copied files.
    #[test]
    fn player_scripts_miss_is_outside_view() {
        let p = fixture("player_scripts");
        let r = dm(&p, &["Players", "LocalPlayer", "PlayerScripts", "SomethingElse"]);
        assert!(matches!(r, Resolution::OutsideView), "got {r:?}");
    }

    // The plain mount alongside the nested one keeps working.
    #[test]
    fn plain_mount_still_resolves() {
        let p = fixture("nested_mount");
        expect_found(
            dm(&p, &["ReplicatedStorage", "Placeholder"]),
            "nested_mount/src/ReplicatedStorage/Placeholder.lua",
        );
    }

    // `context_of` is lexical: mount root decides server/client/shared, but
    // a `.server`/`.client` stem overrides the mount.
    #[test]
    fn context_lexical() {
        use rime_check::project::{CtxSet, Resolver};
        let r = resolver_for_fixture("contexts");
        let f = |rel: &str| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/contexts")
                .join(rel)
                .canonicalize()
                .unwrap()
        };
        assert_eq!(
            r.context_of(&f("src/server/Main.luau")),
            CtxSet { server: true, client: false }
        );
        assert_eq!(
            r.context_of(&f("src/client/Hud.luau")),
            CtxSet { server: false, client: true }
        );
        assert_eq!(r.context_of(&f("src/shared/Util.luau")), CtxSet::BOTH);
        // stem beats mount:
        assert_eq!(
            r.context_of(&f("src/shared/Worker.server.luau")),
            CtxSet { server: true, client: false }
        );
    }

    // ---- SDK chain, end to end -------------------------
    //
    // The `sdk_chain` fixture is a wally layout with a two-package SDK
    // (see `sdk-shared/src/init.lua`'s header): a shared package whose
    // export table is built with `Modules = locateModules()`, where
    // `locateModules` branches on `RunService:IsServer()` and — on the
    // server — hops through `ServerStorage.ServerPackages.Sdk` (a
    // `return script` package) to reach a DIFFERENT `Modules` folder than
    // the client sees.
    //
    // Most of what the evaluator can do is on this one path: context
    // truth, a same-file function call that makes another, an `or`
    // fallback, a guard that must prune, a nested computed
    // `require(...)`, a field of another module's export table, and two
    // linker chases.

    /// `present` answers a different question than `resolve_loc`: not "is
    /// this requireable" but "does the project create this instance".
    #[test]
    fn present_proves_containers_not_modules() {
        use rime_check::project::Resolver;
        let r = resolver_for_fixture("sdk_chain");
        let dmv = |segs: &[&str]| Loc::Dm(segs.iter().map(|s| s.to_string()).collect());

        // A mounted FOLDER: `dm_to_file` cannot make a module of it...
        let ssp = dmv(&["ServerStorage", "ServerPackages"]);
        assert!(
            matches!(r.project.dm_to_file(&["ServerStorage".into(), "ServerPackages".into()]), Resolution::Missing { .. }),
            "expected ServerStorage.ServerPackages to be a non-module"
        );
        // ...but it unquestionably exists.
        assert!(r.present(&ssp));
        // An ancestor of a mount: Rojo builds the containers down to it.
        assert!(r.present(&dmv(&["ServerStorage"])));
        // A folder deeper inside a mount, the same way.
        assert!(r.present(&dmv(&["ServerStorage", "ServerPackages", "_Index"])));
        // Nothing there — no proof, which is NOT a claim of absence.
        assert!(!r.present(&dmv(&["ServerStorage", "Packages"])));

        // Evidence has to be an INSTANCE, not just a file: every wally
        // package ships a `wally.toml` beside its `src`, and Rojo syncs
        // none of them. A bare `exists()` would call it present.
        let pkg = [
            "ServerStorage",
            "ServerPackages",
            "_Index",
            "acme_sdk@1.0.0",
            "sdk",
        ];
        let with = |leaf: &str| {
            let mut v: Vec<&str> = pkg.to_vec();
            v.push(leaf);
            dmv(&v)
        };
        assert!(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/sdk_chain/ServerPackages/_Index/acme_sdk@1.0.0/sdk/wally.toml")
                .is_file(),
            "fixture must actually ship the wally.toml this pins"
        );
        assert!(!r.present(&with("wally.toml")), "a non-synced file is not an instance");
        // The directory beside it is one.
        assert!(r.present(&with("src")));

        // Loc::Fs takes the same evidence, and nothing outside a mount
        // counts at all.
        let fs = |rel: &str| {
            Loc::Fs(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures/sdk_chain")
                    .join(rel)
                    .canonicalize()
                    .unwrap(),
            )
        };
        assert!(r.present(&fs("ServerPackages/_Index")));
        assert!(!r.present(&fs(
            "ServerPackages/_Index/acme_sdk@1.0.0/sdk/wally.toml"
        )));
        // The project file at the fixture root is under no mount.
        assert!(!r.present(&fs("default.project.json")));
    }

    /// The fixture's mounted source set and a resolver over it — the same
    /// pair `check_project` gets from the CLI.
    fn chain_session(name: &str) -> rime_check::session::Session<'static> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
            .canonicalize()
            .unwrap();
        let project = RojoProject::load(&root).unwrap();
        let files: Vec<(PathBuf, String)> = project
            .mount_files()
            .into_iter()
            .map(|p| {
                let src = std::fs::read_to_string(&p).unwrap();
                (p, src)
            })
            .collect();
        assert!(!files.is_empty(), "fixture {name} mounted no files");
        rime_check::session::Session::new(
            files,
            Some(Box::new(RojoResolver::new(project))),
            rime_std::StdNames::bundled().clone(),
            None,
        )
    }

    /// The require diagnostics (R0501 broken / R0502 dynamic) on one file
    /// of a chain fixture, keyed by file name — every name in these
    /// fixtures is unique.
    fn require_diags(
        session: &rime_check::session::Session<'_>,
        file_name: &str,
    ) -> Vec<(String, Vec<String>)> {
        let reports = session.reports();
        let report = reports
            .iter()
            .find(|r| r.path.file_name().is_some_and(|n| n == file_name))
            .unwrap_or_else(|| panic!("no report for {file_name}"));
        report
            .diagnostics
            .iter()
            .filter(|d| d.code.to_string() == "R0501" || d.code.to_string() == "R0502")
            .map(|d| (format!("{}: {}", d.code, d.message), d.notes.clone()))
            .collect()
    }

    /// Server context: `RunService:IsServer()` is true, so `locateModules`
    /// goes on to `serverHalf` — `FindFirstChild("ServerPackages")` is a
    /// mounted folder (present, so the `or` takes it and the `not mount`
    /// guard prunes), `require(mount.Sdk)` chases the ServerPackages
    /// linker into `sdk/src/init.lua`'s `return script`, and `.Modules`
    /// steps into the package's own tree.
    #[test]
    fn sdk_server_consumer_resolves() {
        let s = chain_session("sdk_chain");
        assert_eq!(require_diags(&s, "Boot.server.luau"), Vec::new());
    }

    /// Client context: `RunService:IsServer()` is false, so
    /// `locateModules` returns `script.Modules` — `script` is the collapsed
    /// package instance (`sdk-shared`, not `sdk-shared/src`), and
    /// the step resolves back through the package project.
    #[test]
    fn sdk_client_consumer_resolves() {
        let s = chain_session("sdk_chain");
        assert_eq!(require_diags(&s, "Hud.client.luau"), Vec::new());
    }

    /// A shared module runs in BOTH contexts, so the same require has two
    /// candidates — the server `Modules/Flags.lua` and the shared one.
    /// Byte-identical here, so the union collapses to one file: silence.
    #[test]
    fn sdk_shared_consumer_union() {
        let s = chain_session("sdk_chain");
        assert_eq!(require_diags(&s, "Common.luau"), Vec::new());
    }

    /// Same chain, but the two `Modules/Flags.lua` copies differ, as a
    /// client shim and its server counterpart do. The shared consumer cannot be told
    /// which module it loads: R0502 naming both. The server consumer has
    /// one context and still resolves.
    #[test]
    fn sdk_shared_consumer_diverged() {
        let s = chain_session("sdk_chain_diverged");
        let shared = require_diags(&s, "Common.luau");
        assert_eq!(shared.len(), 1, "expected exactly one require diagnostic, got {shared:?}");
        assert_eq!(shared[0].0, "R0502: require target is not statically resolvable");
        // Separator-agnostic: the note renders real paths, `\` on Windows.
        let notes: Vec<String> =
            shared[0].1.iter().map(|n| n.replace('\\', "/")).collect();
        assert!(
            notes.iter().any(|n| n.contains("which differ")
                // server candidate first (collapse step 6 orders by ctx),
                // and this is a genuine per-ctx split, so each copy carries
                // the ctx that lands on it.
                && n.contains(
                    "acme_sdk@1.0.0/sdk/src/Modules/Flags.lua (server) / \
                     Packages/_Index/acme_sdk-shared@1.0.0/sdk-shared\
                     /src/Modules/Flags.lua (client)"
                )),
            "expected a labeled `which differ` note naming both copies, got {notes:?}"
        );
        assert_eq!(require_diags(&s, "Boot.server.luau"), Vec::new());
    }

    /// When exactly one module carries a broken require's name, R0501
    /// offers to write that module's path over the broken one. The path is
    /// written into the file, so it has to be Luau and it has to resolve:
    /// a name that is not an identifier (`Car-V2`, `My Folder`) is indexed
    /// with brackets. Joined with dots, `Car-V2` is a subtraction and
    /// `My Folder` a syntax error.
    #[test]
    fn dm_expression_brackets_what_is_not_an_identifier() {
        let path = |segs: &[&str]| {
            dm_expression(&segs.iter().map(|s| s.to_string()).collect::<Vec<_>>())
        };
        assert_eq!(path(&["ReplicatedStorage", "Util", "Helper"]), "game.ReplicatedStorage.Util.Helper");
        assert_eq!(path(&["ReplicatedStorage", "Car-V2"]), r#"game.ReplicatedStorage["Car-V2"]"#);
        assert_eq!(path(&["ReplicatedStorage", "My Folder", "X"]), r#"game.ReplicatedStorage["My Folder"].X"#);
        assert_eq!(path(&["ReplicatedStorage", "A.B"]), r#"game.ReplicatedStorage["A.B"]"#);
        assert_eq!(path(&["ReplicatedStorage", "3D"]), r#"game.ReplicatedStorage["3D"]"#);
        assert_eq!(path(&["ReplicatedStorage", "end"]), r#"game.ReplicatedStorage["end"]"#);
        assert_eq!(path(&["ReplicatedStorage", r#"a"b\c"#]), r#"game.ReplicatedStorage["a\"b\\c"]"#);
    }

    #[test]
    fn broken_require_edit_writes_a_path_that_resolves() {
        let mut s = chain_session("rotted_paths");
        let reports = s.reports();
        let report = reports
            .iter()
            .find(|r| r.path.file_name().is_some_and(|n| n == "Main.luau"))
            .unwrap();
        let path = report.path.clone();
        let mut src = std::fs::read_to_string(&path).unwrap();

        let mut edits: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.code.to_string() == "R0501")
            .flat_map(|d| d.suggestions.clone())
            .collect();
        assert_eq!(edits.len(), 4, "one edit per broken require: {edits:?}");
        // Last first, so earlier spans stay where they were.
        edits.sort_by_key(|e| std::cmp::Reverse(e.span.start));
        for edit in &edits {
            src = rime_check::edits::apply(&src, edit).unwrap();
        }

        assert!(src.contains("require(game.ReplicatedStorage.Util.Helper)"), "{src}");
        assert!(src.contains(r#"require(game.ReplicatedStorage.Util["Car-V2"])"#), "{src}");
        assert!(src.contains(r#"require(game.ReplicatedStorage["My Folder"].Spaced)"#), "{src}");
        // Server code may be pointed at a server container.
        assert!(src.contains("require(game.ServerScriptService.Vaults.Vault)"), "{src}");

        s.update(&path, src);
        assert_eq!(require_diags(&s, "Main.luau"), Vec::new());
    }

    /// The one module with the name is somewhere the requiring file cannot
    /// load it from: `Hud` sits under ReplicatedStorage, so it can run on a
    /// client, and the only `Vault` is under ServerScriptService. Writing
    /// that path would swap a require Rime reports for one that fails on
    /// every client and that Rime does not report. The module is still
    /// named in the note; no edit is offered.
    #[test]
    fn broken_require_edit_is_withheld_where_a_client_cannot_load_the_module() {
        let s = chain_session("rotted_paths");
        let reports = s.reports();
        let report = reports
            .iter()
            .find(|r| r.path.file_name().is_some_and(|n| n == "Hud.luau"))
            .unwrap();
        let broken: Vec<_> =
            report.diagnostics.iter().filter(|d| d.code.to_string() == "R0501").collect();
        assert_eq!(broken.len(), 1);
        let d = broken[0];
        assert!(d.suggestions.is_empty(), "{:?}", d.suggestions);
        assert!(
            d.notes.iter().any(|n| n.contains("`game.ServerScriptService.Vaults.Vault`")),
            "{:?}",
            d.notes
        );
    }
}
