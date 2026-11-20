//! R0312 module-scope-computation: a module does work as it loads.
//!
//! A module's top level runs once, on the first `require`, before the
//! requirer gets anything back. Work done there has no caller to ask for
//! it and no declared order: it delays every requirer, it runs in test
//! and tooling contexts that only wanted a constant, and what it reaches
//! for may not exist yet. The rule keeps the top level to declarations.
//!
//! Off by default: a project turns it on in rime.toml, and says there
//! what its modules may still do as they load.
//!
//! What is work:
//!
//!   - a call, unless its callee is allowed (below). One report per call,
//!     wherever the call sits in a top-level statement: a `local`
//!     initializer, an assignment, a table field, a condition, the final
//!     `return`, an argument of another call.
//!   - a loop (`for`, `while`, `repeat`), unless `allow_loops`. One report
//!     for the loop; nothing inside it is reported one by one, so one
//!     allow comment on the loop covers it.
//!
//! What is not: `local`s, assignments, table and other literals,
//! operators, indexing, `if` and `do` (what they hold is still looked
//! at), function definitions, type declarations, the `return`.
//!
//! Which calls are allowed, in the order they are asked:
//!
//!   1. `deny` — a callee that matches is reported, whatever else says.
//!   2. `allow` — the project's own patterns.
//!   3. `require` and `game:GetService` (`ALWAYS_ALLOWED`): how a module
//!      names what it depends on. Not a list to switch off; `deny` takes
//!      either out.
//!   4. the built-in lists in use (`allow_builtin`, both by default):
//!      `luau`, the language's own library of functions that work on the
//!      values they are given, and `roblox-datatypes`, the constructors
//!      of Roblox's value types (`Builtin::contains`). The types are read
//!      off the std files Rime bundles; the libraries are named by hand.
//!
//! 3 and 4 name globals, so they cover a call only when its first name is
//! not a local: after `local math = require(Fixed)`, `math.floor` is the
//! project's function, and it is `allow` that has to name it. A local
//! that holds the global of its own name (`local string = string`) and is
//! never written again is that global.
//!
//! A function body is not module scope: nothing in it runs until it is
//! called. A function literal that is called on the spot is looked into:
//! one handed to an allowed `pcall`, `xpcall`, `table.sort`,
//! `table.foreach`, `table.foreachi` or `string.gsub`
//! (`RUNS_ITS_ARGUMENT`). Any other call may keep the function for later.
//! A function literal called where it is written
//! (`(function() ... end)()`) is a call like any other and is reported as
//! one.
//!
//! What is a module: a file that returns a value from its top level, and
//! whose path matches none of the `scripts` and `exclude` patterns. A
//! file that returns no value cannot be required (Roblox refuses it), so
//! it is an entry script whatever it is called.
//!
//! The callee is matched as written (`game:GetService`,
//! `Players.PlayerAdded:Connect`, `Instance.new`), not by what it
//! resolves to: `local new = Instance.new; new("Part")` is the callee
//! `new`.

use crate::scope;
use rime_diag::{Code, Diagnostic, Label, Span};
use rime_hir as hir;
use rime_std::StdSource;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Allowed whatever `allow_builtin` says: the two calls a module makes to
/// name what it depends on.
pub const ALWAYS_ALLOWED: &[&str] = &["require", "game:GetService"];

/// `scripts` when rime.toml does not set it: the file names Rojo turns
/// into a script and not a ModuleScript (`default_sync_rules`, 7.7.1). A
/// project with `syncRules` of its own lists its patterns in `scripts`.
pub const DEFAULT_SCRIPTS: &[&str] = &[
    "*.server.lua",
    "*.server.luau",
    "*.client.lua",
    "*.client.luau",
    "*.plugin.lua",
    "*.plugin.luau",
];

/// A list of callees that ships with Rime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    /// `luau`: the Luau library functions that work on the values they are
    /// given, and the bare builtins of the same kind.
    Luau,
    /// `roblox-datatypes`: the constructors of Roblox's value types.
    RobloxDatatypes,
}

/// Luau libraries taken whole: any function of one of these is in `luau`,
/// unless `LUAU_FUNCTIONS_LEFT_OUT` names it. By library and not function
/// by function, so a function newer than the bundled std files
/// (`math.isnan`) is in. Picked by hand; the std files say what a library
/// holds, not what kind of thing it does.
pub const LUAU_LIBRARIES: &[&str] = &["bit32", "buffer", "math", "string", "table", "utf8", "vector"];

/// The Luau libraries left out, and why. With `LUAU_LIBRARIES` this is
/// every library in the bundled Luau std (a test holds it to that), so a
/// library a newer std file adds has to be put in one or the other.
pub const LUAU_LIBRARIES_LEFT_OUT: &[(&str, &str)] = &[
    ("coroutine", "starts, resumes and yields threads"),
    ("debug", "reads the call stack and the memory profiler"),
    ("os", "reads the clock: the answer depends on when the module loads"),
];

/// Single functions of a kept library that are not in `luau`.
pub const LUAU_FUNCTIONS_LEFT_OUT: &[(&str, &str)] = &[
    ("math.random", "a different answer on each load"),
    ("math.randomseed", "changes the generator every script shares"),
];

/// Bare Luau builtins in `luau`. Picked by hand, for the same reason.
pub const LUAU_GLOBALS: &[&str] = &[
    "getmetatable",
    "ipairs",
    "next",
    "pairs",
    "rawequal",
    "rawget",
    "rawlen",
    "rawset",
    "select",
    "setmetatable",
    "tonumber",
    "tostring",
    "type",
    "typeof",
    "unpack",
];

/// The bare Luau builtins left out, and why. With `LUAU_GLOBALS` this is
/// every bare function in the bundled Luau std (same test).
pub const LUAU_GLOBALS_LEFT_OUT: &[(&str, &str)] = &[
    ("assert", "stops the module loading when it fails"),
    ("error", "stops the module loading"),
    ("print", "writes to the output"),
    ("pcall", "runs a function; a project that allows it has that function checked"),
    ("xpcall", "runs a function; a project that allows it has that function checked"),
    ("require", "always allowed, on its own (`ALWAYS_ALLOWED`)"),
    ("loadstring", "compiles and runs code"),
    ("getfenv", "reads the environment, and turns the optimizer off for the file"),
    ("setfenv", "replaces an environment"),
    ("collectgarbage", "asks the collector"),
    ("gcinfo", "asks the collector"),
    ("newproxy", "makes a userdata"),
];

/// Roblox types whose functions are not in `roblox-datatypes`, and why.
/// Every other type the bundled `roblox.yml` gives a `Type.function` is
/// in, with any function of it (so a constructor newer than that file
/// is in too).
pub const DATATYPES_LEFT_OUT: &[(&str, &str)] = &[
    ("Enum", "enum items are values, not calls; `GetEnumItems` is a lookup, not a constructor"),
    ("Instance", "makes an instance, not a value"),
    ("SharedTable", "state shared between actors, not a value"),
];

/// Single functions of a kept type that are not in `roblox-datatypes`.
/// Two neighbours stay in, because they only build a value: `Random.new`
/// (seeded from entropy when given no seed; every method called on it is
/// a report of its own) and `DateTime.fromLocalTime` (reads the machine's
/// time zone, not the clock).
pub const DATATYPE_FUNCTIONS_LEFT_OUT: &[(&str, &str)] = &[
    ("BrickColor.random", "a different answer on each load"),
    ("DateTime.now", "reads the clock, like the `os` library"),
];

/// What calls a function it is handed before it returns. A function
/// literal given to one of these, when the call is allowed, is module
/// scope. Everything else may keep the function for later
/// (`table.insert(handlers, function() ... end)`), and is not entered.
pub const RUNS_ITS_ARGUMENT: &[&str] =
    &["pcall", "xpcall", "table.sort", "table.foreach", "table.foreachi", "string.gsub"];

impl Builtin {
    pub const ALL: &'static [Builtin] = &[Builtin::Luau, Builtin::RobloxDatatypes];

    /// The name rime.toml's `allow-builtin` uses.
    pub fn name(self) -> &'static str {
        match self {
            Builtin::Luau => "luau",
            Builtin::RobloxDatatypes => "roblox-datatypes",
        }
    }

    pub fn from_name(name: &str) -> Option<Builtin> {
        Builtin::ALL.iter().copied().find(|b| b.name() == name)
    }

    /// The libraries (`luau`) or types (`roblox-datatypes`) whose
    /// functions the list holds, sorted. The types are read off the
    /// bundled `roblox.yml` once: every `Type` it gives a `Type.function`.
    pub fn roots(self) -> &'static [&'static str] {
        static TYPES: OnceLock<Vec<&'static str>> = OnceLock::new();
        match self {
            Builtin::Luau => LUAU_LIBRARIES,
            Builtin::RobloxDatatypes => TYPES.get_or_init(|| {
                let mut types: Vec<&'static str> = rime_std::bundled_functions()
                    .iter()
                    .filter(|function| function.source == StdSource::Roblox)
                    .filter_map(|function| function.name.split_once('.'))
                    .filter(|(ty, member)| {
                        !member.contains('.')
                            && ty.starts_with(|c: char| c.is_ascii_uppercase())
                            && !DATATYPES_LEFT_OUT.iter().any(|(left_out, _)| left_out == ty)
                    })
                    .map(|(ty, _)| ty)
                    .collect();
                types.sort_unstable();
                types.dedup();
                types
            }),
        }
    }

    /// Whether the list holds `callee`, as written at the call.
    pub fn contains(self, callee: &str) -> bool {
        let (globals, left_out) = match self {
            Builtin::Luau => (LUAU_GLOBALS, LUAU_FUNCTIONS_LEFT_OUT),
            Builtin::RobloxDatatypes => (&[][..], DATATYPE_FUNCTIONS_LEFT_OUT),
        };
        if globals.contains(&callee) {
            return true;
        }
        // `root.function` and nothing more: `CFrame.new():Inverse` is a
        // method on a value, not a function of the type.
        let Some((root, function)) = callee.split_once('.') else { return false };
        !function.is_empty()
            && function.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            && self.roots().binary_search(&root).is_ok()
            && !left_out.iter().any(|(name, _)| *name == callee)
    }
}

/// rime.toml's `[rules.module-scope-computation]`, less the level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Callees a module may call as it loads, on top of the built-in
    /// lists. `*` stands for any run of characters: `*.new`,
    /// `Signal.*`, `*:IsServer`.
    pub allow: Vec<String>,
    /// The built-in lists in use. Empty: none.
    pub allow_builtin: Vec<Builtin>,
    /// Callees that are reported whatever `allow` and the built-in lists
    /// say. Same patterns as `allow`.
    pub deny: Vec<String>,
    /// Whether a module may loop as it loads. Calls inside the loop are
    /// then checked one by one.
    pub allow_loops: bool,
    /// Files that are entry scripts, not modules, and are left alone. A
    /// pattern without `/` is matched against the file name; one with `/`
    /// against the path below `root`, where `*` stops at `/` and `**`
    /// does not. A pattern whose last part has no `*` is a folder below
    /// `root` as well, and covers everything under it.
    pub scripts: Vec<String>,
    /// Files and folders the rule leaves alone for any other reason.
    /// Same patterns as `scripts`.
    pub exclude: Vec<String>,
    /// The directory `scripts` and `exclude` paths are relative to
    /// (rime.toml's). None when there is no rime.toml: only file-name
    /// patterns can match.
    pub root: Option<PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            allow: Vec::new(),
            allow_builtin: Builtin::ALL.to_vec(),
            deny: Vec::new(),
            allow_loops: false,
            scripts: DEFAULT_SCRIPTS.iter().map(|s| s.to_string()).collect(),
            exclude: Vec::new(),
            root: None,
        }
    }
}

/// Why a callee is, or is not, allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Allowed,
    Reported,
    /// On a built-in list, but the name is a local here.
    ReportedShadowed,
}

impl Options {
    /// `global`: the callee's first name is not a local.
    fn verdict(&self, callee: &str, global: bool) -> Verdict {
        let matches = |patterns: &[String]| {
            patterns.iter().any(|p| wild(p.as_bytes(), callee.as_bytes(), None))
        };
        if matches(&self.deny) {
            return Verdict::Reported;
        }
        if matches(&self.allow) {
            return Verdict::Allowed;
        }
        let built_in =
            ALWAYS_ALLOWED.contains(&callee) || self.allow_builtin.iter().any(|b| b.contains(callee));
        match (built_in, global) {
            (true, true) => Verdict::Allowed,
            (true, false) => Verdict::ReportedShadowed,
            (false, _) => Verdict::Reported,
        }
    }

    /// `scripts` and `exclude` entries that name a path below `root`
    /// which is not there, as (setting, entry): a typo, or a path written
    /// from the wrong folder. Only the part of a pattern before its first
    /// `*` can be looked up, and a bare name cannot be at all (it is a
    /// file name anywhere), so this finds some dead entries, not all.
    pub fn missing_paths(&self) -> Vec<(&'static str, &str)> {
        let Some(root) = &self.root else { return Vec::new() };
        let entries = self.scripts.iter().map(|p| ("scripts", p)).chain(self.exclude.iter().map(|p| ("exclude", p)));
        entries
            .filter(|(_, pattern)| pattern.contains('/'))
            .filter(|(_, pattern)| {
                let fixed: Vec<&str> = pattern.split('/').take_while(|part| !part.contains('*')).collect();
                !fixed.is_empty() && !root.join(fixed.join("/")).exists()
            })
            .map(|(setting, pattern)| (setting, pattern.as_str()))
            .collect()
    }

    /// Whether the rule leaves the file at `path` alone: an entry script
    /// (`scripts`) or a path the project took out (`exclude`).
    pub fn leaves_alone(&self, path: &Path) -> bool {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let below_root = self.root.as_ref().and_then(|root| path.strip_prefix(root).ok()).map(|rel| {
            rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/")
        });
        self.scripts.iter().chain(&self.exclude).any(|pattern| {
            let by_name = !pattern.contains('/');
            if by_name && wild(pattern.as_bytes(), name.as_bytes(), Some(b'/')) {
                return true;
            }
            let Some(rel) = &below_root else { return false };
            if !by_name && wild(pattern.as_bytes(), rel.as_bytes(), Some(b'/')) {
                return true;
            }
            // A pattern that ends in a plain name is a folder below `root`
            // too, as in the top-level `ignore`: `tools`, `tools/` and
            // `tools/**` leave out the same files. `tools/*` is not one:
            // it is the files directly in `tools`.
            let folder = pattern.trim_end_matches('/');
            let last = folder.rsplit('/').next().unwrap_or(folder);
            !last.contains('*') && wild(format!("{folder}/**").as_bytes(), rel.as_bytes(), Some(b'/'))
        })
    }
}

/// `*` matches any run of characters. With `sep`, `*` stops at it and `**`
/// does not, and `**/` also matches nothing at all (`a/**/b` is `a/b` too).
///
/// The work is bounded by pattern length times text length, however many
/// stars the pattern has: a project's `allow` entry cannot stall a run.
fn wild(pattern: &[u8], text: &[u8], sep: Option<u8>) -> bool {
    match sep {
        None => wild_name(pattern, text),
        Some(sep) => {
            let mut failed = vec![false; (pattern.len() + 1) * (text.len() + 1)];
            wild_path(pattern, text, sep, 0, 0, &mut failed)
        }
    }
}

/// No separator: every star is the same star. On a mismatch, go back to
/// the last star and let it take one more character.
fn wild_name(pattern: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        if pattern.get(p) == Some(&b'*') {
            p += 1;
            star = Some((p, t));
        } else if pattern.get(p) == Some(&text[t]) {
            p += 1;
            t += 1;
        } else if let Some((after, taken)) = star {
            p = after;
            t = taken + 1;
            star = Some((after, taken + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|c| *c == b'*')
}

/// With a separator there are two kinds of star, so one place to go back
/// to is not enough. `failed` remembers each (pattern, text) position that
/// has been tried and did not match, so none is tried twice.
fn wild_path(pattern: &[u8], text: &[u8], sep: u8, p: usize, t: usize, failed: &mut [bool]) -> bool {
    let slot = p * (text.len() + 1) + t;
    if failed[slot] {
        return false;
    }
    let matched = 'matched: {
        let Some(first) = pattern.get(p) else { break 'matched t == text.len() };
        if *first != b'*' {
            break 'matched text.get(t) == Some(first) && wild_path(pattern, text, sep, p + 1, t + 1, failed);
        }
        let crosses = pattern.get(p + 1) == Some(&b'*');
        let rest = if crosses { p + 2 } else { p + 1 };
        if crosses && pattern.get(rest) == Some(&sep) && wild_path(pattern, text, sep, rest + 1, t, failed) {
            break 'matched true;
        }
        for end in t..=text.len() {
            if wild_path(pattern, text, sep, rest, end, failed) {
                break 'matched true;
            }
            if !crosses && text.get(end) == Some(&sep) {
                break;
            }
        }
        false
    };
    failed[slot] = !matched;
    matched
}

/// A `scripts` or `exclude` entry as the rule matches it: `/` between
/// folders, and no `./` or `/` in front. `src\tools`, `./src/tools` and
/// `/src/tools` are all `src/tools`.
pub fn path_pattern(raw: &str) -> String {
    let mut pattern = raw.replace('\\', "/");
    loop {
        let rest = pattern.strip_prefix("./").or_else(|| pattern.strip_prefix('/'));
        match rest {
            Some(rest) => pattern = rest.to_string(),
            None => return pattern,
        }
    }
}

/// `path` is the file, when the caller has one. `None` skips the path
/// tests, and the file is judged by its `return` alone.
pub(crate) fn check(
    module: &hir::Module,
    names: &scope::Resolution,
    path: Option<&Path>,
    src: &str,
    options: &Options,
    diags: &mut Vec<Diagnostic>,
) {
    if path.is_some_and(|p| options.leaves_alone(p)) {
        return;
    }
    let root = module.body(module.root);
    if !returns_a_value(&root.block) {
        return;
    }
    Walk { module, names, src, options, diags, stmt: 0..0, renamed: HashSet::new() }.block(&root.block);
}

/// Whether the block, or a block nested in it, ends in `return <value>`.
/// Function bodies are not blocks of this body and are not looked at.
fn returns_a_value(block: &hir::Block) -> bool {
    use hir::StmtKind as K;
    if matches!(&block.terminator, Some(hir::Terminator::Return { exprs, .. }) if !exprs.is_empty()) {
        return true;
    }
    block.stmts.iter().any(|stmt| match &stmt.kind {
        K::Do(b)
        | K::While { body: b, .. }
        | K::Repeat { body: b, .. }
        | K::NumericFor { body: b, .. }
        | K::GenericFor { body: b, .. } => returns_a_value(b),
        K::If { then, else_ifs, or_else, .. } => {
            returns_a_value(then)
                || else_ifs.iter().any(|(_, b)| returns_a_value(b))
                || or_else.as_ref().is_some_and(returns_a_value)
        }
        _ => false,
    })
}

struct Walk<'a> {
    module: &'a hir::Module,
    names: &'a scope::Resolution,
    src: &'a str,
    options: &'a Options,
    diags: &'a mut Vec<Diagnostic>,
    /// The statement being walked: where an allow comment for any call in
    /// it goes, and what a call with no callee to point at is reported on
    /// (a function called where it is written).
    stmt: Span,
    /// Where each `local name = name` seen so far declares its name, when
    /// the right-hand `name` is the global.
    renamed: HashSet<usize>,
}

impl Walk<'_> {
    fn block(&mut self, block: &hir::Block) {
        use hir::StmtKind as K;
        let outer = self.stmt.clone();
        for stmt in &block.stmts {
            self.stmt = stmt.span.clone();
            match &stmt.kind {
                K::Local { decls, init } => {
                    self.exprs(init);
                    for (decl, value) in decls.iter().zip(init) {
                        if let hir::Expr::Name { name, span } = value {
                            if *name == decl.name && !self.names.is_local(span) {
                                self.renamed.insert(decl.span.start);
                            }
                        }
                    }
                }
                K::Assign { targets, values } => {
                    for target in targets {
                        self.target(target);
                    }
                    self.exprs(values);
                }
                K::CompoundAssign { target, value } => {
                    self.target(target);
                    self.expr(value);
                }
                K::Call(call) => self.expr(call),
                K::Do(body) => self.block(body),
                K::If { cond, then, else_ifs, or_else } => {
                    self.expr(cond);
                    self.block(then);
                    for (cond, arm) in else_ifs {
                        self.expr(cond);
                        self.block(arm);
                    }
                    if let Some(arm) = or_else {
                        self.block(arm);
                    }
                }
                K::While { cond, body } => {
                    if self.looping(stmt, "while") {
                        self.expr(cond);
                        self.block(body);
                    }
                }
                K::Repeat { body, until } => {
                    if self.looping(stmt, "repeat") {
                        self.block(body);
                        self.expr(until);
                    }
                }
                K::NumericFor { start, end, step, body, .. } => {
                    if self.looping(stmt, "for") {
                        self.expr(start);
                        self.expr(end);
                        if let Some(step) = step {
                            self.expr(step);
                        }
                        self.block(body);
                    }
                }
                K::GenericFor { exprs, body, .. } => {
                    if self.looping(stmt, "for") {
                        self.exprs(exprs);
                        self.block(body);
                    }
                }
                K::LocalFunction { .. } | K::FunctionDecl { .. } => {}
            }
        }
        if let Some(hir::Terminator::Return { exprs, span }) = &block.terminator {
            self.stmt = span.clone();
            self.exprs(exprs);
        }
        self.stmt = outer;
    }

    /// A loop statement. True when loops are allowed, and the caller then
    /// looks inside; otherwise the loop is reported and that is all.
    fn looping(&mut self, stmt: &hir::Stmt, keyword: &str) -> bool {
        if self.options.allow_loops {
            return true;
        }
        let start = stmt.span.start;
        let span = if self.src[start..].starts_with(keyword) {
            start..start + keyword.len()
        } else {
            first_line(self.src, stmt.span.clone())
        };
        self.diags.push(
            Diagnostic::warning(Code(312), format!("`{keyword}` loop at module scope"))
                .with_label(Label::primary(span, "runs when the module is first required"))
                .with_note(WHY)
                .with_note("calls inside the loop are not reported one by one")
                .conservative(
                    "valid Luau: whether a module may loop as it loads is this project's \
                     choice (`allow-loops` under `[rules.module-scope-computation]` in rime.toml)",
                )
                .with_help("move the loop into a function, so it runs when something calls it"),
        );
        false
    }

    /// `local string = string`, never written again: the name is a local,
    /// and what it holds is the global of that name.
    fn is_the_global(&self, root: &Span) -> bool {
        let name = &self.src[root.clone()];
        self.names
            .var(name, root)
            .is_some_and(|var| var.writes.len() == 1 && self.renamed.contains(&var.writes[0].span.start))
    }

    fn target(&mut self, target: &hir::Target) {
        if let hir::Target::Projection(e) = target {
            self.expr(e);
        }
    }

    fn exprs(&mut self, exprs: &[hir::Expr]) {
        for e in exprs {
            self.expr(e);
        }
    }

    fn expr(&mut self, e: &hir::Expr) {
        use hir::Expr as E;
        match e {
            E::Call { base, args, method, span, .. } => {
                // Source order: what the callee is read from, the call, its
                // arguments.
                self.expr(base);
                let callee = callee_text(base, method.as_deref());
                let global = root_name(base).is_none_or(|root| !self.names.is_local(root) || self.is_the_global(root));
                let verdict = self.options.verdict(&callee, global);
                let allowed = verdict == Verdict::Allowed;
                if !allowed {
                    self.call(&callee, base, method.as_deref(), span, verdict);
                }
                let runs_its_argument =
                    allowed && global && method.is_none() && RUNS_ITS_ARGUMENT.contains(&callee.as_str());
                for arg in args {
                    match arg {
                        E::Function(body) if runs_its_argument => {
                            self.block(&self.module.body(*body).block)
                        }
                        other => self.expr(other),
                    }
                }
            }
            E::Index { base, key, .. } => {
                self.expr(base);
                if let Some(key) = key {
                    self.expr(key);
                }
            }
            E::BinOp { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            E::Not(inner) | E::UnOp { operand: inner, .. } => self.expr(inner),
            E::Table(fields) => {
                for field in fields {
                    match field {
                        hir::TableField::Keyed { key, value } => {
                            self.expr(key);
                            self.expr(value);
                        }
                        hir::TableField::Named { value, .. } => self.expr(value),
                        hir::TableField::Item(value) => self.expr(value),
                    }
                }
            }
            E::IfExpr(arms) => {
                for (cond, value) in arms {
                    if let Some(cond) = cond {
                        self.expr(cond);
                    }
                    self.expr(value);
                }
            }
            E::Composite(parts) => self.exprs(parts),
            E::Name { .. } | E::Function(_) | E::Literal { .. } | E::Leaf => {}
        }
    }

    fn call(
        &mut self,
        callee: &str,
        base: &hir::Expr,
        method: Option<&str>,
        suffix: &Span,
        verdict: Verdict,
    ) {
        // A function called where it is written has no callee to name, and
        // its call parentheses sit after its `end`, lines away from where a
        // reader (or an allow comment) looks: point at the statement.
        let (message, span) = if matches!(base, hir::Expr::Function(_)) && method.is_none() {
            (
                "a function is called at module scope where it is written".to_string(),
                first_line(self.src, self.stmt.clone()),
            )
        } else {
            (
                format!("`{callee}` is called at module scope"),
                callee_span(self.src, base, method, suffix),
            )
        };
        // On its statement's first line as well as its own, so one allow
        // comment above a statement written over several lines covers
        // every call in it.
        let mut diag = Diagnostic::warning(Code(312), message)
            .with_label(Label::primary(span, "runs when the module is first required"))
            .with_allow_anchor(self.stmt.start)
            .with_note(WHY);
        if verdict == Verdict::ReportedShadowed {
            let root = root_name(base).map(|s| &self.src[s.clone()]).unwrap_or(callee);
            diag = diag.with_note(format!(
                "`{root}` is a local here, not the global the built-in list names: \
                 list `{callee}` under `allow` if the project's own is fine to call"
            ));
        }
        self.diags.push(
            diag.conservative(
                "valid Luau: which calls a module may make as it loads is this project's \
                 choice (`allow` under `[rules.module-scope-computation]` in rime.toml)",
            )
            .with_help("move the call into a function, so it runs when something calls it"),
        );
    }
}

const WHY: &str = "a module's top level runs on the first `require`, before the requirer gets \
                   anything back: work there delays every requirer, and nothing declares its order";

/// The callee as written: `a.b.c`, `a.b:m`, `f().x:m`, `t[]`, `<function>`.
/// A call in the chain is `()`, a bracket index `[]`, and anything that is
/// not a name, an index or a call (`("x"):rep(2)`) is `<expr>`.
fn callee_text(base: &hir::Expr, method: Option<&str>) -> String {
    fn path(e: &hir::Expr, out: &mut String) {
        match e {
            hir::Expr::Name { name, .. } => out.push_str(name),
            hir::Expr::Index { base, name: Some(name), .. } => {
                path(base, out);
                out.push('.');
                out.push_str(name);
            }
            hir::Expr::Index { base, .. } => {
                path(base, out);
                out.push_str("[]");
            }
            hir::Expr::Call { base, method, .. } => {
                path(base, out);
                if let Some(m) = method {
                    out.push(':');
                    out.push_str(m);
                }
                out.push_str("()");
            }
            hir::Expr::Function(_) => out.push_str("<function>"),
            _ => out.push_str("<expr>"),
        }
    }
    let mut out = String::new();
    path(base, &mut out);
    if let Some(m) = method {
        out.push(':');
        out.push_str(m);
    }
    out
}

/// The first name of the callee (`a` in `a.b:c()`, through calls and
/// indexing). None when it starts with anything else.
fn root_name(e: &hir::Expr) -> Option<&Span> {
    match e {
        hir::Expr::Name { span, .. } => Some(span),
        hir::Expr::Index { base, .. } | hir::Expr::Call { base, .. } => root_name(base),
        _ => None,
    }
}

/// Where a dotted path of names starts (`a` in `a.b.c`). None when the
/// expression is anything else.
fn path_start(e: &hir::Expr) -> Option<usize> {
    match e {
        hir::Expr::Name { span, .. } => Some(span.start),
        hir::Expr::Index { base, name: Some(_), .. } => path_start(base),
        _ => None,
    }
}

/// The part of the call to point at: the callee, not its arguments, so a
/// call that hands over a fifty-line function still gets a one-line label
/// and each call of a chain gets its own. `suffix` is `Expr::Call::span`,
/// which starts at the `:` of a method call and at the `(` of a plain one.
fn callee_span(src: &str, base: &hir::Expr, method: Option<&str>, suffix: &Span) -> Span {
    let bytes = src.as_bytes();
    let fallback = || first_line(src, suffix.clone());
    match method {
        Some(name) => {
            if bytes.get(suffix.start) != Some(&b':') {
                return fallback();
            }
            let mut at = suffix.start + 1;
            while bytes.get(at).is_some_and(|b| b.is_ascii_whitespace()) {
                at += 1;
            }
            if !src[at..].starts_with(name) {
                return fallback();
            }
            path_start(base).unwrap_or(at)..at + name.len()
        }
        None => {
            let mut end = suffix.start;
            while end > 0 && bytes[end - 1].is_ascii_whitespace() {
                end -= 1;
            }
            let start = path_start(base).or(match base {
                hir::Expr::Index { name_span: Some(s), .. } => Some(s.start),
                _ => None,
            });
            match start {
                Some(start) if start < end => start..end,
                _ => fallback(),
            }
        }
    }
}

/// `span`, cut at the end of the line it starts on.
fn first_line(src: &str, span: Span) -> Span {
    let stop = span.end.min(src.len());
    let end = src[span.start..stop].find(['\r', '\n']).map_or(stop, |i| span.start + i);
    span.start..end
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_at(path: Option<&str>, src: &str, options: &Options) -> Vec<Diagnostic> {
        let module = rime_syntax::parse(src).expect("test source parses");
        let names = scope::resolve(&module);
        let mut diags = Vec::new();
        let path = path.map(PathBuf::from);
        check(&module, &names, path.as_deref(), src, options, &mut diags);
        diags
    }

    fn run(src: &str, options: &Options) -> Vec<Diagnostic> {
        run_at(None, src, options)
    }

    fn messages(diags: &[Diagnostic]) -> Vec<&str> {
        diags.iter().map(|d| d.message.as_str()).collect()
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    /// No built-in list: `require` and `game:GetService` are all a module
    /// may call.
    fn bare() -> Options {
        Options { allow_builtin: Vec::new(), ..Options::default() }
    }

    /// `callees` on top of `bare`, so a test of the patterns sees only them.
    fn allowing(callees: &[&str]) -> Options {
        Options { allow: strings(callees), ..bare() }
    }

    /// The text each report's primary label covers.
    fn pointed_at<'a>(diags: &[Diagnostic], src: &'a str) -> Vec<&'a str> {
        diags.iter().map(|d| &src[d.primary_span()]).collect()
    }

    #[test]
    fn declarations_are_not_work() {
        let src = "\
local Players = game:GetService(\"Players\")
local Other = require(script.Parent.Other)
type Speed = number
local DAY = 60 * 60 * 24
local NAME = \"a\" .. \"b\"
local Folder = script.Parent.Assets
local Module = {}
Module.__index = Module
Module.Limits = { Min = 1, Max = DAY * 2, Tags = { \"a\", \"b\" } }
local count = 0
count += 1
local function helper(x: number): number
\treturn x + count
end
function Module.start()
\tPlayers.PlayerAdded:Connect(helper)
\ttask.spawn(helper, 1)
end
Module.stop = function()
\tOther.stop()
end
return Module
";
        assert_eq!(messages(&run(src, &Options::default())), Vec::<&str>::new());
    }

    #[test]
    fn a_call_at_module_scope_is_reported() {
        let src = "local Module = {}\ntask.spawn(function() end)\nreturn Module\n";
        let diags = run(src, &Options::default());
        assert_eq!(messages(&diags), vec!["`task.spawn` is called at module scope"]);
        assert_eq!(diags[0].code, Code(312));
        assert_eq!(diags[0].severity, rime_diag::Severity::Warning);
        assert!(diags[0].conservative, "valid Luau, rejected by the project's own rule");
        assert!(diags[0].suggestions.is_empty(), "there is no edit that moves a call into a function");
    }

    #[test]
    fn a_call_is_found_wherever_it_sits_in_a_statement() {
        let src = "\
local Module = {}
local a = compute()
Module.b = compute()
Module.c = { x = compute(), [compute()] = 1, compute() }
local d = if compute() then 1 else 2
local e = `{compute()} items`
local f = -compute() + #compute()
Module[compute()] = 1
if compute() then
\tcompute()
elseif compute() then
\tcompute()
else
\tcompute()
end
do
\tcompute()
end
return compute()
";
        let diags = run(src, &Options::default());
        assert_eq!(diags.len(), 17, "{:#?}", messages(&diags));
        assert!(diags.iter().all(|d| d.message == "`compute` is called at module scope"));
    }

    #[test]
    fn with_no_built_in_list_only_require_and_get_service_pass() {
        let src = "\
local Players = game:GetService(\"Players\")
local Other = require(script.Parent.Other)
local n = math.floor(1.5)
local v = Vector3.new(1, 2, 3)
return {}
";
        assert_eq!(
            messages(&run(src, &bare())),
            vec!["`math.floor` is called at module scope", "`Vector3.new` is called at module scope"]
        );
    }

    #[test]
    fn by_default_the_luau_library_and_roblox_value_constructors_pass() {
        let src = "\
local a = math.floor(1.5) + math.max(1, 2)
local b = string.format(\"%d\", 1)
local c = table.freeze({ 1, 2 })
local d = setmetatable({}, { __index = c })
local e = typeof(d) == type(d) and tostring(a) or tonumber(b)
local f = bit32.band(1, 3) + utf8.len(\"x\") + buffer.len(buffer.create(4))
local g = Vector3.new(1, 2, 3)
local h = CFrame.Angles(0, math.rad(90), 0) * CFrame.new(g)
local i = Color3.fromRGB(255, 0, 0)
local j = TweenInfo.new(1)
local k = UDim2.fromScale(1, 1)
return { a, b, c, d, e, f, g, h, i, j, k }
";
        assert_eq!(messages(&run(src, &Options::default())), Vec::<&str>::new());
    }

    #[test]
    fn the_built_in_lists_stop_short_of_work() {
        let src = "\
local part = Instance.new(\"Part\")
local started = os.clock()
local now = DateTime.now()
local thread = coroutine.create(print)
local child = script:WaitForChild(\"Child\")
local found = workspace:FindFirstChild(\"Map\")
print(\"loaded\")
task.spawn(print)
assert(part, \"no part\")
return {}
";
        assert_eq!(
            messages(&run(src, &Options::default())),
            vec![
                "`Instance.new` is called at module scope",
                "`os.clock` is called at module scope",
                "`DateTime.now` is called at module scope",
                "`coroutine.create` is called at module scope",
                "`script:WaitForChild` is called at module scope",
                "`workspace:FindFirstChild` is called at module scope",
                "`print` is called at module scope",
                "`task.spawn` is called at module scope",
                "`assert` is called at module scope",
            ]
        );
    }

    #[test]
    fn allow_adds_to_the_built_in_lists() {
        let src = "\
local Other = require(script.Parent.Other)
local v = Vector3.new(1, 2, 3)
local changed = Signal.new()
local part = Instance.new(\"Part\")
return {}
";
        let options = Options { allow: strings(&["Signal.new"]), ..Options::default() };
        assert_eq!(messages(&run(src, &options)), vec!["`Instance.new` is called at module scope"]);
    }

    #[test]
    fn allow_builtin_says_which_lists_are_in_use() {
        let src = "local n = math.floor(1.5)\nlocal v = Vector3.new(1, 2, 3)\nreturn { n, v }\n";
        let with = |lists: &[Builtin]| {
            let options = Options { allow_builtin: lists.to_vec(), ..Options::default() };
            run(src, &options).iter().map(|d| d.message.clone()).collect::<Vec<_>>()
        };
        assert_eq!(with(&[Builtin::Luau, Builtin::RobloxDatatypes]), Vec::<String>::new());
        assert_eq!(with(&[Builtin::Luau]), vec!["`Vector3.new` is called at module scope"]);
        assert_eq!(with(&[Builtin::RobloxDatatypes]), vec!["`math.floor` is called at module scope"]);
        assert_eq!(with(&[]).len(), 2);
    }

    #[test]
    fn deny_takes_a_callee_back_out() {
        let src = "\
local Players = game:GetService(\"Players\")
local source = Random.new(7)
local low = math.floor(1.5)
local name = string.format(\"%d\", low)
local changed = Signal.new()
local part = Thing.new()
return {}
";
        let with = |allow: &[&str], deny: &[&str]| {
            let options = Options { allow: strings(allow), deny: strings(deny), ..Options::default() };
            run(src, &options).iter().map(|d| d.message.clone()).collect::<Vec<_>>()
        };
        assert_eq!(with(&["*.new"], &[]), Vec::<String>::new());
        // Out of a built-in list, one entry.
        assert_eq!(with(&["*.new"], &["math.floor"]), vec!["`math.floor` is called at module scope"]);
        assert_eq!(with(&[], &["Random.new"])[0], "`Random.new` is called at module scope");
        // Out of the project's own pattern.
        assert_eq!(with(&["*.new"], &["Thing.new"]), vec!["`Thing.new` is called at module scope"]);
        // A pattern, and one of the two that are always allowed.
        assert_eq!(
            with(&["*.new"], &["string.*", "game:GetService"]),
            vec![
                "`game:GetService` is called at module scope",
                "`string.format` is called at module scope",
            ]
        );
        // `deny` wins over an `allow` entry that names the same callee.
        assert_eq!(with(&["math.floor"], &["math.floor"]).len(), 3);
    }

    #[test]
    fn a_built_in_name_that_is_a_local_is_not_the_built_in() {
        let src = "\
local math = require(script.Parent.FixedPoint)
local string = { format = function() end }
local half = math.floor(1.5)
local text = string.format(\"%d\", 1)
local whole = tostring(half)
return { half, text, whole }
";
        let diags = run(src, &Options::default());
        assert_eq!(
            messages(&diags),
            vec!["`math.floor` is called at module scope", "`string.format` is called at module scope"]
        );
        assert!(
            diags[0].notes.iter().any(|n| n.contains("`math` is a local here")),
            "{:?}",
            diags[0].notes
        );
        // The project's own `allow` goes by the text, local or not.
        let options = Options { allow: strings(&["math.floor", "string.*"]), ..Options::default() };
        assert_eq!(run(src, &options), Vec::new());

        // The two always-allowed calls are the globals too.
        let src = "local require = loader(script)\nlocal Other = require(\"Other\")\nreturn Other\n";
        let diags = run(src, &Options::default());
        assert_eq!(
            messages(&diags),
            vec!["`loader` is called at module scope", "`require` is called at module scope"]
        );
        assert!(diags[1].notes.iter().any(|n| n.contains("`require` is a local here")));
        assert!(!diags[0].notes.iter().any(|n| n.contains("is a local here")));
    }

    #[test]
    fn the_luau_list_is_its_libraries_and_the_named_builtins() {
        let luau = Builtin::Luau;
        for name in [
            "math.floor", "math.lerp", "string.format", "string.split", "table.freeze", "table.insert",
            "bit32.band", "utf8.char", "utf8.graphemes", "buffer.writeu8", "vector.create",
            "setmetatable", "typeof", "rawset", "unpack", "math.isnan",
        ] {
            assert!(luau.contains(name), "luau lacks {name}");
        }
        for name in [
            "print", "error", "assert", "pcall", "require", "os.clock", "coroutine.wrap", "debug.traceback",
            "task.spawn", "Instance.new", "Vector3.new", "warn", "tick", "wait", "io.open",
            "math.random", "math.randomseed", "math", "math.", "math.floor.x", "math.floor()", "math:floor",
        ] {
            assert!(!luau.contains(name), "luau has {name}");
        }
        let mut sorted = luau.roots().to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(luau.roots(), &sorted[..], "sorted, no repeats: `contains` searches it");
    }

    #[test]
    fn every_luau_library_and_builtin_is_either_picked_or_left_out_with_a_reason() {
        let luau: Vec<&str> = rime_std::bundled_functions()
            .iter()
            .filter(|f| f.source == StdSource::Luau)
            .map(|f| f.name.as_str())
            .collect();
        let mut libraries: Vec<&str> = luau.iter().filter_map(|n| n.split_once('.')).map(|(l, _)| l).collect();
        libraries.dedup();
        let mut decided: Vec<&str> =
            LUAU_LIBRARIES.iter().copied().chain(LUAU_LIBRARIES_LEFT_OUT.iter().map(|(l, _)| *l)).collect();
        decided.sort_unstable();
        assert_eq!(libraries, decided, "a library in the bundled Luau std has no decision, or one has two");

        let globals: Vec<&str> = luau.iter().copied().filter(|n| !n.contains('.')).collect();
        let mut decided: Vec<&str> =
            LUAU_GLOBALS.iter().copied().chain(LUAU_GLOBALS_LEFT_OUT.iter().map(|(g, _)| *g)).collect();
        decided.sort_unstable();
        assert_eq!(globals, decided, "a bare function in the bundled Luau std has no decision, or one has two");
    }

    #[test]
    fn the_datatype_types_are_read_off_the_bundled_std() {
        let datatypes = Builtin::RobloxDatatypes;
        // Pinned: a type a newer roblox.yml adds shows up here to be looked at.
        assert_eq!(
            datatypes.roots(),
            [
                "Axes", "BrickColor", "CFrame", "CatalogSearchParams", "Color3", "ColorSequence",
                "ColorSequenceKeypoint", "Content", "DateTime", "DockWidgetPluginGuiInfo", "Faces",
                "FloatCurveKey", "Font", "NumberRange", "NumberSequence", "NumberSequenceKeypoint",
                "OverlapParams", "Path2DControlPoint", "PathWaypoint", "PhysicalProperties", "Random",
                "Ray", "RaycastParams", "Rect", "Region3", "Region3int16", "RotationCurveKey",
                "TweenInfo", "UDim", "UDim2", "Vector2", "Vector2int16", "Vector3", "Vector3int16",
            ]
        );
        for name in [
            "Vector3.new", "CFrame.Angles", "CFrame.lookAt", "Color3.fromRGB", "Color3.fromHex",
            "TweenInfo.new", "UDim2.fromScale", "NumberSequenceKeypoint.new", "RaycastParams.new",
            "DateTime.fromUnixTimestamp", "DateTime.fromLocalTime", "Random.new",
            "CFrame.fromRotationBetweenVectors",
        ] {
            assert!(datatypes.contains(name), "roblox-datatypes lacks {name}");
        }
        for name in [
            "Instance.new", "Instance.fromExisting", "SharedTable.new", "DateTime.now", "BrickColor.random",
            "task.spawn", "task.wait", "UserSettings", "settings", "tick", "wait", "math.floor",
            "utf8.graphemes", "debug.profilebegin", "Enum.Material.GetEnumItems", "CFrame.new():Inverse",
            "Vector3.new().Unit", "Thing.new",
        ] {
            assert!(!datatypes.contains(name), "roblox-datatypes has {name}");
        }
    }

    #[test]
    fn what_is_left_out_by_hand_is_checked_against_the_std() {
        let std: Vec<&str> = rime_std::bundled_functions().iter().map(|f| f.name.as_str()).collect();
        // A left-out entry names something the std has: a typo here would
        // leave nothing out.
        for (name, _) in DATATYPE_FUNCTIONS_LEFT_OUT.iter().chain(LUAU_FUNCTIONS_LEFT_OUT) {
            assert!(std.contains(name), "{name} is not in the bundled std");
        }
        for (library, _) in DATATYPES_LEFT_OUT {
            let below = format!("{library}.");
            assert!(std.iter().any(|n| n.starts_with(&below)), "no `{library}.` function in the bundled std");
        }
        for name in ALWAYS_ALLOWED {
            assert!(
                Builtin::ALL.iter().all(|b| !b.contains(name)),
                "{name} is always allowed and must not also be on a list `allow-builtin` can switch off"
            );
        }
        // What runs its argument is allowed in the first place, or is one
        // of the two a project has to allow.
        for name in RUNS_ITS_ARGUMENT {
            assert!(Builtin::Luau.contains(name) || ["pcall", "xpcall"].contains(name), "{name}");
        }
    }

    #[test]
    fn a_star_in_a_pattern_stands_for_any_run_of_characters() {
        let src = "\
local a = Vector3.new(1, 2, 3)
local b = Vector3.fromAxis(Enum.Axis.X)
local c = Thing.new()
local d = folder:WaitForChild(\"A\"):WaitForChild(\"B\")
local e = Players.PlayerAdded:Connect(print)
return {}
";
        let left = |allow: &[&str]| -> Vec<String> {
            run(src, &allowing(allow)).iter().map(|d| d.message.clone()).collect()
        };
        assert_eq!(left(&["*"]), Vec::<String>::new());
        assert_eq!(
            left(&["Vector3.*", "*:WaitForChild", "*:Connect", "print"]),
            vec!["`Thing.new` is called at module scope"]
        );
        assert_eq!(
            left(&["*.new", "folder:*", "Vector3.fromAxis"]),
            vec!["`Players.PlayerAdded:Connect` is called at module scope"]
        );
        // No star, no partial match.
        assert_eq!(left(&["Vector3", "new", "Connect"]).len(), 6);
    }

    #[test]
    fn each_call_of_a_chain_is_its_own_report_at_its_own_place() {
        let src = "local a = folder:WaitForChild(\"A\"):WaitForChild(\"B\").Child:Clone()\nreturn a\n";
        let diags = run(src, &Options::default());
        assert_eq!(
            messages(&diags),
            vec![
                "`folder:WaitForChild` is called at module scope",
                "`folder:WaitForChild():WaitForChild` is called at module scope",
                "`folder:WaitForChild():WaitForChild().Child:Clone` is called at module scope",
            ]
        );
        assert_eq!(pointed_at(&diags, src), vec!["folder:WaitForChild", "WaitForChild", "Clone"]);
        let starts: Vec<usize> = diags.iter().map(|d| d.primary_span().start).collect();
        assert!(starts[0] < starts[1] && starts[1] < starts[2], "{starts:?}");
    }

    #[test]
    fn the_label_is_the_callee_not_the_arguments() {
        let src = "\
local Module = {}
task.defer(function()
\tprint(\"later\")
end)
Module.Changed = Signal.new(Module, {
\tname = \"Changed\",
})
local n = (\"%d\"):format(1)
local made = build { 1, 2 }
local got = registry[\"key\"](1)
return Module
";
        let diags = run(src, &Options::default());
        assert_eq!(
            messages(&diags),
            vec![
                "`task.defer` is called at module scope",
                "`Signal.new` is called at module scope",
                "`<expr>:format` is called at module scope",
                "`build` is called at module scope",
                "`registry[]` is called at module scope",
            ]
        );
        let pointed = pointed_at(&diags, src);
        assert_eq!(&pointed[..2], ["task.defer", "Signal.new"]);
        assert_eq!(pointed[2], "format");
        assert_eq!(pointed[3], "build");
        for (d, text) in diags.iter().zip(&pointed) {
            assert!(!text.contains('\n'), "{}: label spans lines: {text:?}", d.message);
            assert!(!text.is_empty(), "{}: empty label", d.message);
        }
    }

    #[test]
    fn an_argument_that_is_a_call_is_checked_too() {
        let src = "local Other = require(folder:WaitForChild(\"Other\"))\nreturn Other\n";
        assert_eq!(
            messages(&run(src, &Options::default())),
            vec!["`folder:WaitForChild` is called at module scope"]
        );
        // And an allowed argument does not excuse the call it is handed to.
        let src = "local made = build(require(script.Parent.Other))\nreturn made\n";
        assert_eq!(messages(&run(src, &Options::default())), vec!["`build` is called at module scope"]);
    }

    #[test]
    fn a_function_body_is_not_module_scope() {
        let src = "\
local Module = {}
local function later()
\ttask.wait(1)
\tfor i = 1, 10 do
\t\tprint(i)
\tend
end
function Module.start()
\tlater()
end
function Module:stop()
\tlater()
end
Module.callback = function()
\tlater()
end
local handlers = { run = function() later() end }
task.defer(function()
\tlater()
end)
return Module
";
        assert_eq!(
            messages(&run(src, &Options::default())),
            vec!["`task.defer` is called at module scope"]
        );
    }

    #[test]
    fn a_loop_is_one_report_and_covers_what_is_inside_it() {
        let src = "\
local Module = { byName = {} }
for _, item in pairs(Module.items) do
\tModule.byName[item.name] = build(item)
\tfor i = 1, 3 do
\t\tregister(item, i)
\tend
end
for i = 1, compute() do
end
while poll() do
end
repeat
\tstep()
until done()
return Module
";
        let diags = run(src, &Options::default());
        assert_eq!(
            messages(&diags),
            vec![
                "`for` loop at module scope",
                "`for` loop at module scope",
                "`while` loop at module scope",
                "`repeat` loop at module scope",
            ]
        );
        assert_eq!(pointed_at(&diags, src), vec!["for", "for", "while", "repeat"]);
        assert!(diags.iter().all(|d| d.conservative && d.code == Code(312)));
        assert!(
            diags[0].notes.iter().any(|n| n.contains("not reported one by one")),
            "{:?}",
            diags[0].notes
        );
    }

    #[test]
    fn with_loops_allowed_the_calls_inside_are_checked_one_by_one() {
        let src = "\
local Module = { byName = {} }
for _, item in pairs(Module.items) do
\tModule.byName[item.name] = build(item)
end
while poll() do
\tstep()
end
return Module
";
        let options = Options { allow_loops: true, ..allowing(&["pairs"]) };
        assert_eq!(
            messages(&run(src, &options)),
            vec![
                "`build` is called at module scope",
                "`poll` is called at module scope",
                "`step` is called at module scope",
            ]
        );
    }

    #[test]
    fn an_if_is_not_work_but_what_it_holds_is() {
        let src = "\
local Module = {}
if RunService:IsServer() then
\tModule.limit = 10
\tsetup()
else
\tModule.limit = 5
end
return Module
";
        assert_eq!(
            messages(&run(src, &Options::default())),
            vec!["`RunService:IsServer` is called at module scope", "`setup` is called at module scope"]
        );
        assert_eq!(messages(&run(src, &allowing(&["*:IsServer", "setup"]))), Vec::<&str>::new());
    }

    #[test]
    fn the_returned_value_is_checked_like_any_other_expression() {
        let src = "local Module = {}\nreturn setmetatable(Module, { __index = build() })\n";
        assert_eq!(
            messages(&run(src, &bare())),
            vec!["`setmetatable` is called at module scope", "`build` is called at module scope"]
        );
        assert_eq!(
            messages(&run("local Module = {}\nreturn table.freeze(Module)\n", &allowing(&["table.freeze"]))),
            Vec::<&str>::new()
        );
    }

    #[test]
    fn a_function_called_where_it_is_written_is_a_call() {
        let src = "local value = (function()\n\treturn compute()\nend)()\nreturn value\n";
        let diags = run(src, &Options::default());
        assert_eq!(messages(&diags), vec!["a function is called at module scope where it is written"]);
        // At the statement, not at the `()` after the `end`: that is where
        // an allow comment goes.
        assert_eq!(&src[diags[0].primary_span()], "local value = (function()");
    }

    #[test]
    fn an_allowed_pcall_does_not_hide_what_it_runs() {
        let src = "\
local ok, value = pcall(function()
\treturn fetch()
end)
local ok2 = xpcall(function()
\tfetch()
end, function(err)
\treport(err)
end)
local ok3 = pcall(fetch)
return value
";
        // Not allowed: the pcall is the report, and it owns its function.
        assert_eq!(
            messages(&run(src, &Options::default())),
            vec![
                "`pcall` is called at module scope",
                "`xpcall` is called at module scope",
                "`pcall` is called at module scope",
            ]
        );
        // Allowed: the function it runs on the spot is module scope. The
        // error handler runs only on failure, and `report` in it is seen
        // too — it is handed over the same way.
        assert_eq!(
            messages(&run(src, &allowing(&["pcall", "xpcall"]))),
            vec![
                "`fetch` is called at module scope",
                "`fetch` is called at module scope",
                "`report` is called at module scope",
            ]
        );
    }

    #[test]
    fn a_library_function_does_not_hide_the_function_it_runs() {
        // These four call the function they are handed before they return,
        // so that function's body is module scope.
        let src = "\
local names = { \"b\", \"a\" }
table.foreachi(names, function(_, name)
\tannounce(name)
end)
table.sort(names, function(left, right)
\treturn rank(left) < rank(right)
end)
local shown = string.gsub(\"a b\", \"%a\", function(word)
\treturn lookup(word)
end)
table.foreach(names, function(_, name)
\tregister(name)
end)
local meta = setmetatable({}, { __index = function(_, key)
\treturn compute(key)
end })
return { names, shown, meta }
";
        assert_eq!(
            messages(&run(src, &Options::default())),
            vec![
                "`announce` is called at module scope",
                "`rank` is called at module scope",
                "`rank` is called at module scope",
                "`lookup` is called at module scope",
                "`register` is called at module scope",
            ],
            "a function kept in a table is not run by the call"
        );
        // The project's own callee is not known to run what it is given.
        let own = "local handle = Signal.new(function()\n\twork()\nend)\nreturn handle\n";
        let options = Options { allow: strings(&["Signal.new"]), ..Options::default() };
        assert_eq!(run(own, &options), Vec::new());
    }

    #[test]
    fn a_function_handed_to_a_call_that_keeps_it_is_not_run() {
        // The rest of the `luau` list stores or inspects a function it is
        // given. Nothing in these bodies runs as the module loads.
        let src = "\
local handlers = {}
table.insert(handlers, function()
\twork()
end)
rawset(handlers, \"late\", function()
\twork()
end)
local packed = table.pack(function()
\twork()
end)
local filled = table.create(2, function()
\twork()
end)
local first = select(1, function()
\twork()
end)
local kind = type(function()
\twork()
end)
local shown = string.format(\"%s\", function()
\twork()
end)
return { handlers, packed, filled, first, kind, shown }
";
        assert_eq!(messages(&run(src, &Options::default())), Vec::<&str>::new());
    }

    #[test]
    fn what_gives_a_different_answer_on_each_load_is_not_built_in() {
        let src = "\
local roll = math.random(1, 6)
math.randomseed(7)
local colour = BrickColor.random()
local source = Random.new()
local stamp = DateTime.fromLocalTime(2026, 1, 1)
return { roll, colour, source, stamp }
";
        assert_eq!(
            messages(&run(src, &Options::default())),
            vec![
                "`math.random` is called at module scope",
                "`math.randomseed` is called at module scope",
                "`BrickColor.random` is called at module scope",
            ],
            "`Random.new` and `DateTime.fromLocalTime` only build a value"
        );
    }

    #[test]
    fn a_library_or_type_is_allowed_by_its_name_not_function_by_function() {
        // None of these is in the bundled std files, which are older than
        // Luau and Roblox are.
        let src = "\
local a = math.isnan(0 / 0) or math.isinf(1) or math.isfinite(1)
local b = vector.lerp(vector.create(0, 0, 0), vector.create(1, 1, 1), 0.5)
local c = buffer.readbits(buffer.create(4), 0, 3)
buffer.writebits(buffer.create(4), 0, 3, 1)
local d = CFrame.fromRotationBetweenVectors(Vector3.xAxis, Vector3.yAxis)
return { a, b, c, d }
";
        assert_eq!(messages(&run(src, &Options::default())), Vec::<&str>::new());
        // A library or type that is not on a list stays off it, and so
        // does a method called on what a constructor gives back.
        let src = "\
local a = os.isnan(1)
local b = Instance.fromExisting(a)
local c = CFrame.new():Inverse()
local d = (\"x\"):rep(3)
return { a, b, c, d }
";
        assert_eq!(
            messages(&run(src, &Options::default())),
            vec![
                "`os.isnan` is called at module scope",
                "`Instance.fromExisting` is called at module scope",
                "`CFrame.new():Inverse` is called at module scope",
                "`<expr>:rep` is called at module scope",
            ]
        );
    }

    #[test]
    fn a_local_that_holds_the_global_of_its_own_name_is_the_global() {
        let src = "\
local string = string
local math, table = math, table
local game = game
local require = require
local Players = game:GetService(\"Players\")
local Other = require(script.Parent.Other)
local text = string.rep(\"-\", math.floor(2.5))
local list = table.freeze({ text })
return { Players, Other, list }
";
        assert_eq!(messages(&run(src, &Options::default())), Vec::<&str>::new());

        // Written again later, or holding something else, it is the
        // project's own.
        let src = "\
local math = math
local string = table
math = require(script.Parent.FixedPoint)
local half = math.floor(1.5)
local text = string.rep(\"-\", 3)
return { half, text }
";
        assert_eq!(
            messages(&run(src, &Options::default())),
            vec!["`math.floor` is called at module scope", "`string.rep` is called at module scope"]
        );

        // A local of that name further in is another variable.
        let src = "\
local math = math
do
\tlocal math = require(script.Parent.FixedPoint)
\tlocal half = math.floor(1.5)
end
return math.floor(2.5)
";
        assert_eq!(messages(&run(src, &Options::default())), vec!["`math.floor` is called at module scope"]);
    }

    #[test]
    fn an_allow_above_a_statement_covers_every_call_in_it() {
        let src = "\
local Storage = game:GetService(\"ReplicatedStorage\")
-- rime: allow(module-scope-computation, \"boot\")
local remote = Storage
\t:WaitForChild(\"Remotes\")
\t:WaitForChild(\"Fire\")
local other = Storage
\t:WaitForChild(\"Remotes\")
\t:WaitForChild(\"Other\")
return { remote, other }
";
        let diags = run(src, &Options::default());
        assert_eq!(diags.len(), 4);
        let statement = src.find("local remote").unwrap();
        assert_eq!(diags[0].allow_anchor, Some(statement));
        assert_eq!(diags[1].allow_anchor, Some(statement));
        let applied = rime_diag::allow::apply(src, diags);
        // The statement under the comment, and not the next one.
        assert_eq!(applied.suppressed, 2);
        assert_eq!(
            messages(&applied.kept),
            vec![
                "`Storage:WaitForChild` is called at module scope",
                "`Storage:WaitForChild():WaitForChild` is called at module scope",
            ]
        );

        // A table written over many lines is one statement too.
        let src = "\
-- rime: allow(module-scope-computation, \"built once\")
local Table = {
\tfirst = build(1),
\tsecond = build(2),
}
return Table
";
        assert_eq!(rime_diag::allow::apply(src, run(src, &Options::default())).kept, Vec::new());

        // A block is not: each statement in it is its own.
        let src = "\
-- rime: allow(module-scope-computation, \"the check only\")
if ready() then
\tstart()
end
return {}
";
        let applied = rime_diag::allow::apply(src, run(src, &Options::default()));
        assert_eq!(messages(&applied.kept), vec!["`start` is called at module scope"]);
    }

    #[test]
    fn a_pattern_with_many_stars_is_matched_without_backtracking_forever() {
        // One long chain as the callee, and a pattern that almost matches
        // it: each star used to multiply the work.
        let chain = format!("root{}", ":Step()".repeat(40));
        let pattern = format!("root{}:Missing", "*:Step()".repeat(12));
        let started = std::time::Instant::now();
        assert!(!wild(pattern.as_bytes(), chain.as_bytes(), None));
        let path_text = format!("{}file.luau", "dir/".repeat(40));
        let path_pattern = format!("{}missing.luau", "**/dir/*".repeat(10));
        assert!(!wild(path_pattern.as_bytes(), path_text.as_bytes(), Some(b'/')));
        assert!(started.elapsed() < std::time::Duration::from_secs(1), "{:?}", started.elapsed());
    }

    #[test]
    fn an_entry_script_is_left_alone_by_its_name() {
        let src = "local Boot = require(script.Parent.Boot)\nBoot.start()\nreturn nil\n";
        let options = Options::default();
        assert_eq!(run_at(Some("src/server/Main.luau"), src, &options).len(), 1);
        // Every suffix Rojo turns into a script, in both extensions.
        for name in [
            "Main.server.luau", "Main.server.lua", "Hud.client.luau", "Hud.client.lua", "init.server.luau",
            "Tool.plugin.luau", "Tool.plugin.lua", "init.client.lua", "init.plugin.luau",
        ] {
            let path = format!("src/server/{name}");
            assert_eq!(run_at(Some(&path), src, &options), Vec::new(), "{name}");
        }
        // The suffix has to be the script class, not any dotted name.
        assert_eq!(run_at(Some("src/server/Main.spec.luau"), src, &options).len(), 1);
        assert_eq!(run_at(Some("src/server/server.luau"), src, &options).len(), 1);
        assert_eq!(run_at(Some("src/server/plugin.luau"), src, &options).len(), 1);
        // Rojo makes ModuleScripts of these two: they are modules.
        assert_eq!(run_at(Some("src/server/Old.legacy.luau"), src, &options).len(), 1);
        assert_eq!(run_at(Some("src/server/Hud.local.lua"), src, &options).len(), 1);
        assert_eq!(DEFAULT_SCRIPTS.len(), 6);
    }

    #[test]
    fn exclude_leaves_out_paths_and_keeps_the_script_names() {
        let src = "local made = build()\nreturn made\n";
        let root = PathBuf::from("proj");
        let reports = |path: &str, exclude: &[&str]| {
            let options = Options { exclude: strings(exclude), root: Some(root.clone()), ..Options::default() };
            run_at(Some(path), src, &options).len()
        };
        assert_eq!(reports("proj/src/plugin/macros/Make.luau", &[]), 1);
        // A folder, written three ways, covers everything under it.
        for pattern in ["src/plugin/**", "src/plugin/", "src/plugin"] {
            assert_eq!(reports("proj/src/plugin/macros/Make.luau", &[pattern]), 0, "{pattern}");
            assert_eq!(reports("proj/src/plugin/Make.luau", &[pattern]), 0, "{pattern}");
            assert_eq!(reports("proj/src/plugins/Make.luau", &[pattern]), 1, "{pattern}");
            assert_eq!(reports("proj/other/src/plugin/Make.luau", &[pattern]), 1, "{pattern}");
        }
        // One file, a file-name pattern, a folder at any depth.
        assert_eq!(reports("proj/src/Data.luau", &["src/Data.luau"]), 0);
        assert_eq!(reports("proj/src/Data.spec.luau", &["*.spec.luau"]), 0);
        assert_eq!(reports("proj/src/a/tests/Case.luau", &["**/tests/**"]), 0);
        assert_eq!(reports("proj/src/a/Case.luau", &["**/tests/**"]), 1);
        // `exclude` is beside `scripts`, not in place of it.
        assert_eq!(reports("proj/src/Main.server.luau", &["src/plugin/**"]), 0);
        // Without a `/` it is a file name anywhere, or a folder right
        // below rime.toml: not a folder further down.
        assert_eq!(reports("proj/src/plugin/Make.luau", &["plugin"]), 1);
        assert_eq!(reports("proj/plugin/Make.luau", &["plugin"]), 0);
        assert_eq!(reports("proj/plugin/deep/Make.luau", &["plugin"]), 0);
        assert_eq!(reports("proj/plugin.luau", &["plugin"]), 1);
        // A `*` in the last part is files, not a folder to go under.
        assert_eq!(reports("proj/src/plugin/Make.luau", &["src/*"]), 1);
        assert_eq!(reports("proj/src/Make.luau", &["src/*"]), 0);
        assert_eq!(reports("proj/src/plugin/macros/Make.luau", &["src/*/macros"]), 0);
    }

    #[test]
    fn the_project_says_which_other_files_are_scripts() {
        let src = "local made = build()\nreturn made\n";
        let root = PathBuf::from("proj");
        let scripts = |patterns: &[&str]| Options {
            scripts: patterns.iter().map(|s| s.to_string()).collect(),
            root: Some(root.clone()),
            ..Options::default()
        };
        let reports = |path: &str, patterns: &[&str]| run_at(Some(path), src, &scripts(patterns)).len();

        // A name pattern matches the file name anywhere.
        assert_eq!(reports("proj/src/a/Thing.spec.luau", &["*.spec.luau"]), 0);
        assert_eq!(reports("proj/src/a/Thing.luau", &["*.spec.luau"]), 1);
        // A path pattern is relative to rime.toml's directory.
        assert_eq!(reports("proj/tools/gen/Make.luau", &["tools/**"]), 0);
        assert_eq!(reports("proj/tools/Make.luau", &["tools/**"]), 0);
        assert_eq!(reports("proj/src/tools/Make.luau", &["tools/**"]), 1);
        assert_eq!(reports("proj/src/tools/Make.luau", &["**/tools/**"]), 0);
        assert_eq!(reports("proj/tools/Make.luau", &["**/tools/**"]), 0);
        // `*` stops at a directory, `**` does not.
        assert_eq!(reports("proj/tools/gen/Make.luau", &["tools/*"]), 1);
        assert_eq!(reports("proj/tools/Make.luau", &["tools/*"]), 0);
        assert_eq!(reports("proj/tools/gen/Make.luau", &["tools/*/*.luau"]), 0);
        // Setting `scripts` replaces the default names.
        assert_eq!(reports("proj/src/Main.server.luau", &["*.spec.luau"]), 1);
        // A file outside the root can only match by name.
        assert_eq!(reports("elsewhere/tools/Make.luau", &["tools/**"]), 1);
        assert_eq!(reports("elsewhere/tools/Make.luau", &["Make.luau"]), 0);
    }

    #[test]
    fn a_file_that_returns_no_value_is_not_a_module() {
        // No path given: the `return` is all there is to go by.
        let options = Options::default();
        for src in [
            "print(\"boot\")\nfor i = 1, 3 do\n\tprint(i)\nend\n",
            "local ready = check()\nif not ready then\n\treturn\nend\nstart()\n",
            "start()\nreturn\n",
            "local function f()\n\treturn 1\nend\nf()\n",
        ] {
            assert_eq!(run(src, &options), Vec::new(), "{src}");
        }
    }

    #[test]
    fn a_module_may_return_from_inside_a_branch() {
        let options = Options::default();
        let early = "local Module = {}\nif true then\n\treturn Module\nend\nsetup()\n";
        assert_eq!(messages(&run(early, &options)), vec!["`setup` is called at module scope"]);
        let both = "local made = build()\nif FLAG then\n\treturn made\nelse\n\treturn made\nend\n";
        assert_eq!(messages(&run(both, &options)), vec!["`build` is called at module scope"]);
        let in_do = "do\n\tlocal made = build()\n\treturn made\nend\n";
        assert_eq!(messages(&run(in_do, &options)), vec!["`build` is called at module scope"]);
    }

    #[test]
    fn a_path_pattern_is_read_with_forward_slashes_and_no_leading_dot() {
        for raw in ["src/tools", "src\\tools", "./src/tools", ".\\src\\tools", "/src/tools", "././src/tools"] {
            assert_eq!(path_pattern(raw), "src/tools", "{raw}");
        }
        assert_eq!(path_pattern("*.spec.luau"), "*.spec.luau");
        assert_eq!(path_pattern("src\\**\\gen"), "src/**/gen");
        // A leading `..` is left as it is: it matches nothing, and
        // `missing_paths` is what says so.
        assert_eq!(path_pattern("../other"), "../other");
    }

    #[test]
    fn missing_paths_names_the_entries_whose_folder_is_not_there() {
        let root = std::env::temp_dir().join(format!("rime-module-scope-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src/tools")).unwrap();
        std::fs::write(root.join("src/Data.luau"), "return {}\n").unwrap();
        let options = Options {
            scripts: strings(&["*.server.luau", "boot/**"]),
            exclude: strings(&[
                "src/tools/**", "src/tools", "src/Data.luau", "src/*/gen", "*.spec.luau", "vendor",
                "src/tool/**", "SRC2/x", "../other/**",
            ]),
            root: Some(root.clone()),
            ..Options::default()
        };
        assert_eq!(
            options.missing_paths(),
            vec![("scripts", "boot/**"), ("exclude", "src/tool/**"), ("exclude", "SRC2/x"), ("exclude", "../other/**")]
        );
        // No rime.toml, nothing to be below.
        assert_eq!(Options { root: None, ..options }.missing_paths(), Vec::new());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn wild_matches_names_and_paths() {
        let name = |p: &str, t: &str| wild(p.as_bytes(), t.as_bytes(), None);
        assert!(name("require", "require"));
        assert!(!name("require", "required"));
        assert!(name("*", ""));
        assert!(name("a*c", "ac") && name("a*c", "abbc") && !name("a*c", "abcd"));
        assert!(name("*.new", "a.b.new") && !name("*.new", "new"));
        assert!(name("*:Connect", "a.b():c.d:Connect"));
        assert!(name("a*b*c", "a-b-b-c") && !name("a*b*c", "a-b-b"));
        assert!(name("**", "anything") && name("a**c", "abc"));
        let path = |p: &str, t: &str| wild(p.as_bytes(), t.as_bytes(), Some(b'/'));
        assert!(path("*.lua", "a.lua") && !path("*.lua", "d/a.lua"));
        assert!(path("**", "d/e/a.lua"));
        assert!(path("d/**/a.lua", "d/a.lua") && path("d/**/a.lua", "d/e/f/a.lua"));
        assert!(!path("d/**/a.lua", "x/d/a.lua"));
        assert!(path("**/a.lua", "a.lua") && path("**/a.lua", "d/a.lua"));
        assert!(path("d/*/a.lua", "d/e/a.lua") && !path("d/*/a.lua", "d/e/f/a.lua"));
        assert!(path("**/t/**", "a/t/b/c.lua") && !path("**/t/**", "a/b/c.lua"));
        assert!(path("d/*.lua", "d/a.lua") && !path("d/*.lua", "d/a.luau"));
        assert!(path("d/**", "d/a.lua") && path("d/**", "d/e/a.lua") && !path("d/**", "e/a.lua"));
        assert!(path("*/**/*.spec.lua", "a/b/c/x.spec.lua") && !path("*/**/*.spec.lua", "x.spec.lua"));
    }
}
