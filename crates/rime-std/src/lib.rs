//! Selene standard-library files (YAML format v2), reduced to the one fact
//! the globals rules need: WHICH global names exist. Signatures, arg types,
//! structs, and per-field correctness are not read yet and will grow here.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::OnceLock;

/// The set of root global names defined by one or more selene std-lib YAML
/// documents (e.g. `print`, `math`, `workspace` — dotted/wildcarded keys are
/// reduced to their leading segment; see [`StdNames::parse_str`]).
#[derive(Debug, Clone)]
pub struct StdNames {
    names: HashSet<String>,
}

impl StdNames {
    /// Whether `name` is a known root global.
    pub fn contains(&self, name: &str) -> bool {
        self.names.contains(name)
    }

    /// Parse one selene std-lib YAML document, recursively resolving `base:`
    /// through `base_lookup` (which returns the named base's YAML text, or
    /// `None` if the base is unknown).
    pub fn parse_str(
        yaml: &str,
        base_lookup: &dyn Fn(&str) -> Option<&'static str>,
    ) -> Result<StdNames, String> {
        let mut names = HashSet::new();
        let mut seen_bases = HashSet::new();
        let owned_lookup = |name: &str| base_lookup(name).map(str::to_string);
        collect(yaml, &owned_lookup, &mut names, &mut seen_bases)?;
        Ok(StdNames { names })
    }

    /// Parse and union the named std-lib files. This REPLACES the bundled
    /// set by construction (callers who also want the bundled names must
    /// list bundled files explicitly, or merge with [`StdNames::bundled`]).
    ///
    /// Each file's `base:` resolves first against builtin names (`lua51`,
    /// `luau`, `roblox`), then against a sibling `<base>.yml` next to the
    /// file. An unknown base or a base cycle is an `Err`.
    pub fn from_files(paths: &[PathBuf]) -> Result<StdNames, String> {
        let mut names = HashSet::new();
        for path in paths {
            let yaml = std::fs::read_to_string(path)
                .map_err(|e| format!("failed to read std-lib file `{}`: {e}", path.display()))?;
            let dir = path.parent().map(PathBuf::from);
            let sibling_lookup = |base: &str| -> Option<String> {
                if let Some(text) = builtin(base) {
                    return Some(text.to_string());
                }
                let sibling = dir.as_ref()?.join(format!("{base}.yml"));
                std::fs::read_to_string(sibling).ok()
            };
            let mut seen_bases = HashSet::new();
            collect(&yaml, &sibling_lookup, &mut names, &mut seen_bases)?;
        }
        Ok(StdNames { names })
    }

    /// Add extra global names (e.g. from a project's own `selene.toml`
    /// `globals` list) on top of whatever this set already contains.
    pub fn add_globals<I: IntoIterator<Item = String>>(&mut self, names: I) {
        self.names.extend(names);
    }

    /// The std-lib names vendored with the crate: Luau + Roblox, resolved
    /// through `roblox.yml`'s `base: luau` chain (selene's generator also
    /// embeds the fully-resolved set directly in `roblox.yml`, but walking
    /// the chain is harmless and keeps this in step with `from_files`).
    ///
    /// `_G` is a `globals:` key in vendored `lua51.yml` (selene does not
    /// special-case it away here), so no separate handling is needed.
    pub fn bundled() -> &'static StdNames {
        &bundled().names
    }
}

/// Where a bundled function comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdSource {
    /// `lua51.yml` or `luau.yml`: the language's own library.
    Luau,
    /// `roblox.yml` alone: what Roblox adds.
    Roblox,
}

/// One function the bundled std files define, by the name it is called
/// through: `tostring`, `math.floor`, `CFrame.new`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StdFunction {
    pub name: String,
    pub source: StdSource,
}

/// Every function in the bundled std files, sorted by name. A function is
/// a `globals:` entry with `args:`; one a later file marks `removed: true`
/// (`os.execute`, which Luau does not have) is not listed. `roblox.yml`
/// repeats the Lua 5.1 and Luau libraries, removed entries included, so a
/// function is `Roblox` only when neither Luau file has it, and a removed
/// one stays removed.
pub fn bundled_functions() -> &'static [StdFunction] {
    &bundled().functions
}

struct Bundled {
    names: StdNames,
    functions: Vec<StdFunction>,
}

/// The three vendored files, each parsed once, behind both
/// `StdNames::bundled` and `bundled_functions`.
fn bundled() -> &'static Bundled {
    static BUNDLED: OnceLock<Bundled> = OnceLock::new();
    BUNDLED.get_or_init(|| {
        let mut names = HashSet::new();
        // name -> (still a function, where it was first seen)
        let mut functions: std::collections::BTreeMap<String, (bool, StdSource)> = Default::default();
        // Base first, so a later file's `removed: true` lands on an entry.
        for (file, source) in
            [("lua51", StdSource::Luau), ("luau", StdSource::Luau), ("roblox", StdSource::Roblox)]
        {
            let doc: serde_yaml::Value = serde_yaml::from_str(builtin(file).unwrap())
                .expect("vendored std files must parse");
            let Some(globals) = doc.get("globals").and_then(|v| v.as_mapping()) else { continue };
            for (key, value) in globals {
                let Some(key) = key.as_str() else { continue };
                let root = key.split('.').next().unwrap_or(key);
                if root != "*" && !root.is_empty() {
                    names.insert(root.to_string());
                }
                let removed = value.get("removed").and_then(|v| v.as_bool()) == Some(true);
                if removed {
                    // `io` removed takes `io.open` with it.
                    let below = format!("{key}.");
                    for (name, entry) in functions.iter_mut() {
                        if name == key || name.starts_with(&below) {
                            entry.0 = false;
                        }
                    }
                } else if value.get("args").is_some() {
                    functions.entry(key.to_string()).or_insert((true, source));
                }
            }
        }
        let functions = functions
            .into_iter()
            .filter(|(_, (live, _))| *live)
            .map(|(name, (_, source))| StdFunction { name, source })
            .collect();
        Bundled { names: StdNames { names }, functions }
    })
}

/// Bundled std-lib YAML text for a builtin base name (`lua51`, `luau`,
/// `roblox`), vendored under `std/` and baked into the binary.
pub(crate) fn builtin(name: &str) -> Option<&'static str> {
    match name {
        "lua51" => Some(include_str!("../std/lua51.yml")),
        "luau" => Some(include_str!("../std/luau.yml")),
        "roblox" => Some(include_str!("../std/roblox.yml")),
        _ => None,
    }
}

/// Parse `yaml`, resolving its `base:` chain (each hop's cycle-checked
/// against `seen_bases`) and inserting every root global name into `names`.
fn collect(
    yaml: &str,
    base_lookup: &dyn Fn(&str) -> Option<String>,
    names: &mut HashSet<String>,
    seen_bases: &mut HashSet<String>,
) -> Result<(), String> {
    let doc: serde_yaml::Value =
        serde_yaml::from_str(yaml).map_err(|e| format!("invalid std-lib YAML: {e}"))?;
    if let Some(base) = doc.get("base").and_then(|v| v.as_str()) {
        if !seen_bases.insert(base.to_string()) {
            return Err(format!("std-lib base cycle through `{base}`"));
        }
        let text = base_lookup(base).ok_or_else(|| format!("unknown std-lib base `{base}`"))?;
        collect(&text, base_lookup, names, seen_bases)?;
    }
    if let Some(globals) = doc.get("globals").and_then(|v| v.as_mapping()) {
        for (key, _) in globals {
            let Some(key) = key.as_str() else { continue };
            let root = key.split('.').next().unwrap_or(key);
            if root != "*" && !root.is_empty() {
                names.insert(root.to_string());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_base(_: &str) -> Option<&'static str> {
        None
    }

    #[test]
    fn root_names_from_plain_and_dotted_keys() {
        let s = StdNames::parse_str(
            "globals:\n  print:\n    args:\n      - type: '...'\n  math.floor:\n    args:\n      - type: number\n  workspace.*:\n    property: read-only\n",
            &no_base,
        )
        .unwrap();
        assert!(s.contains("print"));
        assert!(s.contains("math"));
        assert!(s.contains("workspace"));
        assert!(!s.contains("floor"));
    }

    #[test]
    fn base_chain_resolves() {
        let lookup = |name: &str| (name == "lua51").then_some("globals:\n  print:\n    args: []\n");
        let s = StdNames::parse_str("base: lua51\nglobals:\n  game:\n    property: read-only\n", &lookup).unwrap();
        assert!(s.contains("print"));
        assert!(s.contains("game"));
    }

    #[test]
    fn unknown_base_is_an_error() {
        let err = StdNames::parse_str("base: nope\nglobals: {}\n", &no_base).unwrap_err();
        assert!(err.contains("nope"), "{err}");
    }

    #[test]
    fn base_cycle_is_an_error() {
        // a file whose base lookup returns a document that bases on itself
        let lookup = |name: &str| (name == "a").then_some("base: a\nglobals: {}\n");
        assert!(StdNames::parse_str("base: a\nglobals: {}\n", &lookup).is_err());
    }

    #[test]
    fn add_globals_extends() {
        let mut s = StdNames::parse_str("globals: {}\n", &no_base).unwrap();
        assert!(!s.contains("Knit"));
        s.add_globals(["Knit".to_string()]);
        assert!(s.contains("Knit"));
    }

    #[test]
    fn missing_globals_key_is_fine() {
        // a pure-base file is legal
        assert!(StdNames::parse_str("base: lua51\n", &|n| (n == "lua51").then_some("globals: {}\n")).is_ok());
    }

    #[test]
    fn from_files_resolves_sibling_base() {
        let dir = std::env::temp_dir().join(format!("rime-std-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let main_path = dir.join("main.yml");
        let sibling_path = dir.join("sibling.yml");
        std::fs::write(&main_path, "base: sibling\nglobals:\n  Extra:\n    property: read-only\n").unwrap();
        std::fs::write(&sibling_path, "globals:\n  SiblingGlobal:\n    property: read-only\n").unwrap();

        let s = StdNames::from_files(&[main_path]).unwrap();
        assert!(s.contains("Extra"));
        assert!(s.contains("SiblingGlobal"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn from_files_unknown_base_is_an_error() {
        let dir = std::env::temp_dir().join(format!("rime-std-test2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let main_path = dir.join("main.yml");
        std::fs::write(&main_path, "base: nope\nglobals: {}\n").unwrap();

        let err = StdNames::from_files(&[main_path]).unwrap_err();
        assert!(err.contains("nope"), "{err}");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn bundled_has_luau_and_roblox_names() {
        let s = StdNames::bundled();
        for name in [
            "print", "pcall", "math", "string", "typeof", "buffer", "task", "game", "workspace",
            "script", "Enum", "Vector3", "CFrame", "shared", "_G",
        ] {
            assert!(s.contains(name), "missing {name}");
        }
        assert!(!s.contains("definitely_not_a_global"));
    }

    #[test]
    fn bundled_names_are_what_walking_the_base_chain_gives() {
        let walked = StdNames::parse_str(builtin("roblox").unwrap(), &builtin).unwrap();
        assert_eq!(StdNames::bundled().names, walked.names);
    }

    fn source_of(name: &str) -> Option<StdSource> {
        bundled_functions().iter().find(|f| f.name == name).map(|f| f.source)
    }

    #[test]
    fn bundled_functions_say_where_each_comes_from() {
        for name in ["tostring", "math.floor", "table.freeze", "buffer.readu8", "typeof", "vector.create"] {
            assert_eq!(source_of(name), Some(StdSource::Luau), "{name}");
        }
        for name in ["CFrame.new", "Color3.fromRGB", "Instance.new", "task.spawn", "utf8.graphemes"] {
            assert_eq!(source_of(name), Some(StdSource::Roblox), "{name}");
        }
    }

    #[test]
    fn bundled_functions_leave_out_what_is_not_a_function_or_was_removed() {
        // Properties and structs.
        for name in ["game", "workspace", "CFrame.identity", "math.pi", "script"] {
            assert_eq!(source_of(name), None, "{name}");
        }
        // Lua 5.1 functions Luau does not have, alone and by library.
        for name in ["os.execute", "dofile", "io.open", "package.loadlib", "string.dump"] {
            assert_eq!(source_of(name), None, "{name}");
        }
        let names: Vec<&str> = bundled_functions().iter().map(|f| f.name.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }
}
