//! Which files a run is about: the source files under each root, minus
//! what git ignores.
//!
//! A project's git-ignored directories are where everything that is not
//! the project lives — build output, package caches, other checkouts.
//! `rime check .` on a large codebase walked many times as many files in
//! ignored agent worktrees as the project has, and reported on all of them.
//!
//! The rules, which follow git and ripgrep:
//!
//!   - `.gitignore` files (the directory's own and every one above it, up
//!     to the top of the work tree), `.git/info/exclude`, and the user's
//!     global excludes file, with git's precedence: the nearest
//!     `.gitignore` wins, a `!pattern` re-includes.
//!   - Only inside a git work tree. With no `.git` above a directory, a
//!     `.gitignore` in it is just a file.
//!   - A root named on the command line is always walked, ignored or not;
//!     the rules apply to what is found beneath it.
//!   - A directory with its own `.git` (a submodule, a linked worktree)
//!     starts again from its own rules, as git does.
//!   - It is patterns only: a file that matches is skipped even when it is
//!     tracked (committed before the pattern was added). `git` itself would
//!     still list that one.
//!
//! Rojo mounts are the exception to "skipped". A mounted directory can be
//! git-ignored (wally's `Packages/`) and the code that is checked still
//! requires into it, so files under a mount are always listed — marked as
//! not asked for: read and resolved against, never reported on. That is
//! what they are when a run names `src` instead of `.`.
//!
//! Same walker underneath as everything else (`rime_check::walk::list_dir`),
//! same pre-order, so with nothing ignored this lists exactly what
//! `walk::sources` lists.

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use ignore::Match;
use rayon::prelude::*;
use rime_check::walk::{self, Item};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct Discovery {
    /// Off: every source file under a root is listed (`--no-gitignore`).
    gitignore: bool,
    /// Canonical bases of the project's Rojo mounts.
    mounts: Vec<PathBuf>,
    /// The user's global excludes file (`core.excludesFile`), if there is one.
    global: Option<PathBuf>,
}

/// One root's listing.
#[derive(Default)]
pub struct Found {
    /// The files the run is about, in walk order.
    pub files: Vec<PathBuf>,
    /// `files` plus the git-ignored files that sit under a Rojo mount, in
    /// the same order: under every mount, what an unfiltered walk lists.
    pub with_mounts: Vec<PathBuf>,
    /// Git-ignored entries left out of `files`: a file, or a directory
    /// (counted once, whatever it holds).
    pub git_ignored: usize,
}

/// The ignore files in force in one directory, nearest first.
#[derive(Clone, Default)]
struct Rules(Option<Arc<Layer>>);

struct Layer {
    /// The directory the patterns are relative to.
    dir: PathBuf,
    matcher: Gitignore,
    /// What applies when this layer has nothing to say about a path.
    below: Rules,
}

impl Rules {
    /// These rules with `file`'s patterns, anchored at `dir`, on top.
    fn with_file(&self, dir: &Path, file: &Path) -> Rules {
        let mut builder = GitignoreBuilder::new(dir);
        // git matches case-insensitively where the filesystem does
        // (`core.ignorecase`, set at init/clone on Windows).
        let _ = builder.case_insensitive(cfg!(windows));
        // A line git cannot use, git skips; so does `add`, which hands back
        // what it skipped. An unreadable file adds nothing.
        let _ = builder.add(file);
        match builder.build() {
            Ok(matcher) if !matcher.is_empty() => {
                Rules(Some(Arc::new(Layer { dir: dir.to_path_buf(), matcher, below: self.clone() })))
            }
            _ => self.clone(),
        }
    }

    /// Whether git ignores this entry, given that it does not ignore the
    /// directory it is in.
    fn ignores(&self, path: &Path, is_dir: bool) -> bool {
        let mut layer = self.0.as_deref();
        while let Some(l) = layer {
            let relative = path.strip_prefix(&l.dir).unwrap_or(path);
            match l.matcher.matched(relative, is_dir) {
                Match::Ignore(_) => return true,
                Match::Whitelist(_) => return false,
                Match::None => layer = l.below.0.as_deref(),
            }
        }
        false
    }
}

/// `<git dir>/info/exclude` for the work tree whose top is `top`. In a
/// linked worktree or a submodule `.git` is a file naming the git dir, and
/// a linked worktree's `info/` lives in the main repository's (`commondir`).
fn info_exclude(top: &Path) -> Option<PathBuf> {
    let dot_git = top.join(".git");
    let git_dir = if dot_git.is_dir() {
        dot_git
    } else {
        let text = std::fs::read_to_string(&dot_git).ok()?;
        let named = text.lines().find_map(|line| line.strip_prefix("gitdir:"))?.trim();
        let git_dir = resolve(top, named);
        match std::fs::read_to_string(git_dir.join("commondir")) {
            Ok(common) => resolve(&git_dir, common.trim()),
            Err(_) => git_dir,
        }
    };
    let exclude = git_dir.join("info").join("exclude");
    exclude.is_file().then_some(exclude)
}

/// `base` plus a path as git writes one (`/`-separated, possibly with
/// `..`, possibly absolute), resolved lexically. `Path::join` is not
/// enough: `base` is canonical, which on Windows is a verbatim path
/// (`\\?\C:\…`), and there the OS takes `/` and `..` as literal names.
fn resolve(base: &Path, text: &str) -> PathBuf {
    if Path::new(text).is_absolute() {
        return PathBuf::from(text);
    }
    let mut out = base.to_path_buf();
    for part in text.split(|c| c == '/' || (cfg!(windows) && c == '\\')) {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            name => out.push(name),
        }
    }
    out
}

/// `files` in order, with the hidden ones (git-ignored, under a mount)
/// flagged.
#[derive(Default)]
struct Walked {
    files: Vec<(PathBuf, bool)>,
    git_ignored: usize,
}

impl Walked {
    fn merge(parts: Vec<Walked>) -> Walked {
        let mut out = Walked::default();
        for part in parts {
            out.files.extend(part.files);
            out.git_ignored += part.git_ignored;
        }
        out
    }
}

impl Discovery {
    pub fn new(gitignore: bool, mounts: Vec<PathBuf>) -> Discovery {
        let global = if gitignore {
            ignore::gitignore::gitconfig_excludes_path().filter(|path| path.is_file())
        } else {
            None
        };
        Discovery::with_global(gitignore, mounts, global)
    }

    /// `new`, with the global excludes file given instead of looked up in
    /// the user's git config.
    fn with_global(gitignore: bool, mounts: Vec<PathBuf>, global: Option<PathBuf>) -> Discovery {
        Discovery { gitignore, mounts, global }
    }

    /// The source files under `root` (canonical), or `root` itself when it
    /// is one.
    pub fn sources(&self, root: &Path) -> Found {
        if !self.gitignore || !root.is_dir() {
            // A file named outright is taken as it is, like any root.
            let files = walk::sources(root);
            return Found { with_mounts: files.clone(), files, git_ignored: 0 };
        }
        let above = self.rules_above(root);
        let walked = self.walk_dir(root, above.as_ref());
        let mut found = Found { git_ignored: walked.git_ignored, ..Found::default() };
        for (path, hidden) in walked.files {
            if !hidden {
                found.files.push(path.clone());
            }
            found.with_mounts.push(path);
        }
        found
    }

    /// The rules of the work tree whose top is `top`, before any
    /// `.gitignore`: the two files that apply to the whole tree, the
    /// global one weakest.
    fn repo_rules(&self, top: &Path) -> Rules {
        let mut rules = Rules::default();
        if let Some(global) = &self.global {
            rules = rules.with_file(top, global);
        }
        if let Some(exclude) = info_exclude(top) {
            rules = rules.with_file(top, &exclude);
        }
        rules
    }

    /// The rules in force in `dir`'s parent, or `None` when `dir` is not
    /// inside a git work tree (it may still be the top of one itself —
    /// `walk_dir` sees that from its listing).
    fn rules_above(&self, dir: &Path) -> Option<Rules> {
        let top = dir.ancestors().skip(1).find(|d| d.join(".git").exists())?;
        let mut chain: Vec<&Path> = dir.ancestors().skip(1).take_while(|d| *d != top).collect();
        chain.push(top);
        let mut rules = self.repo_rules(top);
        for above in chain.into_iter().rev() {
            let file = above.join(".gitignore");
            if file.is_file() {
                rules = rules.with_file(above, &file);
            }
        }
        Some(rules)
    }

    fn under_mount(&self, path: &Path) -> bool {
        self.mounts.iter().any(|base| path.starts_with(base))
    }

    /// `inherited` is what applies in the parent directory; `None` outside
    /// any work tree, where nothing is ignored.
    fn walk_dir(&self, dir: &Path, inherited: Option<&Rules>) -> Walked {
        let listing = walk::list_dir(dir);
        let mut rules = if listing.has_git { Some(self.repo_rules(dir)) } else { inherited.cloned() };
        if listing.has_gitignore {
            if let Some(rules) = rules.as_mut() {
                *rules = rules.with_file(dir, &dir.join(".gitignore"));
            }
        }
        let rules = rules.as_ref();
        let ignored = |path: &Path, is_dir: bool| rules.is_some_and(|r| r.ignores(path, is_dir));
        // Subdirectories walk on the pool; results splice back at the
        // position the entry had in the listing, preserving pre-order.
        let parts: Vec<Walked> = listing
            .items
            .into_par_iter()
            .map(|item| match item {
                Item::File(path) if ignored(&path, false) => {
                    let files = if self.under_mount(&path) { vec![(path, true)] } else { Vec::new() };
                    Walked { files, git_ignored: 1 }
                }
                Item::File(path) => Walked { files: vec![(path, false)], git_ignored: 0 },
                Item::Dir(path) if ignored(&path, true) => {
                    Walked { files: self.walk_ignored(&path), git_ignored: 1 }
                }
                Item::Dir(path) => self.walk_dir(&path, rules),
            })
            .collect();
        Walked::merge(parts)
    }

    /// Inside a git-ignored directory. Nothing in it is checked; it is
    /// entered at all only on the way to, or inside, a Rojo mount, whose
    /// files are listed as hidden.
    fn walk_ignored(&self, dir: &Path) -> Vec<(PathBuf, bool)> {
        if !self.mounts.iter().any(|base| base.starts_with(dir) || dir.starts_with(base)) {
            return Vec::new();
        }
        walk::list_dir(dir)
            .items
            .into_par_iter()
            .map(|item| match item {
                Item::File(path) if self.under_mount(&path) => vec![(path, true)],
                Item::File(_) => Vec::new(),
                Item::Dir(path) => self.walk_ignored(&path),
            })
            .collect::<Vec<_>>()
            .into_iter()
            .flatten()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Tree {
        root: PathBuf,
    }

    impl Tree {
        fn new(name: &str) -> Tree {
            let dir = std::env::temp_dir().join(format!("rime-discover-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Tree { root: dir.canonicalize().unwrap() }
        }

        /// `rel` (`/`-separated) under the root. Component by component:
        /// the root is canonical, and a verbatim Windows path takes a `/`
        /// inside a joined string as part of a name.
        fn at(&self, rel: &str) -> PathBuf {
            rel.split('/').filter(|part| !part.is_empty()).fold(self.root.clone(), |path, part| path.join(part))
        }

        fn write(&self, rel: &str, text: &str) -> &Tree {
            let path = self.at(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
            self
        }

        /// Source files at these paths, empty.
        fn sources(&self, rels: &[&str]) -> &Tree {
            for rel in rels {
                self.write(rel, "");
            }
            self
        }

        /// Make `rel` the top of a git work tree, the cheap way: git and
        /// this walker both go by the presence of `.git`.
        fn git(&self, rel: &str) -> &Tree {
            fs::create_dir_all(self.at(rel).join(".git").join("info")).unwrap();
            self
        }

        /// No global excludes file: the tests must not depend on the git
        /// configuration of whoever runs them.
        fn discovery(&self, mounts: &[&str]) -> Discovery {
            Discovery::with_global(true, mounts.iter().map(|m| self.at(m)).collect(), None)
        }

        fn rel(&self, paths: &[PathBuf]) -> Vec<String> {
            let mut out: Vec<String> = paths
                .iter()
                .map(|p| p.strip_prefix(&self.root).unwrap().to_string_lossy().replace('\\', "/"))
                .collect();
            out.sort();
            out
        }

        fn found(&self, sub: &str, mounts: &[&str]) -> (Vec<String>, Vec<String>, usize) {
            let found = self.discovery(mounts).sources(&self.at(sub));
            (self.rel(&found.files), self.rel(&found.with_mounts), found.git_ignored)
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn nothing_ignored_lists_exactly_what_the_plain_walk_lists() {
        let t = Tree::new("parity");
        t.git("").sources(&["a.lua", "src/b.luau", "src/deep/c.lua", "z/y/x.luau"]).write("src/notes.txt", "");
        let plain = walk::sources(&t.root);
        let found = t.discovery(&[]).sources(&t.root);
        assert_eq!(found.files, plain, "same files, same order");
        assert_eq!(found.with_mounts, plain);
        assert_eq!(found.git_ignored, 0);
    }

    #[test]
    fn switched_off_it_is_the_plain_walk() {
        let t = Tree::new("off");
        t.git("").write(".gitignore", "ignored.lua\nout/\n").sources(&["ignored.lua", "out/o.lua", "a.lua"]);
        let found = Discovery::new(false, Vec::new()).sources(&t.root);
        assert_eq!(found.files, walk::sources(&t.root));
        assert_eq!(found.files.len(), 3);
        assert_eq!(found.git_ignored, 0);
    }

    #[test]
    fn the_global_excludes_file_applies_and_is_the_weakest() {
        let t = Tree::new("global");
        t.write("home/ignore", "*.bak.lua\nnotes.lua\n")
            .git("repo")
            .write("repo/.gitignore", "!notes.lua\n")
            .sources(&["repo/a.lua", "repo/a.bak.lua", "repo/notes.lua"]);
        let found = Discovery::with_global(true, Vec::new(), Some(t.at("home/ignore"))).sources(&t.at("repo"));
        assert_eq!(t.rel(&found.files), vec!["repo/a.lua", "repo/notes.lua"]);
    }

    #[test]
    fn gitignore_patterns_follow_git() {
        let t = Tree::new("patterns");
        t.git("")
            .write(
                ".gitignore",
                "# a comment\nbuild\n/rooted.lua\n*.gen.lua\nscratch/\n!scratch.keep.gen.lua\n/.agents/\n",
            )
            .sources(&[
                "kept.lua",
                "rooted.lua",          // `/rooted.lua` — only at the top
                "src/rooted.lua",      // not this one
                "build/out.lua",       // `build` — a directory at any depth
                "src/build/out.lua",
                "src/a.gen.lua",       // `*.gen.lua` — a file at any depth
                "src/scratch/s.lua",   // `scratch/` — directories only
                "src/scratch.lua",     // so not a file that merely starts like it
                "scratch.keep.gen.lua", // re-included by the `!` line
                ".agents/worktrees/copy/src/x.lua",
                "src/.agents/x.lua",   // `/.agents/` is rooted
            ]);
        let (files, with_mounts, ignored) = t.found("", &[]);
        assert_eq!(
            files,
            vec!["kept.lua", "scratch.keep.gen.lua", "src/.agents/x.lua", "src/rooted.lua", "src/scratch.lua"]
        );
        assert_eq!(with_mounts, files, "no mounts, nothing hidden");
        // rooted.lua, build, src/build, a.gen.lua, src/scratch, .agents:
        // a directory counts once.
        assert_eq!(ignored, 6);
    }

    #[test]
    fn a_nearer_gitignore_wins_and_applies_only_below_itself() {
        let t = Tree::new("nested");
        t.git("")
            .write(".gitignore", "*.tmp.lua\ngenerated/\n")
            .write("pkg/.gitignore", "!keep.tmp.lua\nlocal.lua\n!generated/\n")
            .sources(&[
                "a.tmp.lua",
                "local.lua",
                "generated/g.lua",
                "pkg/a.tmp.lua",
                "pkg/keep.tmp.lua",
                "pkg/local.lua",
                "pkg/generated/g.lua",
                "pkg/sub/local.lua",
            ]);
        let (files, _, _) = t.found("", &[]);
        assert_eq!(files, vec!["local.lua", "pkg/generated/g.lua", "pkg/keep.tmp.lua"]);
    }

    #[test]
    fn info_exclude_applies_and_gitignore_can_override_it() {
        let t = Tree::new("exclude");
        t.git("")
            .write(".git/info/exclude", "**/.claude/worktrees/\nprivate.lua\nshared.lua\n")
            .write(".gitignore", "!shared.lua\n")
            .sources(&["a.lua", "private.lua", "shared.lua", ".claude/worktrees/w/src/a.lua", ".claude/kept.lua"]);
        let (files, _, _) = t.found("", &[]);
        assert_eq!(files, vec![".claude/kept.lua", "a.lua", "shared.lua"]);
    }

    #[test]
    fn a_gitignore_outside_any_work_tree_is_just_a_file() {
        let t = Tree::new("nogit");
        t.write(".gitignore", "ignored.lua\n").sources(&["ignored.lua", "a.lua"]);
        let (files, _, ignored) = t.found("", &[]);
        assert_eq!(files, vec!["a.lua", "ignored.lua"]);
        assert_eq!(ignored, 0);
    }

    #[test]
    fn a_root_below_the_top_is_under_the_gitignores_above_it() {
        let t = Tree::new("subroot");
        t.git("")
            .write(".gitignore", "*.gen.lua\n/src/vendor/\n")
            .write("src/.gitignore", "tmp/\n")
            .sources(&["src/game/a.lua", "src/game/a.gen.lua", "src/game/tmp/t.lua", "src/vendor/v.lua"]);
        // Walking `src/game`: the top's and `src`'s rules both reach it.
        let (files, _, _) = t.found("src/game", &[]);
        assert_eq!(files, vec!["src/game/a.lua"]);
        // The root itself is never tested: naming an ignored directory
        // walks it (what is beneath it is still filtered).
        t.sources(&["src/vendor/w.gen.lua"]);
        let (files, _, _) = t.found("src/vendor", &[]);
        assert_eq!(files, vec!["src/vendor/v.lua"]);
    }

    #[test]
    fn a_nested_work_tree_goes_by_its_own_rules() {
        let t = Tree::new("nestedrepo");
        t.git("")
            .write(".gitignore", "*.gen.lua\n")
            .git("sub")
            .write("sub/.gitignore", "private/\n")
            .sources(&["a.gen.lua", "sub/b.gen.lua", "sub/private/p.lua", "sub/ok.lua"]);
        let (files, _, _) = t.found("", &[]);
        // The outer `*.gen.lua` does not reach into `sub`; `sub`'s own
        // rules do apply there.
        assert_eq!(files, vec!["sub/b.gen.lua", "sub/ok.lua"]);
    }

    #[test]
    fn a_linked_worktree_reads_the_main_repositorys_info_exclude() {
        let t = Tree::new("worktree");
        // main/.git/worktrees/wt is the worktree's git dir; its
        // `commondir` points back at main/.git.
        t.git("main")
            .write("main/.git/info/exclude", "excluded.lua\n")
            .write("main/.git/worktrees/wt/commondir", "../..\n")
            .write("wt/.git", &format!("gitdir: {}\n", t.at("main/.git/worktrees/wt").display()))
            .sources(&["wt/excluded.lua", "wt/a.lua"]);
        let (files, _, _) = t.found("wt", &[]);
        assert_eq!(files, vec!["wt/a.lua"]);
    }

    #[test]
    fn files_under_a_mount_are_listed_but_hidden_when_git_ignores_them() {
        let t = Tree::new("mounts");
        t.git("")
            .write(".gitignore", "Packages\n/out/\n*.gen.lua\n")
            .sources(&[
                "src/a.lua",
                "src/a.gen.lua",              // ignored file inside a mount
                "Packages/_Index/pkg/init.lua", // ignored directory that IS a mount
                "Packages/Lib.lua",
                "out/deep/mounted/m.lua",     // a mount inside an ignored directory
                "out/deep/other/o.lua",       // its sibling: not under any mount
                "out/top.lua",
                "elsewhere/Packages/p.lua",   // ignored, no mount: gone entirely
            ]);
        let mounts = ["src", "Packages", "out/deep/mounted"];
        let (files, with_mounts, _) = t.found("", &mounts);
        assert_eq!(files, vec!["src/a.lua"], "only what git does not ignore is asked for");
        assert_eq!(
            with_mounts,
            vec![
                "Packages/Lib.lua",
                "Packages/_Index/pkg/init.lua",
                "out/deep/mounted/m.lua",
                "src/a.gen.lua",
                "src/a.lua",
            ],
            "every file under a mount is still listed"
        );
        // Under each mount, exactly what the plain walk lists there, in
        // the same order — `RojoProject::seed_walk` relies on it.
        let all = t.discovery(&mounts).sources(&t.root);
        for mount in mounts {
            let base = t.at(mount);
            let listed: Vec<&PathBuf> = all.with_mounts.iter().filter(|p| p.starts_with(&base)).collect();
            let plain = walk::sources(&base);
            assert_eq!(listed, plain.iter().collect::<Vec<_>>(), "mount {mount}");
        }
    }

    #[test]
    fn a_file_named_outright_is_taken_even_when_ignored() {
        let t = Tree::new("file");
        t.git("").write(".gitignore", "*.gen.lua\n").sources(&["a.gen.lua"]);
        let (files, _, ignored) = t.found("a.gen.lua", &[]);
        assert_eq!(files, vec!["a.gen.lua"]);
        assert_eq!(ignored, 0);
    }

    /// The oracle: a real repository, and git's own answer to "which
    /// files do you not ignore". Skipped (passes) where there is no `git`
    /// to ask.
    #[test]
    fn the_listing_matches_what_git_itself_does_not_ignore() {
        let t = Tree::new("oracle");
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&t.root)
                // Nothing from the machine's or the user's git config but
                // the global excludes file, which both sides read.
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
        };
        match git(&["init", "--quiet"]) {
            Ok(out) if out.status.success() => {}
            _ => {
                eprintln!("git is not available; skipping the comparison with git");
                return;
            }
        }
        t.write(
            ".gitignore",
            "# comment\n\nbuild\n/rooted.lua\n*.gen.lua\nscratch/\n!keep.gen.lua\n/.agents/\nPackages\n\
             doc/**/draft.lua\n**/cache/\nlogs/*.lua\n!logs/keep.lua\nsp ace.lua\n[Tt]emp/\n",
        )
        .write(".git/info/exclude", "excluded.lua\n**/.claude/worktrees/\nshared.lua\n")
        .write("pkg/.gitignore", "!a.gen.lua\nlocal.lua\n/only-here.lua\n!shared.lua\nvendor/*\n!vendor/keep.lua\n")
        .sources(&[
            "kept.lua",
            "rooted.lua",
            "src/rooted.lua",
            "build/out.lua",
            "src/build/out.lua",
            "src/a.gen.lua",
            "src/keep.gen.lua",
            "src/scratch/s.lua",
            "src/scratch.lua",
            ".agents/worktrees/copy/src/x.lua",
            "src/.agents/x.lua",
            "Packages/_Index/p/init.lua",
            "src/Packages/q.lua",
            "doc/draft.lua",
            "doc/a/draft.lua",
            "doc/a/b/draft.lua",
            "doc/a/final.lua",
            "src/cache/c.lua",
            "cache/c.lua",
            "logs/a.lua",
            "logs/keep.lua",
            "logs/sub/b.lua",
            "sp ace.lua",
            "Temp/t.lua",
            "temp/t.lua",
            "src/temp.lua",
            "excluded.lua",
            "src/excluded.lua",
            "shared.lua",
            ".claude/worktrees/w/a.lua",
            ".claude/other.lua",
            "pkg/a.gen.lua",
            "pkg/b.gen.lua",
            "pkg/local.lua",
            "pkg/sub/local.lua",
            "pkg/only-here.lua",
            "pkg/sub/only-here.lua",
            "pkg/shared.lua",
            "pkg/vendor/x.lua",
            "pkg/vendor/keep.lua",
            "pkg/vendor/deep/y.lua",
            "pkg/src/main.luau",
        ]);
        // Untracked files git does not ignore; nothing is tracked here, so
        // this is every file it does not ignore.
        let out = git(&["ls-files", "--others", "--exclude-standard", "-z"]).expect("git ls-files");
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let mut expected: Vec<String> = String::from_utf8(out.stdout)
            .unwrap()
            .split('\0')
            .filter(|p| p.ends_with(".lua") || p.ends_with(".luau"))
            .map(str::to_string)
            .collect();
        expected.sort();
        assert!(expected.len() > 5 && expected.len() < 30, "the fixture exercises both outcomes: {expected:?}");

        let found = Discovery::new(true, Vec::new()).sources(&t.root);
        assert_eq!(t.rel(&found.files), expected);
    }

    #[cfg(windows)]
    #[test]
    fn matching_is_case_insensitive_on_windows() {
        let t = Tree::new("case");
        t.git("").write(".gitignore", "packages\n").sources(&["Packages/p.lua", "a.lua"]);
        let (files, _, _) = t.found("", &[]);
        assert_eq!(files, vec!["a.lua"]);
    }
}
