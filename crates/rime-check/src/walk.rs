//! Source-file discovery shared by the CLI and the Rojo resolver.
//!
//! One walker, so every path Rime handles is built the same way: the caller
//! hands in an ALREADY-CANONICAL root (one `canonicalize` per root — a
//! handful of syscalls) and every yielded path is `root.join(...)` with the
//! on-disk casing `read_dir` reports. That is what lets the CLI's requested
//! set, the project's mount files, and rime.toml's `ignore` prefixes compare
//! byte-for-byte without a per-file `canonicalize` (a syscall each — three
//! serial sweeps of it were a large share of a run on a large codebase).
//!
//! Order is pre-order over a directory's entries sorted by name (plain byte
//! order, see `list_dir`), descending into each subdirectory at the point
//! it is met — the same on every operating system and file system, and the
//! same order `Vec<PathBuf>::sort` gives. Directories are read in parallel
//! because the cost is per-directory syscall latency, not CPU; the results
//! splice back in listing order, so the parallel and the serial walk agree.
//! Symlinks are not followed. `.git` is skipped — it never holds sources
//! and on a real repo it is most of the directories.
//!
//! `sources` and `sources_serial` share `list_dir` (entry classification) so
//! they cannot drift apart — see `sources_serial` for when the serial
//! variant is required instead of the parallel one.

use rayon::prelude::*;
use std::path::{Path, PathBuf};

/// `.luau` / `.lua` files under `root` (or `root` itself when it is one).
pub fn sources(root: &Path) -> Vec<PathBuf> {
    if root.is_dir() {
        walk_dir(root)
    } else if is_source(root) {
        vec![root.to_path_buf()]
    } else {
        Vec::new()
    }
}

/// Same listing, same pre-order, as `sources` — but with a plain recursive
/// `read_dir`, no rayon. For callers that may already be running on a rayon
/// worker thread while holding a lock or inside a `OnceLock` initializer:
/// a worker parked in `collect()` there steals other queued jobs onto
/// itself, and if a stolen job tries to re-enter the same (non-reentrant)
/// lock or the same `OnceLock` currently being initialized, it deadlocks
/// (found at scale in `RojoProject::mount_walk`'s lazy path — see there).
pub fn sources_serial(root: &Path) -> Vec<PathBuf> {
    if root.is_dir() {
        walk_dir_serial(root)
    } else if is_source(root) {
        vec![root.to_path_buf()]
    } else {
        Vec::new()
    }
}

fn is_source(path: &Path) -> bool {
    path.extension().is_some_and(|x| x == "luau" || x == "lua")
}

pub enum Item {
    File(PathBuf),
    Dir(PathBuf),
}

impl Item {
    /// The entry's own name, as the bytes the listing is sorted by.
    fn name_bytes(&self) -> &[u8] {
        let (Item::File(path) | Item::Dir(path)) = self;
        path.file_name().unwrap_or_default().as_encoded_bytes()
    }
}

/// One directory, read once.
#[derive(Default)]
pub struct Listing {
    /// Source files to keep and subdirectories to descend into, sorted by
    /// name in plain byte order, files and subdirectories together.
    pub items: Vec<Item>,
    /// The directory holds a `.gitignore` file.
    pub has_gitignore: bool,
    /// The directory holds a `.git` entry — a directory, or the file a
    /// linked worktree or submodule has — so it is the top of a git work
    /// tree.
    pub has_git: bool,
}

/// One directory's entries, classified into files to keep and
/// subdirectories to descend into — shared by every walker (the two here
/// and the CLI's git-ignore-aware one) so their file/skip rules (`.git`,
/// extensions) cannot drift.
pub fn list_dir(dir: &Path) -> Listing {
    let mut listing = Listing::default();
    let Ok(read) = std::fs::read_dir(dir) else {
        return listing;
    };
    // `file_type` comes free with the directory listing on every platform
    // Rime runs on — no extra stat per entry.
    for entry in read.filter_map(Result::ok) {
        let Ok(kind) = entry.file_type() else { continue };
        let name = entry.file_name();
        if name == ".git" {
            listing.has_git = true;
            continue;
        }
        let path = entry.path();
        if kind.is_dir() {
            listing.items.push(Item::Dir(path));
        } else if is_source(&path) {
            listing.items.push(Item::File(path));
        } else if name == ".gitignore" {
            listing.has_gitignore = true;
        }
    }
    // `read_dir` hands entries back in the file system's own order: by
    // name without regard to case on NTFS, by a hash of the name on ext4,
    // newest first on tmpfs. Files are checked and reported in the order
    // they are listed, so the listing is sorted here, by name, in plain
    // byte order: no case folding, no locale, no number awareness. `Zeta`
    // comes before `alpha` and `10` before `9`, on every system.
    listing.items.sort_unstable_by(|a, b| a.name_bytes().cmp(b.name_bytes()));
    listing
}

fn walk_dir(dir: &Path) -> Vec<PathBuf> {
    let items = list_dir(dir).items;
    // Subdirectories walk on the pool; results splice back at the position
    // the entry had in the listing, preserving pre-order.
    items
        .into_par_iter()
        .map(|item| match item {
            Item::File(path) => vec![path],
            Item::Dir(path) => walk_dir(&path),
        })
        .collect::<Vec<Vec<PathBuf>>>()
        .into_iter()
        .flatten()
        .collect()
}

fn walk_dir_serial(dir: &Path) -> Vec<PathBuf> {
    list_dir(dir)
        .items
        .into_iter()
        .flat_map(|item| match item {
            Item::File(path) => vec![path],
            Item::Dir(path) => walk_dir_serial(&path),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// `sources` (parallel) and `sources_serial` must list the exact same
    /// files in the exact same order — `.git` contents and non-source
    /// extensions excluded, `.lua`/`.luau` included at every depth.
    #[test]
    fn sources_and_sources_serial_agree() {
        // Process-suffixed: two concurrent `cargo test` runs (workspace +
        // per-crate) would otherwise share one directory and delete each
        // other's fixtures mid-walk.
        let dir = std::env::temp_dir().join(format!("rime-walk-serial-parity-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sub1/.git")).unwrap();
        fs::create_dir_all(dir.join("sub1/sub2")).unwrap();
        fs::write(dir.join("a.lua"), "").unwrap();
        fs::write(dir.join("b.txt"), "").unwrap();
        fs::write(dir.join("sub1/c.luau"), "").unwrap();
        fs::write(dir.join("sub1/.git/d.lua"), "").unwrap();
        fs::write(dir.join("sub1/sub2/e.lua"), "").unwrap();
        fs::write(dir.join("sub1/sub2/f.txt"), "").unwrap();

        let par = sources(&dir);
        let ser = sources_serial(&dir);
        assert_eq!(par, ser, "parallel and serial walkers must agree, order included");
        assert_eq!(par.len(), 3, "expected exactly a.lua, sub1/c.luau, sub1/sub2/e.lua");
        for p in &par {
            assert!(
                !p.components().any(|c| c.as_os_str() == ".git"),
                "{p:?} should have skipped .git"
            );
            assert_ne!(
                p.extension().and_then(|e| e.to_str()),
                Some("txt"),
                "{p:?} should not be a .txt file"
            );
        }

        let _ = fs::remove_dir_all(&dir);
    }

    /// A folder's entries come sorted by name, byte for byte, whatever
    /// order the file system hands them back in: NTFS lists `alpha` before
    /// `Zeta`, ext4 lists by a hash of the name.
    #[test]
    fn entries_come_in_byte_order_of_their_names() {
        let dir = std::env::temp_dir().join(format!("rime-walk-order-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        // Written in an order that is neither the expected one nor
        // alphabetical, for the file systems that list by creation.
        let written = [
            "alpha.luau",
            "a.luau",
            "Zeta.luau",
            "9.luau",
            "beta.lua",
            "a/x.luau",
            "_first.luau",
            "Beta/inner.luau",
            "10.luau",
            "a-b.luau",
        ];
        for rel in written {
            let path = dir.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "").unwrap();
        }
        let root = dir.canonicalize().unwrap();
        let names = |files: &[PathBuf]| -> Vec<String> {
            files
                .iter()
                .map(|p| p.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/"))
                .collect()
        };

        // Digits, then capitals, then `_`, then small letters. A folder
        // sorts by its own name among the files beside it: `a` comes
        // before `a-b.luau` and `a.luau`, and its files come with it.
        let expected = [
            "10.luau",
            "9.luau",
            "Beta/inner.luau",
            "Zeta.luau",
            "_first.luau",
            "a/x.luau",
            "a-b.luau",
            "a.luau",
            "alpha.luau",
            "beta.lua",
        ];
        let walked = sources(&root);
        assert_eq!(names(&walked), expected);
        assert_eq!(names(&sources_serial(&root)), expected);

        // The same order `Vec<PathBuf>::sort` gives, which is how a Rojo
        // project's mount files are ordered: one order for every file.
        let mut sorted = walked.clone();
        sorted.sort();
        assert_eq!(walked, sorted);

        let _ = fs::remove_dir_all(&dir);
    }
}
