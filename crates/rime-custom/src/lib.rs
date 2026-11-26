//! Custom project rules (`rime-rules/*.luau`). The contract rule authors
//! write against is `examples/rime-rules/README.md`.
//!
//! One sandboxed Luau VM per `rime check`; rules loaded once, alphabetically;
//! per file, the HIR is bridged to Lua tables and every rule whose `files`
//! glob matches runs `check(file, ctx)`. Nothing here can abort the run: a
//! broken or throwing rule is an R0025 on its own file.

pub mod bridge;
pub mod contract;
pub mod ctx;
pub mod stdlib;

use contract::Rule;
use mlua::{Lua, Value};
use rime_diag::Diagnostic;
use std::path::{Path, PathBuf};

/// A diagnostic anchored on a rule file, with that file's text for rendering.
pub struct RuleDiag {
    pub path: PathBuf,
    pub src: String,
    pub diagnostic: Diagnostic,
}

pub struct LoadReport {
    /// None when `rime-rules/` is absent or the VM could not start.
    pub engine: Option<Engine>,
    pub diagnostics: Vec<RuleDiag>,
}

pub struct Engine {
    lua: Lua,
    rules: Vec<Rule>,
    throws: Vec<Throw>,
}

/// One rule's throw, merged across every file it threw on at the same span.
/// A rule that throws does so on nearly every file it matches — one bug, so
/// one report: `finish` turns each entry into a single R0025.
struct Throw {
    /// Index into `Engine::rules`.
    rule: usize,
    /// Line of the rule file the error pointed at (the collapse key).
    span: rime_diag::Span,
    /// The full error text of the first occurrence.
    first_error: String,
    /// Project-relative paths, in the order they threw.
    files: Vec<String>,
}

#[derive(Default)]
pub struct FileOut {
    /// `ctx:fail` results carrying the rule's code and severity.
    pub diagnostics: Vec<Diagnostic>,
    /// (index into `rule_ids()`, wall time) per rule that ran on this file.
    pub timings: Vec<(usize, std::time::Duration)>,
    /// R0025s for internal failures (the bridge, the `file` or `ctx` table)
    /// while checking this file, anchored on a rule file. A rule that *threw*
    /// is not here — those collapse into `Engine::finish`.
    pub rule_errors: Vec<RuleDiag>,
}

/// One R0025 blaming `rule` for an internal failure while checking
/// `rel_path` — every failure reports somewhere, never silently (consistent
/// with the bridge-failure branch in `check_file`).
fn internal_error(rule: &Rule, rel_path: &str, what: &str, err: &dyn std::fmt::Display) -> RuleDiag {
    let d = contract::error_diag(&rule.id, &format!("internal: could not build {what} for {rel_path}: {err}"), 0..0);
    RuleDiag { path: rule.path.clone(), src: rule.src.clone(), diagnostic: d }
}

fn new_vm() -> mlua::Result<Lua> {
    let lua = Lua::new();
    let globals = lua.globals();
    globals.set("require", Value::Nil)?;
    globals.set("os", Value::Nil)?;
    globals.set("io", Value::Nil)?;
    stdlib::install(&lua)?;
    // Globals become read-only from here: rules keep state in locals.
    lua.sandbox(true)?;
    Ok(lua)
}

impl Engine {
    /// Discover and load `<project_root>/rime-rules/*.luau` (`.lua` too),
    /// sorted by file name.
    pub fn load(project_root: &Path) -> LoadReport {
        let dir = project_root.join("rime-rules");
        if !dir.is_dir() {
            return LoadReport { engine: None, diagnostics: Vec::new() };
        }
        let mut files: Vec<PathBuf> = match std::fs::read_dir(&dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|x| x == "luau" || x == "lua"))
                .collect(),
            Err(e) => {
                // A directory that exists but cannot be listed is not "no
                // rules": report it rather than silently checking nothing.
                let d = contract::error_diag("rime-rules", &format!("could not read the rime-rules directory: {e}"), 0..0);
                return LoadReport {
                    engine: None,
                    diagnostics: vec![RuleDiag { path: dir, src: String::new(), diagnostic: d }],
                };
            }
        };
        files.sort();

        let lua = match new_vm() {
            Ok(lua) => lua,
            Err(e) => {
                let d = contract::error_diag("rime-rules", &format!("could not start the rule VM: {e}"), 0..0);
                return LoadReport {
                    engine: None,
                    diagnostics: vec![RuleDiag { path: dir, src: String::new(), diagnostic: d }],
                };
            }
        };
        let mut rules: Vec<Rule> = Vec::new();
        let mut diagnostics = Vec::new();
        for path in files {
            let src = match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(e) => {
                    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    // A broken rule still reserves its default (stem) id so
                    // `[rules] <stem> = ...` in rime.toml resolves instead of
                    // aborting config parsing with "unknown rule" — the user
                    // sees this R0025 instead. An explicit `id` inside a file
                    // that can't even be read is unknowable, so that case
                    // stays an unknown-rule config error.
                    // Never for a core-looking stem: `R0301.luau` or
                    // `unused-local.luau` must not hijack the core key.
                    if !contract::looks_like_core_code(&stem) && !contract::is_core_name(&stem) {
                        rime_diag::register_custom(&stem);
                    }
                    let d = contract::error_diag(&stem, &format!("could not read the rule file: {e}"), 0..0);
                    diagnostics.push(RuleDiag { path, src: String::new(), diagnostic: d });
                    continue;
                }
            };
            match contract::load_rule(&lua, &path, &src, &rules) {
                Ok(rule) => rules.push(rule),
                Err(d) => {
                    // Same reservation as above, for the far more common
                    // case: a rule file that reads fine but fails contract
                    // validation (missing `check`, bad `files`, a throw
                    // while evaluating the chunk, ...).
                    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    // Same core-key guard as above: a file named `R0301.luau`
                    // or `unused-local.luau` must not hijack the core key.
                    if !contract::looks_like_core_code(&stem) && !contract::is_core_name(&stem) {
                        rime_diag::register_custom(&stem);
                    }
                    diagnostics.push(RuleDiag { path, src, diagnostic: d })
                }
            }
        }
        LoadReport { engine: Some(Engine { lua, rules, throws: Vec::new() }), diagnostics }
    }

    /// Rule ids in load order.
    pub fn rule_ids(&self) -> Vec<&str> {
        self.rules.iter().map(|r| r.id.as_str()).collect()
    }

    /// Each rule's diagnostic code, in load order.
    pub fn rule_codes(&self) -> Vec<rime_diag::Code> {
        self.rules.iter().map(|r| r.code).collect()
    }

    /// Run every rule whose `files` glob matches `rel_path`. Files no rule
    /// matches cost nothing: the HIR is bridged only when needed.
    pub fn check_file(&mut self, rel_path: &str, module: &rime_hir::Module, src: &str) -> FileOut {
        let mut out = FileOut::default();
        let matching: Vec<usize> = (0..self.rules.len())
            .filter(|&i| self.rules[i].globs.is_match(rel_path))
            .collect();
        if matching.is_empty() {
            return out;
        }
        let root = match bridge::module_tree(&self.lua, module) {
            Ok(t) => t,
            Err(e) => {
                // Not a rule's fault; blame the first matching rule's file so
                // the failure is visible somewhere, never silent.
                let rule = &self.rules[matching[0]];
                out.rule_errors.push(internal_error(rule, rel_path, "the node tree", &e));
                return out;
            }
        };
        let file = match ctx::make_file(&self.lua, rel_path, src, root) {
            Ok(f) => f,
            Err(e) => {
                let rule = &self.rules[matching[0]];
                out.rule_errors.push(internal_error(rule, rel_path, "the file table", &e));
                return out;
            }
        };
        for i in matching {
            let rule = &self.rules[i];
            let sink = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
            let ctx = match ctx::make_ctx(&self.lua, src, sink.clone()) {
                Ok(c) => c,
                Err(e) => {
                    out.rule_errors.push(internal_error(rule, rel_path, "ctx", &e));
                    continue;
                }
            };
            let start = std::time::Instant::now();
            let result = rule.check.call::<()>((file.clone(), ctx));
            out.timings.push((i, start.elapsed()));
            match result {
                Ok(()) => {
                    for f in sink.take() {
                        let mut d = match rule.severity {
                            rime_diag::Severity::Error => Diagnostic::error(rule.code, f.message.clone()),
                            rime_diag::Severity::Warning => Diagnostic::warning(rule.code, f.message.clone()),
                        }
                        .with_label(rime_diag::Label::primary(f.span, f.message));
                        if let Some(n) = f.note {
                            d = d.with_note(n);
                        }
                        out.diagnostics.push(d);
                    }
                }
                Err(e) => {
                    // Recorded, not reported: `finish` collapses a rule's
                    // throws to one R0025 per (rule, error line) for the whole
                    // run. Any `ctx:fail` this run made is dropped with the
                    // sink — a run that threw did not finish its judgement.
                    let line = contract::lua_error_line(&e);
                    let src = &self.rules[i].src;
                    let span = line.map(|l| contract::line_span(src, l)).unwrap_or(0..src.len());
                    match self.throws.iter_mut().find(|t| t.rule == i && t.span == span) {
                        Some(t) => t.files.push(rel_path.to_string()),
                        None => self.throws.push(Throw {
                            rule: i,
                            span,
                            first_error: e.to_string(),
                            files: vec![rel_path.to_string()],
                        }),
                    }
                }
            }
        }
        out
    }

    /// Drain the collapsed throw reports: one R0025 per (rule, error line)
    /// over the whole run, in rule order then span order. The CLI calls this
    /// once after the last `check_file` — a throwing rule produces nothing
    /// until then, so that a rule broken on every file is one diagnostic
    /// naming the first file, not thousands of copies of one bug.
    pub fn finish(&mut self) -> Vec<RuleDiag> {
        let mut throws = std::mem::take(&mut self.throws);
        throws.sort_by(|a, b| {
            a.rule.cmp(&b.rule).then(a.span.start.cmp(&b.span.start)).then(a.span.end.cmp(&b.span.end))
        });
        throws
            .into_iter()
            .map(|t| {
                let rule = &self.rules[t.rule];
                let first = t.first_error.lines().next().unwrap_or_default();
                let mut d = contract::error_diag(
                    &rule.id,
                    &format!("threw while checking {}: {first}", t.files[0]),
                    t.span.clone(),
                )
                .with_note(t.first_error.clone());
                if t.files.len() > 1 {
                    let shown = t.files.iter().take(5).cloned().collect::<Vec<_>>().join(", ");
                    let extra = t.files.len().saturating_sub(5);
                    let more = if extra > 0 { format!(", … (+{extra} more)") } else { String::new() };
                    d = d.with_note(format!("threw on {} files: {shown}{more}", t.files.len()));
                }
                RuleDiag { path: rule.path.clone(), src: rule.src.clone(), diagnostic: d }
            })
            .collect()
    }
}

#[cfg(test)]
mod load_tests {
    use super::*;

    fn temp_project(tag: &str, rules: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rime-custom-load-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("rime-rules")).unwrap();
        for (name, src) in rules {
            std::fs::write(dir.join("rime-rules").join(name), src).unwrap();
        }
        dir
    }

    #[test]
    fn absent_dir_means_no_engine_and_no_diagnostics() {
        let dir = std::env::temp_dir().join(format!("rime-custom-none-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let report = Engine::load(&dir);
        assert!(report.engine.is_none() && report.diagnostics.is_empty());
    }

    #[test]
    fn loads_in_name_order_and_keeps_going_past_a_broken_rule() {
        let dir = temp_project("order", &[
            ("b-second.luau", "return { files = \"*\", check = function() end }"),
            ("a-first.luau", "return { files = \"*\", check = function() end }"),
            ("c-broken.luau", "return { files = \"*\" }"),
        ]);
        let report = Engine::load(&dir);
        let engine = report.engine.expect("engine");
        assert_eq!(engine.rule_ids(), vec!["a-first", "b-second"]);
        assert_eq!(report.diagnostics.len(), 1);
        let d = &report.diagnostics[0];
        assert!(d.path.ends_with("c-broken.luau"));
        assert_eq!(d.diagnostic.code, rime_diag::Code(25));
        assert!(d.diagnostic.message.contains("`check`"), "{}", d.diagnostic.message);
        assert!(rime_diag::custom_code("c-broken").is_some(), "a broken rule still registers its stem");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sandbox_hides_require_os_io_and_reports_load_errors() {
        let dir = temp_project("sandbox", &[
            ("g.luau", "assert(require == nil and os == nil and io == nil, \"sandbox leaks a global\")\nreturn { files = \"*\", check = function() end }"),
            ("h.luau", "error(\"nope\")"),
        ]);
        let report = Engine::load(&dir);
        let engine = report.engine.expect("engine");
        assert_eq!(engine.rule_ids(), vec!["g"], "g loads cleanly: no require/os/io visible");
        assert_eq!(report.diagnostics.len(), 1);
        let d = &report.diagnostics[0];
        assert!(d.path.ends_with("h.luau"));
        assert_eq!(d.diagnostic.code, rime_diag::Code(25));
        assert!(d.diagnostic.message.contains("nope"), "{}", d.diagnostic.message);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn check_file_runs_matching_rules_and_isolates_a_throwing_one() {
        let dir = temp_project("check", &[
            ("a-flag-prints.luau", "return { severity = \"warn\", files = \"src/**/*.lua\", check = function(file, ctx) for _, c in rime.calls(file.root) do if rime.callee_name(c) == \"print\" then ctx:fail(c, \"no print\") end end end }"),
            ("b-throws.luau", "return { files = \"src/**/*.lua\", check = function(file, ctx) error(\"boom\") end }"),
            ("c-other-files.luau", "return { files = \"lib/**\", check = function(file, ctx) ctx:fail(file.root, \"never\") end }"),
        ]);
        let mut engine = Engine::load(&dir).engine.unwrap();
        let module = rime_syntax::parse("print(1)\nprint(2)\n").unwrap();
        let out = engine.check_file("src/m.lua", &module, "print(1)\nprint(2)\n");
        assert_eq!(out.diagnostics.len(), 2, "one per occurrence");
        let d = &out.diagnostics[0];
        assert_eq!(d.code, rime_diag::custom_code("a-flag-prints").unwrap());
        assert_eq!(d.severity, rime_diag::Severity::Warning);
        assert_eq!(d.message, "no print");
        assert_eq!(d.labels[0].span, 0..8);
        assert!(out.rule_errors.is_empty(), "a throw is held for `finish`, not reported per file");
        assert_eq!(out.timings.len(), 2, "two rules ran, the third did not match");
        let none = engine.check_file("docs/readme.md", &module, "");
        assert!(none.diagnostics.is_empty() && none.timings.is_empty());
        let throws = engine.finish();
        assert_eq!(throws.len(), 1);
        let e = &throws[0];
        assert!(e.path.ends_with("b-throws.luau"));
        assert_eq!(e.diagnostic.code, rime_diag::Code(25));
        assert!(e.diagnostic.message.contains("b-throws") && e.diagnostic.message.contains("src/m.lua"), "{}", e.diagnostic.message);
        assert!(e.diagnostic.notes.iter().any(|n| n.contains("boom")));
        assert!(engine.finish().is_empty(), "finish drains");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn throwing_rule_collapses_across_files() {
        let dir = temp_project("collapse", &[(
            "b-throws.luau",
            "return { files = \"src/**/*.lua\", check = function(file, ctx) error(\"boom\") end }",
        )]);
        let mut engine = Engine::load(&dir).engine.unwrap();
        let module = rime_syntax::parse("print(1)\n").unwrap();
        for rel in ["src/a.lua", "src/b.lua"] {
            let out = engine.check_file(rel, &module, "print(1)\n");
            assert!(out.rule_errors.is_empty());
        }
        let throws = engine.finish();
        assert_eq!(throws.len(), 1, "one bug, one report");
        let d = &throws[0].diagnostic;
        assert!(d.message.contains("src/a.lua"), "the message names the first file: {}", d.message);
        assert!(
            d.notes.iter().any(|n| n.contains("threw on 2 files: src/a.lua, src/b.lua")),
            "{:?}",
            d.notes
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn core_looking_stems_do_not_register() {
        let dir = temp_project("core-stems", &[("R0301.luau", "return 1"), ("unused-local.luau", "return 1")]);
        let report = Engine::load(&dir);
        assert_eq!(report.diagnostics.len(), 2, "both files are broken rules");
        assert!(report.diagnostics.iter().all(|d| d.diagnostic.code == rime_diag::Code(25)));
        assert!(rime_diag::custom_code("R0301").is_none(), "a core-looking stem must not take the raw code key");
        assert!(rime_diag::custom_code("unused-local").is_none(), "a core rule name must not be hijacked");
        std::fs::remove_dir_all(&dir).ok();
    }
}
