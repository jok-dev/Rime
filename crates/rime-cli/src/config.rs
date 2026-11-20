//! rime.toml — per-rule severity configuration.
//!
//! Every rule is ON at its default severity: the theorem is a property of
//! the default configuration. The exceptions are the few rules on
//! `rime_diag::OFF_BY_DEFAULT`, each a convention a project has to choose:
//! they report nothing until this file gives them a level (or `--only`
//! names them). A rule's level sets the severity of its
//! reports — `"warn"` keeps an error rule visible without failing the run,
//! `"off"` suppresses it, and `"error"` makes a warning rule fail the run
//! (Selene fails on its warning lints; this is how a project asks Rime for
//! the same). Downgrading a soundness rule is the user's explicit, greppable
//! choice, same philosophy as `--!trusted`. R0901 internal-error takes no
//! setting.
//!
//! This is a CLI-layer concern on purpose: `rime_check::check_source` stays
//! severity-pure, and the corpus harness snapshots the checker's own output,
//! never a configured view of it.
//!
//! ```toml
//! [conservative]
//! default = "warn"                   # every rule's conservative reports
//!
//! [rules]
//! definite-assignment = "warn"
//! unreachable-code    = "off"
//! R0041               = "warn"       # raw codes always work
//! possible-nil-use    = { level = "error", conservative = "off" }
//! ```
//!
//! One rule is the other way round: R0312 module-scope-computation is OFF
//! until it is given a level, and it alone takes settings of its own. Those
//! do reach the checker (`rule_options`), because they decide what the rule
//! looks for, not how loud its reports are:
//!
//! ```toml
//! [rules.module-scope-computation]
//! level = "error"
//! allow-builtin = ["luau", "roblox-datatypes"]
//! allow = ["Signal.new", "*:IsServer"]
//! deny = ["Random.new"]
//! allow-loops = false
//! scripts = ["*.server.lua", "*.client.lua"]
//! exclude = ["tools/**"]
//! ```
//!
//! One setting outside `[rules]` reaches the checker the same way: it
//! decides what the player-map rules look for.
//!
//! ```toml
//! [names]
//! player = ["player", "plr", "client"]   # variables that hold a Player
//! ```

use rime_check::rules::module_scope;
use rime_diag::{Code, Diagnostic, Severity};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleSetting {
    Error,
    Warn,
    Off,
}

use rime_diag::RULE_NAMES;

/// R0312, the rule with settings of its own.
const MODULE_SCOPE: u16 = 312;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Config {
    rules: HashMap<u16, RuleSetting>,
    /// `[rules] name = { conservative = "..." }` — a further downgrade that
    /// applies only to diagnostics flagged `conservative` (Rime's stated
    /// limitations, never the user's bug). Precedence: this, then
    /// `conservative_default`, then the rule's own level; the lower wins,
    /// so neither ever raises a report above the rule's level.
    conservative: HashMap<u16, RuleSetting>,
    /// `[conservative] default = "..."` — the same downgrade for every rule
    /// without its own `conservative` entry.
    conservative_default: Option<RuleSetting>,
    /// Directory prefixes whose files produce no reports (top-level
    /// `ignore = ["vendor", ...]`, relative to rime.toml's directory,
    /// canonicalized at load). Interfaces are still extracted from ignored
    /// files in project mode, so cross-file checking of kept files is
    /// unaffected — only the reports are dropped.
    ignore: Vec<PathBuf>,
    /// `project = "place.project.json"` — the Rojo project file to read,
    /// relative to rime.toml's directory. Unset: the nearest one above the
    /// checked path is found (see `workspace::choose_project`). Not
    /// canonicalized: a file that is not there must be an error when it is
    /// loaded, not a silent run without cross-file checks.
    project: Option<PathBuf>,
    /// `[requires] trust = [...]` — DataModel-path patterns whose requires
    /// the user vouches for when resolution misses. Passed to the resolver.
    trust_requires: Vec<String>,
    /// `[std] files = [...]` — selene std-lib YAML files that REPLACE the
    /// bundled Luau+Roblox set (see `build_std_names`). Anchored to
    /// rime.toml's directory at parse time, like `ignore`, but WITHOUT
    /// canonicalize: a missing file must surface as an error at
    /// `build_std_names` time, not silently match nothing.
    std_files: Vec<PathBuf>,
    /// `[std] globals = [...]` — bare extra global names added on top of
    /// whichever set `std_files` selects (bundled, if empty).
    std_globals: Vec<String>,
    /// `rime check --only <rules>`: report exactly these codes, at their
    /// default severity — the `[rules]`/`[conservative]` levels above are
    /// ignored for them, so a rule the toml turns off can still be looked
    /// at without editing the toml. Everything else is dropped (and counted
    /// as ignored). `ignore`, `trust`, `std`, and inline allows still apply.
    only: Option<HashSet<u16>>,
    /// `[rules.module-scope-computation]`'s `allow`, `allow-builtin`,
    /// `deny`, `allow-loops`, `scripts` and `exclude`, each at its default
    /// when unset. Whether the rule runs at
    /// all is its level (see `rule_options`).
    module_scope: module_scope::Options,
    /// `[names] player = [...]` — the variable names the player-map rules
    /// (R0802, R0803) read as "this holds a Player". None: the rules'
    /// own default.
    player_names: Option<Vec<String>>,
}

impl Config {
    /// Where rime.toml is: the explicit --config path, else the nearest
    /// `rime.toml` walking up from `start`. None when neither exists.
    pub fn discover_path(explicit: Option<&Path>, start: &Path) -> Option<PathBuf> {
        match explicit {
            Some(p) => Some(p.to_path_buf()),
            None => start
                .ancestors()
                .map(|d| d.join("rime.toml"))
                .find(|p| p.is_file()),
        }
    }

    /// Locate rime.toml: explicit --config path wins; otherwise walk up from
    /// `start`. No file at all = default config.
    pub fn discover(explicit: Option<&Path>, start: &Path) -> Result<Config, String> {
        match Self::discover_path(explicit, start) {
            None => Ok(Config::default()),
            Some(p) => {
                let text = std::fs::read_to_string(&p)
                    .map_err(|e| format!("could not read {}: {e}", p.display()))?;
                let dir = p.parent().unwrap_or(Path::new("."));
                Config::parse(&text, dir).map_err(|e| format!("{}: {e}", p.display()))
            }
        }
    }

    pub fn parse(text: &str, dir: &Path) -> Result<Config, String> {
        let value: toml::Value = text.parse().map_err(|e| format!("invalid TOML: {e}"))?;

        let mut ignore = Vec::new();
        if let Some(list) = value.get("ignore") {
            let Some(list) = list.as_array() else {
                return Err("`ignore` must be an array of paths".to_string());
            };
            for entry in list {
                let Some(s) = entry.as_str() else {
                    return Err("`ignore` entries must be strings".to_string());
                };
                let anchored = dir.join(s);
                // Canonicalize so prefix matching lines up with the
                // canonical file paths project mode reports (and gets
                // Windows on-disk casing for free). A missing entry is kept
                // as-anchored — harmless, matches nothing.
                ignore.push(anchored.canonicalize().unwrap_or(anchored));
            }
        }

        let mut project = None;
        if let Some(file) = value.get("project") {
            let Some(s) = file.as_str().filter(|s| !s.is_empty()) else {
                return Err("`project` must be the path of a Rojo project file".to_string());
            };
            project = Some(dir.join(s));
        }

        let mut trust_requires = Vec::new();
        if let Some(req) = value.get("requires") {
            if let Some(list) = req.get("trust") {
                let Some(list) = list.as_array() else {
                    return Err("[requires] trust must be an array of paths".to_string());
                };
                for entry in list {
                    let Some(s) = entry.as_str() else {
                        return Err("[requires] trust entries must be strings".to_string());
                    };
                    if !s.starts_with("game.") {
                        return Err(format!(
                            "[requires] trust entry `{s}` must start with `game.`"
                        ));
                    }
                    trust_requires.push(s.to_string());
                }
            }
        }

        let mut std_files = Vec::new();
        let mut std_globals = Vec::new();
        if let Some(std_val) = value.get("std") {
            let Some(std_val) = std_val.as_table() else {
                return Err(
                    "`std` must be a table — use [std] with `files` and/or `globals`; \
                     selene-style `std = \"...\"` strings are not supported"
                        .to_string(),
                );
            };
            if let Some(files) = std_val.get("files") {
                let Some(list) = files.as_array() else {
                    return Err("[std] files must be an array of paths".to_string());
                };
                for entry in list {
                    let Some(s) = entry.as_str() else {
                        return Err("[std] files entries must be strings".to_string());
                    };
                    // No canonicalize (unlike `ignore`): a missing std file
                    // must error at `build_std_names` time, not silently
                    // match nothing.
                    std_files.push(dir.join(s));
                }
            }
            if let Some(globals) = std_val.get("globals") {
                let Some(list) = globals.as_array() else {
                    return Err("[std] globals must be an array of strings".to_string());
                };
                for entry in list {
                    let Some(s) = entry.as_str() else {
                        return Err("[std] globals entries must be strings".to_string());
                    };
                    std_globals.push(s.to_string());
                }
            }
        }

        let mut player_names = None;
        if let Some(names) = value.get("names") {
            let Some(names) = names.as_table() else {
                return Err("`names` must be a table — use [names] with `player`".to_string());
            };
            for (k, v) in names {
                match k.as_str() {
                    "player" => {
                        let bad = || "[names] player must be an array of names".to_string();
                        let mut list = Vec::new();
                        for entry in v.as_array().ok_or_else(bad)? {
                            let name = entry.as_str().ok_or_else(bad)?;
                            if name.is_empty() {
                                return Err("[names] player has an empty name".to_string());
                            }
                            list.push(name.to_string());
                        }
                        player_names = Some(list);
                    }
                    other => return Err(format!("[names]: unknown key `{other}` (only `player`)")),
                }
            }
        }

        let mut conservative_default = None;
        if let Some(c) = value.get("conservative") {
            let Some(c) = c.as_table() else {
                return Err(
                    "`conservative` must be a table — use [conservative] with `default`".to_string(),
                );
            };
            for (k, v) in c {
                match k.as_str() {
                    "default" => {
                        conservative_default = Some(setting_of(v).ok_or_else(|| {
                            "[conservative] default: expected \"error\", \"warn\", or \"off\""
                                .to_string()
                        })?);
                    }
                    other => {
                        return Err(format!("[conservative]: unknown key `{other}` (only `default`)"))
                    }
                }
            }
        }
        let mut conservative = HashMap::new();
        let mut rules = HashMap::new();
        // `scripts` and `exclude` paths are relative to rime.toml, in the
        // same canonical form as the paths the CLI walks (see `ignore`
        // above). `--config name.toml` gives an empty directory, which is
        // the current one.
        let here = if dir.as_os_str().is_empty() { Path::new(".") } else { dir };
        let mut module_scope = module_scope::Options {
            root: Some(here.canonicalize().unwrap_or_else(|_| here.to_path_buf())),
            ..Default::default()
        };
        let Some(table) = value.get("rules") else {
            return Ok(Config {
                rules,
                conservative,
                conservative_default,
                ignore,
                project,
                trust_requires,
                std_files,
                std_globals,
                only: None,
                module_scope,
                player_names,
            });
        };
        let Some(table) = table.as_table() else {
            return Err("[rules] must be a table".to_string());
        };
        for (key, val) in table {
            let code = rule_code(key)
                .ok_or_else(|| format!("unknown rule `{key}` (use a name or a raw R#### code)"))?;
            if let Some(t) = val.as_table() {
                let has_options = code == MODULE_SCOPE;
                for (k, v) in t {
                    let strings = || -> Result<Vec<String>, String> {
                        let bad = || format!("rule `{key}`: `{k}` must be an array of strings");
                        let mut out = Vec::new();
                        for entry in v.as_array().ok_or_else(bad)? {
                            let entry = entry.as_str().ok_or_else(bad)?;
                            if entry.is_empty() {
                                return Err(format!("rule `{key}`: `{k}` has an empty entry"));
                            }
                            out.push(entry.to_string());
                        }
                        Ok(out)
                    };
                    // A callee is matched as the checker writes it, with no
                    // spaces and no call at the end: an entry with either
                    // could never match, and nothing would say so.
                    let callees = || -> Result<Vec<String>, String> {
                        let entries = strings()?;
                        for entry in &entries {
                            if entry.contains(char::is_whitespace) {
                                return Err(format!(
                                    "rule `{key}`: `{k}` entry `{entry}` has a space in it, \
                                     and a callee is matched without any"
                                ));
                            }
                            if let Some(bare) = entry.strip_suffix("()") {
                                return Err(format!(
                                    "rule `{key}`: `{k}` entry `{entry}` ends in `()`: write `{bare}`, \
                                     the callee without its call"
                                ));
                            }
                        }
                        Ok(entries)
                    };
                    let paths = || -> Result<Vec<String>, String> {
                        Ok(strings()?.iter().map(|entry| module_scope::path_pattern(entry)).collect())
                    };
                    match k.as_str() {
                        "allow" if has_options => {
                            module_scope.allow = callees()?;
                        }
                        "deny" if has_options => {
                            module_scope.deny = callees()?;
                        }
                        "allow-builtin" if has_options => {
                            module_scope.allow_builtin.clear();
                            for name in strings()? {
                                let list = module_scope::Builtin::from_name(&name).ok_or_else(|| {
                                    let known: Vec<String> = module_scope::Builtin::ALL
                                        .iter()
                                        .map(|b| format!("`{}`", b.name()))
                                        .collect();
                                    format!(
                                        "rule `{key}`: unknown built-in list `{name}` (the lists are {})",
                                        known.join(", ")
                                    )
                                })?;
                                if !module_scope.allow_builtin.contains(&list) {
                                    module_scope.allow_builtin.push(list);
                                }
                            }
                        }
                        "scripts" if has_options => {
                            module_scope.scripts = paths()?;
                        }
                        "exclude" if has_options => {
                            module_scope.exclude = paths()?;
                        }
                        "allow-loops" if has_options => {
                            module_scope.allow_loops = v
                                .as_bool()
                                .ok_or_else(|| format!("rule `{key}`: `{k}` must be true or false"))?;
                        }
                        "level" | "conservative" => {
                            let setting = setting_of(v).ok_or_else(|| {
                                format!("rule `{key}`: `{k}` expected \"error\", \"warn\", or \"off\"")
                            })?;
                            if k == "level" {
                                rules.insert(code, setting);
                            } else {
                                conservative.insert(code, setting);
                            }
                        }
                        other if has_options => {
                            return Err(format!(
                                "rule `{key}`: unknown key `{other}` (use `level`, \
                                 `conservative`, `allow`, `allow-builtin`, `deny`, \
                                 `allow-loops`, `scripts`, `exclude`)"
                            ))
                        }
                        other => {
                            return Err(format!(
                                "rule `{key}`: unknown key `{other}` (use `level` and/or `conservative`)"
                            ))
                        }
                    }
                }
                // Settings for a rule that stays off would do nothing, and
                // nothing would say so. That goes for `conservative` as
                // much as for a rule's own settings.
                if rime_diag::off_by_default(Code(code)) && !t.is_empty() && !t.contains_key("level") {
                    return Err(format!(
                        "rule `{key}` is off by default: set `level` to \"warn\" or \"error\" \
                         beside its other settings to turn it on"
                    ));
                }
                continue;
            }
            let setting = setting_of(val).ok_or_else(|| {
                format!(
                    "rule `{key}`: expected \"error\", \"warn\", \"off\", or \
                     {{ level = ..., conservative = ... }}"
                )
            })?;
            rules.insert(code, setting);
        }
        Ok(Config {
            rules,
            conservative,
            conservative_default,
            ignore,
            project,
            trust_requires,
            std_files,
            std_globals,
            only: None,
            module_scope,
            player_names,
        })
    }

    /// What the checker itself has to be told: the settings that decide
    /// what a rule looks for. Levels stay here (`apply`), with one
    /// exception: R0312 runs only when rime.toml gives it a level other
    /// than "off", or `--only` names it; otherwise the checker does not
    /// look for it at all.
    pub fn rule_options(&self) -> rime_check::RuleOptions {
        let on = match &self.only {
            Some(only) => only.contains(&MODULE_SCOPE),
            None => matches!(
                self.rules.get(&MODULE_SCOPE),
                Some(RuleSetting::Error | RuleSetting::Warn)
            ),
        };
        let mut options = rime_check::RuleOptions {
            module_scope: on.then(|| self.module_scope.clone()),
            ..Default::default()
        };
        if let Some(names) = &self.player_names {
            options.retention.player_names = names.clone();
        }
        options
    }

    /// One line for each `scripts` / `exclude` entry of R0312 that names a
    /// path which is not below rime.toml, when the rule runs: such an
    /// entry leaves nothing out, and the run would not say so.
    pub fn rule_option_warnings(&self) -> Vec<String> {
        let Some(options) = self.rule_options().module_scope else { return Vec::new() };
        options
            .missing_paths()
            .into_iter()
            .map(|(setting, entry)| {
                let fixed: Vec<&str> = entry.split('/').take_while(|part| !part.contains('*')).collect();
                format!(
                    "[rules.module-scope-computation] `{setting}` entry `{entry}` matches nothing: \
                     there is no `{}` below rime.toml",
                    fixed.join("/")
                )
            })
            .collect()
    }

    /// The Rojo project file rime.toml names (`project = "..."`), if any.
    pub fn project_file(&self) -> Option<&Path> {
        self.project.as_deref()
    }

    pub fn trusted_requires(&self) -> Vec<String> {
        self.trust_requires.clone()
    }

    /// Build the `StdNames` set this config selects: `files` empty starts
    /// from a clone of the bundled Luau+Roblox set; non-empty REPLACES it
    /// entirely with `StdNames::from_files` (a missing/invalid file errors
    /// here, not silently). `globals` extends whichever set that produced.
    pub fn build_std_names(&self) -> Result<rime_std::StdNames, String> {
        let mut std = if self.std_files.is_empty() {
            rime_std::StdNames::bundled().clone()
        } else {
            rime_std::StdNames::from_files(&self.std_files)?
        };
        std.add_globals(self.std_globals.clone());
        Ok(std)
    }

    /// Whether a file's reports are suppressed by an `ignore` entry. A plain
    /// prefix test: entries are canonicalized at load, and every path the
    /// CLI hands in is walked from a canonical root (`rime_check::walk`),
    /// so the two are already in the same form. No per-file syscall here —
    /// the old canonicalize-on-miss fallback ran once per NON-ignored file.
    pub fn ignored(&self, path: &Path) -> bool {
        self.ignore.iter().any(|i| path.starts_with(i))
    }

    /// Apply to one file's diagnostics. A rule's `level` is the severity its
    /// reports get: `off` drops them, `warn` lowers an error rule, `error`
    /// raises a warning rule so it fails the run. A `conservative` setting
    /// (per rule, else `[conservative] default`) then applies to diagnostics
    /// flagged conservative, on top of the rule's level, and only ever
    /// lowers. The second return is how many were dropped, for the summary's
    /// ignored count. Reports of an off-by-default rule that has no level
    /// go too, and are not in that count.
    pub fn apply(&self, diags: Vec<Diagnostic>) -> (Vec<Diagnostic>, usize) {
        let before = diags.len();
        // Reports of an off-by-default rule nobody turned on. They are not
        // "ignored": no one chose to hide them, so the summary leaves them
        // out.
        let mut never_on = 0;
        let kept: Vec<Diagnostic> = diags
            .into_iter()
            .filter_map(|mut d| {
                // R0901 says Rime crashed. No setting hides or softens
                // that — `[rules] internal-error = "off"` parses (it is a
                // rule name like any other) and does nothing.
                if d.is_internal_error() {
                    return Some(d);
                }
                // `--only` decides alone: listed rules keep their default
                // severity, unlisted ones go. R0900 always stays: "none of
                // this file was checked" changes what every other rule's
                // silence means (an in-progress edit on a large codebase
                // silently dissolved a require ring under `--only
                // circular-require` before this line existed).
                if let Some(only) = &self.only {
                    return (only.contains(&d.code.0) || d.code.0 == 900).then_some(d);
                }
                let level = self.rules.get(&d.code.0).copied();
                // An off-by-default rule is on only with a level of its
                // own. `[conservative] default` says how loud some reports
                // are, not whether a rule runs.
                if level.is_none() && rime_diag::off_by_default(d.code) {
                    never_on += 1;
                    return None;
                }
                let conservative = if d.conservative {
                    self.conservative.get(&d.code.0).copied().or(self.conservative_default)
                } else {
                    None
                };
                match level {
                    Some(RuleSetting::Off) => return None,
                    Some(RuleSetting::Warn) => d.severity = Severity::Warning,
                    Some(RuleSetting::Error) => d.severity = Severity::Error,
                    None => {}
                }
                // The conservative setting sits on top of the rule's level
                // and never raises, so the lower of the two wins.
                match conservative {
                    Some(RuleSetting::Off) => return None,
                    Some(RuleSetting::Warn) => d.severity = Severity::Warning,
                    Some(RuleSetting::Error) | None => {}
                }
                Some(d)
            })
            .collect();
        let dropped = before - kept.len() - never_on;
        (kept, dropped)
    }

    /// `--only`: report exactly `codes`, at their default severity (see the
    /// field). Calling it twice unions the sets.
    pub fn restrict_to(&mut self, codes: impl IntoIterator<Item = u16>) {
        self.only.get_or_insert_with(HashSet::new).extend(codes);
    }
}

/// One `--only` value: comma-separated rule names or raw `R####` codes,
/// resolved exactly like rime.toml's `[rules]` keys (custom project rules
/// included, once registered). Errors name the first unknown entry.
pub fn parse_only(spec: &str) -> Result<Vec<u16>, String> {
    let mut codes = Vec::new();
    for entry in spec.split(',').map(str::trim) {
        if entry.is_empty() {
            return Err("expected a rule name (e.g. `circular-require` or `R0509`)".to_string());
        }
        match rule_code(entry) {
            Some(code) => codes.push(code),
            None => return Err(format!("unknown rule `{entry}`")),
        }
    }
    Ok(codes)
}

/// `"error"` / `"warn"` / `"off"` as a setting; None for anything else.
fn setting_of(v: &toml::Value) -> Option<RuleSetting> {
    match v.as_str()? {
        "error" => Some(RuleSetting::Error),
        "warn" => Some(RuleSetting::Warn),
        "off" => Some(RuleSetting::Off),
        _ => None,
    }
}

fn rule_code(key: &str) -> Option<u16> {
    if let Some((_, code)) = RULE_NAMES.iter().find(|(name, _)| *name == key) {
        return Some(*code);
    }
    // A raw `R####` always spells the core code it looks like — the registry
    // never gets to reinterpret one (`Engine::load` refuses to register a
    // core-looking stem, and this ordering holds even if one slipped in).
    if let Some(code) = key.strip_prefix('R').and_then(|d| d.parse().ok()) {
        return Some(code);
    }
    // Custom project rules register their names at load, before rime.toml
    // is parsed (rime-cli sequences it that way).
    rime_diag::custom_code(key).map(|c| c.0)
}

/// Exit-code policy: only error-severity diagnostics fail the run.
pub fn has_errors(diags: &[Diagnostic]) -> bool {
    diags.iter().any(|d| d.severity == Severity::Error)
}

#[allow(dead_code)]
fn _code_is_u16(c: Code) -> u16 {
    c.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use rime_diag::{Code, Diagnostic};

    fn diag(code: u16) -> Diagnostic {
        Diagnostic::error(Code(code), "x")
    }

    fn cons(code: u16) -> Diagnostic {
        Diagnostic::error(Code(code), "x").conservative("a stated limitation")
    }

    #[test]
    fn only_drops_every_unlisted_rule() {
        let mut c = Config::default();
        c.restrict_to([509]);
        let (out, dropped) = c.apply(vec![diag(22), diag(509), diag(13)]);
        assert_eq!(dropped, 2);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].code, Code(509));
    }

    #[test]
    fn only_ignores_the_toml_level_for_listed_rules() {
        let mut c = Config::parse(
            "[rules]\ncircular-require = \"off\"\nbroken-require = \"warn\"\n[conservative]\ndefault = \"off\"\n",
            Path::new("."),
        )
        .unwrap();
        c.restrict_to([509, 501]);
        let (out, dropped) = c.apply(vec![diag(509), cons(501), diag(22)]);
        assert_eq!(dropped, 1, "only the unlisted R0022 goes");
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|d| d.severity == Severity::Error), "default severity, not the toml's warn");
    }

    #[test]
    fn only_always_keeps_parse_errors() {
        // "none of this file was checked" changes what every other rule's
        // silence means — an `--only` run must still show it.
        let mut c = Config::default();
        c.restrict_to([509]);
        let (out, dropped) = c.apply(vec![diag(900), diag(22)]);
        assert_eq!(dropped, 1);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].code, Code(900));
    }

    #[test]
    fn nothing_hides_or_downgrades_an_internal_error() {
        let ice = || Diagnostic::error(rime_diag::INTERNAL_ERROR, "internal error").conservative("x");
        let off = Config::parse(
            "[rules]\ninternal-error = \"off\"\nR0901 = \"warn\"\n[conservative]\ndefault = \"off\"\n",
            Path::new("."),
        )
        .unwrap();
        let (out, dropped) = off.apply(vec![ice()]);
        assert_eq!(dropped, 0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].severity, Severity::Error);

        let mut only = Config::default();
        only.restrict_to([509]);
        let (out, dropped) = only.apply(vec![ice(), diag(22)]);
        assert_eq!(dropped, 1, "only the unlisted R0022 goes");
        assert!(out[0].is_internal_error());
    }

    #[test]
    fn an_off_by_default_rule_reports_nothing_until_it_has_a_level() {
        for &code in rime_diag::OFF_BY_DEFAULT {
            // No setting: gone, and not counted. Nobody ignored anything.
            let (out, dropped) = Config::default().apply(vec![diag(code), diag(22)]);
            assert_eq!(out.len(), 1, "R{code:04}");
            assert_eq!(out[0].code, Code(22));
            assert_eq!(dropped, 0, "R{code:04}");

            for (level, severity) in [("warn", Severity::Warning), ("error", Severity::Error)] {
                let c = Config::parse(&format!("[rules]\nR{code:04} = \"{level}\"\n"), Path::new(".")).unwrap();
                let (out, dropped) = c.apply(vec![diag(code)]);
                assert_eq!(dropped, 0);
                assert_eq!(out.len(), 1, "R{code:04} = {level}");
                assert_eq!(out[0].severity, severity, "R{code:04} = {level}");
            }

            // An explicit "off" is the project's own choice, counted like
            // any other rule it turns off.
            let c = Config::parse(&format!("[rules]\nR{code:04} = \"off\"\n"), Path::new(".")).unwrap();
            assert_eq!(c.apply(vec![diag(code)]), (vec![], 1), "R{code:04} = off");

            // `conservative` says how loud some reports are, not whether
            // the rule is on. `[conservative] default` therefore leaves it
            // off, and a `conservative` of its own with no `level` is
            // refused: it would do nothing, and nothing would say so.
            let c = Config::parse("[conservative]\ndefault = \"warn\"\n", Path::new(".")).unwrap();
            assert_eq!(c.apply(vec![cons(code)]), (vec![], 0), "R{code:04}, conservative default only");
            let err = Config::parse(&format!("[rules]\nR{code:04} = {{ conservative = \"warn\" }}\n"), Path::new("."))
                .err()
                .unwrap_or_else(|| panic!("R{code:04}: `conservative` with no `level` must be refused"));
            assert!(err.contains("off by default"), "R{code:04}: {err}");

            // The table form's `level` turns it on like the plain form.
            let c = Config::parse(
                &format!("[rules]\nR{code:04} = {{ level = \"error\", conservative = \"warn\" }}\n"),
                Path::new("."),
            )
            .unwrap();
            let (out, _) = c.apply(vec![diag(code), cons(code)]);
            assert_eq!(
                out.iter().map(|d| d.severity).collect::<Vec<_>>(),
                [Severity::Error, Severity::Warning],
                "R{code:04}"
            );
        }
    }

    #[test]
    fn only_turns_an_off_by_default_rule_on() {
        for &code in rime_diag::OFF_BY_DEFAULT {
            let mut c = Config::default();
            c.restrict_to([code]);
            let (out, dropped) = c.apply(vec![diag(code), diag(22)]);
            assert_eq!(out.len(), 1, "R{code:04}");
            assert_eq!(out[0].code, Code(code));
            assert_eq!(dropped, 1, "the unlisted R0022");
        }
    }

    #[test]
    fn player_names_default_until_the_project_sets_them() {
        for text in ["", "[rules]\nunused-local = \"off\"\n", "[names]\n"] {
            let c = Config::parse(text, Path::new(".")).unwrap();
            assert_eq!(c.rule_options().retention.player_names, ["player", "plr"], "{text:?}");
        }
        assert_eq!(Config::default().rule_options(), rime_check::RuleOptions::default());

        let c = Config::parse("[names]\nplayer = [\"client\", \"*Player\"]\n", Path::new(".")).unwrap();
        assert_eq!(c.rule_options().retention.player_names, ["client", "*Player"]);

        // An empty list is a setting: no name means a Player.
        let c = Config::parse("[names]\nplayer = []\n", Path::new(".")).unwrap();
        assert_eq!(c.rule_options().retention.player_names, Vec::<String>::new());
    }

    #[test]
    fn player_names_reject_bad_shapes() {
        for (text, expected) in [
            ("names = [\"player\"]\n", "`names` must be a table"),
            ("[names]\nplayer = \"player\"\n", "[names] player must be an array of names"),
            ("[names]\nplayer = [1]\n", "[names] player must be an array of names"),
            ("[names]\nplayer = [\"\"]\n", "empty name"),
            ("[names]\nplayers = [\"plr\"]\n", "unknown key `players`"),
        ] {
            let err = Config::parse(text, Path::new(".")).expect_err(text);
            assert!(err.contains(expected), "text: {text}\nerror: {err}");
        }
    }

    #[test]
    fn parse_only_takes_names_and_raw_codes_comma_separated() {
        assert_eq!(parse_only("circular-require, R0501,unused-local").unwrap(), vec![509, 501, 13]);
    }

    #[test]
    fn parse_only_rejects_unknown_names() {
        let err = parse_only("circular-require,nope").unwrap_err();
        assert!(err.contains("unknown rule `nope`"), "{err}");
        assert!(parse_only("").is_err());
    }

    #[test]
    fn a_removed_rule_name_is_an_unknown_rule() {
        // Removed outright, with no alias left behind: a rime.toml that
        // still names one says so instead of silently doing nothing.
        for name in ["nil-require-stub", "module-file-invalid-name"] {
            let err = Config::parse(&format!("[rules]\n{name} = \"off\"\n"), Path::new("."))
                .expect_err(name);
            assert!(err.contains(&format!("unknown rule `{name}`")), "{err}");
        }
    }

    #[test]
    fn rule_table_form_downgrades_conservative_only() {
        let c = Config::parse(
            "[rules]\npossible-nil-use = { level = \"error\", conservative = \"off\" }\n",
            Path::new("."),
        )
        .unwrap();
        let (out, dropped) = c.apply(vec![diag(201), cons(201)]);
        assert_eq!(dropped, 1);
        assert_eq!(out.len(), 1);
        assert!(!out[0].conservative);
        assert_eq!(out[0].severity, Severity::Error);
    }

    #[test]
    fn rule_table_level_is_optional() {
        let c = Config::parse("[rules]\nuse-before-guard = { conservative = \"warn\" }\n", Path::new("."))
            .unwrap();
        let (out, _) = c.apply(vec![diag(202), cons(202)]);
        assert_eq!(out[0].severity, Severity::Error); // firm: untouched (no level set)
        assert_eq!(out[1].severity, Severity::Warning); // conservative: warn
    }

    #[test]
    fn conservative_default_covers_every_rule() {
        let c = Config::parse("[conservative]\ndefault = \"warn\"\n", Path::new(".")).unwrap();
        let (out, dropped) = c.apply(vec![diag(201), cons(201), cons(202)]);
        assert_eq!(dropped, 0);
        let sev: Vec<Severity> = out.iter().map(|d| d.severity).collect();
        assert_eq!(sev, vec![Severity::Error, Severity::Warning, Severity::Warning]);
    }

    #[test]
    fn per_rule_conservative_beats_the_default() {
        let c = Config::parse(
            "[conservative]\ndefault = \"warn\"\n[rules]\nuse-before-guard = { conservative = \"off\" }\n",
            Path::new("."),
        )
        .unwrap();
        let (out, dropped) = c.apply(vec![cons(201), cons(202)]);
        assert_eq!(dropped, 1);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].code, Code(201));
        assert_eq!(out[0].severity, Severity::Warning);
    }

    #[test]
    fn rule_level_off_drops_conservative_too() {
        let c = Config::parse("[rules]\npossible-nil-use = \"off\"\n", Path::new(".")).unwrap();
        let (out, dropped) = c.apply(vec![diag(201), cons(201)]);
        assert!(out.is_empty());
        assert_eq!(dropped, 2);
    }

    #[test]
    fn conservative_setting_never_upgrades() {
        // Rule at warn, conservative at error: the lower wins.
        let c = Config::parse(
            "[rules]\npossible-nil-use = { level = \"warn\", conservative = \"error\" }\n",
            Path::new("."),
        )
        .unwrap();
        let (out, _) = c.apply(vec![cons(201), diag(201)]);
        assert_eq!(out[0].severity, Severity::Warning);
        assert_eq!(out[1].severity, Severity::Warning);
        // A warning-severity diagnostic stays a warning under "error".
        let c = Config::parse("[conservative]\ndefault = \"error\"\n", Path::new(".")).unwrap();
        let w = Diagnostic::warning(Code(13), "x").conservative("limitation");
        let (out, _) = c.apply(vec![w]);
        assert_eq!(out[0].severity, Severity::Warning);
    }

    fn warn(code: u16) -> Diagnostic {
        Diagnostic::warning(Code(code), "x")
    }

    #[test]
    fn rule_level_error_raises_a_warning_rule() {
        // Selene fails the run on its warning lints; a project that wants
        // the same from Rime raises the rule.
        let c = Config::parse(
            "[rules]\nunused-local = \"error\"\nR0024 = { level = \"error\" }\n",
            Path::new("."),
        )
        .unwrap();
        let (out, dropped) = c.apply(vec![warn(13), warn(24), warn(702)]);
        assert_eq!(dropped, 0);
        let sev: Vec<Severity> = out.iter().map(|d| d.severity).collect();
        assert_eq!(sev, vec![Severity::Error, Severity::Error, Severity::Warning]);
        assert!(has_errors(&out));
    }

    #[test]
    fn a_warning_rule_without_a_level_stays_a_warning() {
        let c = Config::parse("[rules]\nunused-local = { conservative = \"warn\" }\n", Path::new("."))
            .unwrap();
        let (out, _) = c.apply(vec![warn(13), warn(13).conservative("limitation")]);
        assert!(out.iter().all(|d| d.severity == Severity::Warning));
        assert!(!has_errors(&out));
    }

    #[test]
    fn a_raised_rule_still_takes_the_conservative_downgrade() {
        // The conservative setting sits on top of the level and only
        // lowers: raised firm reports block, conservative ones do not.
        let c = Config::parse(
            "[rules]\nunused-local = { level = \"error\", conservative = \"warn\" }\nempty-loop = \"error\"\n[conservative]\ndefault = \"off\"\n",
            Path::new("."),
        )
        .unwrap();
        let (out, dropped) = c.apply(vec![
            warn(13),
            warn(13).conservative("limitation"),
            warn(702),
            warn(702).conservative("limitation"),
        ]);
        assert_eq!(dropped, 1, "[conservative] default = off drops the conservative R0702");
        let got: Vec<(u16, Severity)> = out.iter().map(|d| (d.code.0, d.severity)).collect();
        assert_eq!(
            got,
            vec![(13, Severity::Error), (13, Severity::Warning), (702, Severity::Error)]
        );
    }

    #[test]
    fn a_custom_rule_can_be_raised_too() {
        let code = rime_diag::register_custom("config-tests-raised");
        let c = Config::parse("[rules]\nconfig-tests-raised = \"error\"\n", Path::new(".")).unwrap();
        let (out, _) = c.apply(vec![Diagnostic::warning(code, "x")]);
        assert_eq!(out[0].severity, Severity::Error);
    }

    #[test]
    fn conservative_config_rejects_bad_shapes() {
        for (text, expected) in [
            ("[rules]\npossible-nil-use = { lvl = \"warn\" }\n", "unknown key"),
            (
                "[rules]\npossible-nil-use = { conservative = \"loud\" }\n",
                "expected \"error\", \"warn\", or \"off\"",
            ),
            (
                "[rules]\npossible-nil-use = { level = 1 }\n",
                "expected \"error\", \"warn\", or \"off\"",
            ),
            ("[conservative]\ndefault = \"maybe\"\n", "[conservative] default"),
            ("[conservative]\nother = \"warn\"\n", "only `default`"),
            ("conservative = \"warn\"\n", "must be a table"),
        ] {
            let err = Config::parse(text, Path::new(".")).expect_err(&format!("accepted: {text}"));
            assert!(err.contains(expected), "text: {text}\nerror: {err}\nexpected substring: {expected}");
        }
    }

    #[test]
    fn module_scope_is_off_until_it_has_a_level() {
        assert_eq!(Config::default().rule_options().module_scope, None);
        assert_eq!(Config::parse("", Path::new(".")).unwrap().rule_options().module_scope, None);
        for (text, on) in [
            ("[rules]\nunused-local = \"error\"\n", false),
            ("[rules]\nmodule-scope-computation = \"off\"\n", false),
            ("[rules]\nmodule-scope-computation = \"warn\"\n", true),
            ("[rules]\nmodule-scope-computation = \"error\"\n", true),
            ("[rules]\nR0312 = \"error\"\n", true),
            ("[rules.module-scope-computation]\nlevel = \"error\"\n", true),
            ("[rules.module-scope-computation]\nlevel = \"off\"\nallow = [\"require\"]\n", false),
            ("[rules.module-scope-computation]\nlevel = \"off\"\nconservative = \"warn\"\n", false),
        ] {
            let c = Config::parse(text, Path::new(".")).unwrap();
            assert_eq!(c.rule_options().module_scope.is_some(), on, "{text}");
        }
    }

    #[test]
    fn module_scope_defaults_hold_until_the_project_sets_them() {
        use module_scope::Builtin;
        let c = Config::parse("[rules]\nmodule-scope-computation = \"error\"\n", Path::new(".")).unwrap();
        let got = c.rule_options().module_scope.unwrap();
        assert_eq!(got.allow, Vec::<String>::new());
        assert_eq!(got.allow_builtin, [Builtin::Luau, Builtin::RobloxDatatypes]);
        assert_eq!(got.deny, Vec::<String>::new());
        assert_eq!(got.scripts, module_scope::DEFAULT_SCRIPTS);
        assert_eq!(got.exclude, Vec::<String>::new());
        assert!(!got.allow_loops);

        let c = Config::parse(
            "[rules.module-scope-computation]\nlevel = \"warn\"\nallow = [\"Signal.new\", \"*:IsServer\"]\n\
             allow-builtin = [\"luau\"]\ndeny = [\"Random.new\"]\nallow-loops = true\n\
             scripts = [\"boot/**\"]\nexclude = [\"tools/**\", \"*.spec.luau\"]\n",
            Path::new("."),
        )
        .unwrap();
        let got = c.rule_options().module_scope.unwrap();
        assert_eq!(got.allow, ["Signal.new", "*:IsServer"]);
        assert_eq!(got.allow_builtin, [Builtin::Luau]);
        assert_eq!(got.deny, ["Random.new"]);
        assert_eq!(got.scripts, ["boot/**"]);
        assert_eq!(got.exclude, ["tools/**", "*.spec.luau"]);
        assert!(got.allow_loops);
        assert!(got.root.is_some(), "`scripts` and `exclude` paths are relative to rime.toml's directory");

        // An empty list is a setting: no built-in list at all.
        let c = Config::parse(
            "[rules.module-scope-computation]\nlevel = \"warn\"\nallow-builtin = []\n",
            Path::new("."),
        )
        .unwrap();
        assert_eq!(c.rule_options().module_scope.unwrap().allow_builtin, []);
    }

    #[test]
    fn module_scope_path_patterns_are_read_in_one_form() {
        let c = Config::parse(
            "[rules.module-scope-computation]\nlevel = \"warn\"\n\
             scripts = [\".\\\\boot\\\\**\", \"*.server.luau\"]\n\
             exclude = [\"./src/tools\", \"src\\\\gen\", \"/vendor/**\"]\n\
             allow = [\"Storage:WaitForChild():WaitForChild\"]\n",
            Path::new("."),
        )
        .unwrap();
        let got = c.rule_options().module_scope.unwrap();
        assert_eq!(got.scripts, ["boot/**", "*.server.luau"]);
        assert_eq!(got.exclude, ["src/tools", "src/gen", "vendor/**"]);
        // `()` inside a callee is how a call on a call's result is written.
        assert_eq!(got.allow, ["Storage:WaitForChild():WaitForChild"]);
    }

    #[test]
    fn only_decides_whether_module_scope_runs() {
        let mut c = Config::default();
        c.restrict_to([MODULE_SCOPE]);
        assert!(c.rule_options().module_scope.is_some(), "--only names it: no toml line needed");

        let mut c = Config::parse("[rules]\nmodule-scope-computation = \"error\"\n", Path::new(".")).unwrap();
        c.restrict_to([13]);
        assert_eq!(c.rule_options().module_scope, None, "--only names another rule");
    }

    #[test]
    fn module_scope_rejects_bad_shapes() {
        for (text, expected) in [
            ("[rules.module-scope-computation]\nallow = [\"require\"]\n", "off by default"),
            ("[rules.module-scope-computation]\nscripts = [\"a\"]\n", "off by default"),
            ("[rules.module-scope-computation]\nlevel = \"warn\"\nallow = \"require\"\n", "must be an array of strings"),
            ("[rules.module-scope-computation]\nlevel = \"warn\"\nscripts = [true]\n", "must be an array of strings"),
            ("[rules.module-scope-computation]\nlevel = \"warn\"\nallow = [\"\"]\n", "empty entry"),
            ("[rules.module-scope-computation]\nlevel = \"warn\"\nallow-loops = 1\n", "must be true or false"),
            ("[rules.module-scope-computation]\nlevel = \"warn\"\nloops = true\n", "`allow-loops`, `scripts`, `exclude`"),
            ("[rules.module-scope-computation]\ndeny = [\"Random.new\"]\n", "off by default"),
            ("[rules.module-scope-computation]\nexclude = [\"tools\"]\n", "off by default"),
            ("[rules.module-scope-computation]\nallow-builtin = []\n", "off by default"),
            // `conservative` is about the reports; it does not turn the rule on.
            ("[rules.module-scope-computation]\nconservative = \"warn\"\n", "off by default"),
            ("[rules.module-scope-computation]\nlevel = \"warn\"\ndeny = \"Random.new\"\n", "must be an array of strings"),
            ("[rules.module-scope-computation]\nlevel = \"warn\"\nexclude = [1]\n", "must be an array of strings"),
            ("[rules.module-scope-computation]\nlevel = \"warn\"\nallow-builtin = \"luau\"\n", "must be an array of strings"),
            // A list name Rime does not ship is an error, with the ones it does.
            (
                "[rules.module-scope-computation]\nlevel = \"warn\"\nallow-builtin = [\"lua\"]\n",
                "unknown built-in list `lua` (the lists are `luau`, `roblox-datatypes`)",
            ),
            ("[rules.unused-local]\nlevel = \"warn\"\nexclude = [\"a\"]\n", "use `level` and/or `conservative`"),
            // A callee entry that could never match anything.
            ("[rules.module-scope-computation]\nlevel = \"warn\"\nallow = [\" build\"]\n", "has a space"),
            ("[rules.module-scope-computation]\nlevel = \"warn\"\ndeny = [\"build \"]\n", "has a space"),
            ("[rules.module-scope-computation]\nlevel = \"warn\"\nallow = [\"build()\"]\n", "write `build`"),
            ("[rules.module-scope-computation]\nlevel = \"warn\"\nallow = [\"a:b()\"]\n", "write `a:b`"),
            ("[rules.module-scope-computation]\nlevel = \"loud\"\n", "expected \"error\""),
            // The settings belong to this rule alone.
            ("[rules.unused-local]\nlevel = \"warn\"\nallow-loops = true\n", "use `level` and/or `conservative`"),
            ("[rules.unused-local]\nlevel = \"warn\"\nallow = [\"require\"]\n", "use `level` and/or `conservative`"),
        ] {
            let err = Config::parse(text, Path::new(".")).expect_err(&format!("accepted: {text}"));
            assert!(err.contains(expected), "text: {text}\nerror: {err}\nexpected substring: {expected}");
        }
    }

    #[test]
    fn parses_names_and_raw_codes() {
        let c = Config::parse(
            "[rules]\ndefinite-assignment = \"warn\"\nR0103 = \"off\"\n",
            Path::new("."),
        )
        .unwrap();
        let (out, dropped) = c.apply(vec![diag(101), diag(103), diag(102)]);
        assert_eq!(out.len(), 2);
        assert_eq!(dropped, 1);
        assert_eq!(out[0].severity, Severity::Warning); // 101 downgraded
        assert_eq!(out[1].severity, Severity::Error); // 102 untouched
    }

    #[test]
    fn raw_code_wins_over_registry() {
        // Simulates a bad registration: `R0777` in rime.toml still means
        // Code(777), never the custom rule that answers to that string.
        let custom = rime_diag::register_custom("R0777");
        let c = Config::parse("[rules]\nR0777 = \"off\"\n", Path::new(".")).unwrap();
        let (out, dropped) = c.apply(vec![diag(777), diag(custom.0)]);
        assert_eq!(dropped, 1);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].code, custom, "the registered custom code was untouched");
    }

    #[test]
    fn unknown_rule_is_an_error() {
        assert!(Config::parse("[rules]\nnot-a-rule = \"off\"\n", Path::new(".")).is_err());
    }

    #[test]
    fn empty_config_changes_nothing() {
        let c = Config::parse("", Path::new(".")).unwrap();
        let (out, _) = c.apply(vec![diag(101), cons(201)]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].severity, Severity::Error);
        assert_eq!(out[1].severity, Severity::Error);

        let c = Config::default();
        let (out, _) = c.apply(vec![diag(101), cons(201)]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].severity, Severity::Error);
        assert_eq!(out[1].severity, Severity::Error);
    }

    #[test]
    fn ignore_prefixes_match() {
        let dir = std::env::temp_dir();
        let sub = dir.join("rime-ignore-test").join("vendor");
        std::fs::create_dir_all(&sub).unwrap();
        let c = Config::parse(
            "ignore = [\"rime-ignore-test/vendor\"]\n",
            &dir,
        )
        .unwrap();
        // Callers hand in paths walked from a canonical root (the CLI's
        // contract); `temp_dir()` itself is not canonical on Windows.
        let member = sub.canonicalize().unwrap().join("init.lua");
        assert!(c.ignored(&member));
        let other = dir.join("rime-ignore-test").join("src");
        std::fs::create_dir_all(&other).unwrap();
        assert!(!c.ignored(&other.canonicalize().unwrap().join("a.lua")));
    }

    #[test]
    fn project_key_names_a_file_relative_to_the_config() {
        let dir = Path::new("some").join("where");
        let c = Config::parse("project = \"place.project.json\"\n", &dir).unwrap();
        assert_eq!(c.project_file(), Some(dir.join("place.project.json").as_path()));
        assert_eq!(Config::parse("", &dir).unwrap().project_file(), None);
        // With a [rules] table too: the key is read on both ways out.
        let c = Config::parse("project = \"a.project.json\"\n[rules]\n", &dir).unwrap();
        assert_eq!(c.project_file(), Some(dir.join("a.project.json").as_path()));
        for bad in ["project = 1\n", "project = \"\"\n", "project = [\"a\"]\n"] {
            let err = Config::parse(bad, &dir).expect_err(bad);
            assert!(err.contains("`project`"), "{err}");
        }
    }

    #[test]
    fn std_globals_extend_bundled() {
        let c = Config::parse("[std]\nglobals = [\"Knit\"]\n", Path::new(".")).unwrap();
        let s = c.build_std_names().unwrap();
        assert!(s.contains("Knit"));
        assert!(s.contains("print")); // bundled still active
    }

    #[test]
    fn std_files_replace_bundled() {
        let dir = std::env::temp_dir().join("rime-std-test");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mini.yml"), "globals:\n  OnlyThis:\n    property: read-only\n").unwrap();
        let c = Config::parse("[std]\nfiles = [\"mini.yml\"]\n", &dir).unwrap();
        let s = c.build_std_names().unwrap();
        assert!(s.contains("OnlyThis"));
        assert!(!s.contains("print")); // replaced, not extended
    }

    #[test]
    fn std_bad_types_error() {
        assert!(Config::parse("[std]\nfiles = \"not-a-list\"\n", Path::new(".")).is_err());
        assert!(Config::parse("[std]\nglobals = [1]\n", Path::new(".")).is_err());
    }

    #[test]
    fn std_non_table_value_errors() {
        assert!(Config::parse("std = \"roblox\"\n", Path::new(".")).is_err());
    }

    #[test]
    fn std_missing_file_errors_at_build() {
        let c = Config::parse("[std]\nfiles = [\"nope.yml\"]\n", Path::new(".")).unwrap();
        assert!(c.build_std_names().is_err());
    }

    #[test]
    fn rules_table_accepts_registered_custom_name() {
        let code = rime_diag::register_custom("config-tests-custom");
        let cfg = Config::parse("[rules]\nconfig-tests-custom = \"off\"\n", Path::new("."))
            .expect("custom name is a valid key once registered");
        let d = Diagnostic::error(code, "x").with_label(rime_diag::Label::primary(0..1, "here"));
        let (kept, dropped) = cfg.apply(vec![d]);
        assert!(kept.is_empty());
        assert_eq!(dropped, 1);
    }

    #[test]
    fn rules_table_still_rejects_unknown_names() {
        let err = Config::parse("[rules]\nnever-registered = \"off\"\n", Path::new("."))
            .expect_err("unknown key must fail");
        assert!(err.contains("unknown rule"), "{err}");
    }

    #[test]
    fn discover_path_walks_up_from_start() {
        let dir = std::env::temp_dir().join(format!("rime-cfg-discover-{}", std::process::id()));
        let nested = dir.join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(dir.join("rime.toml"), "").unwrap();
        assert_eq!(Config::discover_path(None, &nested), Some(dir.join("rime.toml")));
        assert_eq!(Config::discover_path(Some(Path::new("x/rime.toml")), &nested), Some(PathBuf::from("x/rime.toml")));
        std::fs::remove_dir_all(&dir).ok();
    }
}
