//! Sibling-file require resolution for multi-file cases: `script.Parent.X`
//! resolves to `<case dir>/X.luau`. Shared by corpus.rs and session.rs.
#![allow(dead_code)]

use rime_check::project::{Loc, Resolution};
use std::path::Path;

pub struct SiblingResolver;

impl rime_check::project::Resolver for SiblingResolver {
    fn resolve(
        &self,
        from: &std::path::Path,
        target: &rime_hir::Expr,
        env: &std::collections::HashMap<String, &rime_hir::Expr>,
    ) -> rime_check::project::Resolution {
        // A static chain rooted at `game` is outside this resolver's view —
        // cases use it to exercise the silence path (041).
        fn rooted_at_game(e: &rime_hir::Expr) -> bool {
            match e {
                rime_hir::Expr::Name { name, .. } => name == "game",
                rime_hir::Expr::Index { base, .. } => rooted_at_game(base),
                _ => false,
            }
        }
        if rooted_at_game(target) {
            return Resolution::OutsideView;
        }
        // Accept `script.Parent.<A>.<B>...`, with one level of local-alias
        // substitution at the root (`local P = script.Parent;
        // require(P.dep)`) so the env plumbing has corpus coverage (051).
        // Nested segments map to subdirectories of the case dir (066/067).
        let is_script_parent = |e: &rime_hir::Expr| -> bool {
            let rime_hir::Expr::Index { base: inner, key: None, name: Some(parent), .. } = e else {
                return false;
            };
            parent == "Parent"
                && matches!(&**inner, rime_hir::Expr::Name { name, .. } if name == "script")
        };
        let mut segs: Vec<&str> = Vec::new();
        let mut cur = target;
        while let rime_hir::Expr::Index { base, key: None, name: Some(n), .. } = cur {
            // Stop before consuming `.Parent` — `script.Parent` is the
            // root, not a path segment.
            if is_script_parent(cur) {
                break;
            }
            segs.push(n);
            cur = base;
        }
        if segs.is_empty() {
            return Resolution::Dynamic;
        }
        segs.reverse();
        let root_ok = match cur {
            e if is_script_parent(e) => true,
            rime_hir::Expr::Name { name, .. } => {
                env.get(name).is_some_and(|init| is_script_parent(init))
            }
            _ => false,
        };
        if !root_ok {
            return Resolution::Dynamic;
        }
        let Some(dir) = from.parent() else {
            return Resolution::Dynamic;
        };
        let mut path = dir.to_path_buf();
        for s in &segs[..segs.len() - 1] {
            path = path.join(s);
        }
        let candidate = path.join(format!("{}.luau", segs.last().unwrap()));
        if candidate.is_file() {
            Resolution::Found(candidate)
        } else {
            Resolution::Missing { described: format!("script.Parent.{}", segs.join(".")) }
        }
    }

    /// Every `<name>.luau` under the case dir, as `script.Parent.…` path
    /// descriptions — the corpus stand-in for the project-wide module-name
    /// index the candidate note runs on.
    fn candidates(&self, from: &std::path::Path, name: &str) -> Vec<String> {
        fn scan(base: &Path, rel: &mut Vec<String>, name: &str, out: &mut Vec<String>) {
            let Ok(rd) = std::fs::read_dir(base) else {
                return;
            };
            for entry in rd.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    rel.push(p.file_name().unwrap().to_string_lossy().into_owned());
                    scan(&p, rel, name, out);
                    rel.pop();
                } else if p.extension().is_some_and(|x| x == "luau")
                    && p.file_stem().is_some_and(|s| s == name)
                {
                    let mut segs = rel.clone();
                    segs.push(name.to_string());
                    out.push(format!("script.Parent.{}", segs.join(".")));
                }
            }
        }
        let Some(dir) = from.parent() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        scan(dir, &mut Vec::new(), name, &mut out);
        out.sort();
        out
    }

    /// One instance step from the evaluator: `.Name` joins, `Parent`
    /// pops. Only `Loc::Fs` is ever produced by this resolver (`service`
    /// stays unimplemented — `game` never yields a real service here), so a
    /// `Loc::Dm` — never actually built for this corpus, but the trait shape
    /// allows it — has no meaning and just fails to step.
    fn step(&self, base: &Loc, seg: &str) -> Option<Loc> {
        let Loc::Fs(p) = base else { return None };
        if seg == "Parent" {
            Some(Loc::Fs(p.parent()?.to_path_buf()))
        } else {
            Some(Loc::Fs(p.join(seg)))
        }
    }

    /// The instance a script file IS: its own path, extension stripped
    /// (`Dir/Name.luau` -> `Dir/Name`) — the same `script.Parent.X` walk
    /// `resolve` above performs by hand, but through the evaluator's generic
    /// `Loc` machinery instead.
    fn instance_of(&self, file: &Path) -> Option<Loc> {
        Some(Loc::Fs(file.with_extension("")))
    }

    /// `Loc` -> file: re-attach `.luau` and check the disk, the evaluator's
    /// one on-disk resolution step. `OutsideView` for a non-`Fs` loc — same
    /// "not this resolver's view" answer `resolve` gives `game`-rooted
    /// chains above.
    fn resolve_loc(&self, loc: &Loc) -> Resolution {
        let Loc::Fs(inst) = loc else { return Resolution::OutsideView };
        let f = inst.with_extension("luau");
        if f.is_file() {
            Resolution::Found(f)
        } else {
            Resolution::Missing { described: display_rel(inst) }
        }
    }

    // `service`: default (None) — this resolver never mounts a DataModel,
    // so `game:GetService(...)` stays unresolvable and `game` stays
    // `OutsideView` via `resolve` above, exactly as before this task.
    // `context_of`: default (BOTH) — the corpus has no client/server split.
}

/// `Loc::Fs` -> `"script.Parent.X.Y..."` dialect, matching `resolve`'s own
/// `Missing::described` format — full dotted chain, not just the last
/// component. `resolve_loc`'s signature carries no `from`, so the case dir
/// can't be handed in directly; it's found instead as the nearest ancestor
/// of `inst` that itself contains a `main.luau` — every `Loc::Fs` this
/// resolver ever builds is anchored at a file living directly under some
/// case dir (`instance_of` strips the extension off a file there; further
/// `step` hops walk up/down from it), so that ancestor's `main.luau` marks
/// exactly the `script.Parent` root the missing chain's segments are
/// relative to. Deterministic and simple: one disk check per ancestor,
/// same `is_file()` idiom `resolve`/`resolve_loc` already use.
fn display_rel(inst: &Path) -> String {
    let mut dir = inst.parent();
    while let Some(d) = dir {
        if d.join("main.luau").is_file() {
            if let Ok(rel) = inst.strip_prefix(d) {
                let segs: Vec<_> =
                    rel.components().map(|c| c.as_os_str().to_string_lossy()).collect();
                if !segs.is_empty() {
                    return format!("script.Parent.{}", segs.join("."));
                }
            }
            break;
        }
        dir = d.parent();
    }
    // Defensive fallback — no case dir found on disk (or `inst` somehow
    // equals it): the last component alone, still names SOMETHING.
    format!("script.Parent.{}", inst.file_name().unwrap_or_default().to_string_lossy())
}
