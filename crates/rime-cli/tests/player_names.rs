//! End-to-end: the names the player-map rules (R0802, R0803) read as "this
//! variable holds a Player" come from rime.toml's `[names] player`. The
//! default is `player` and `plr`; a project that writes `client` says so.

use std::path::PathBuf;
use std::process::Command;

/// A module table keyed by `client` and by `player`, never cleared.
const SOURCE: &str = "\
local byClient = {}
local byPlayer = {}

local function onJoin(client, player)
\tbyClient[client] = true
\tbyPlayer[player] = true
end

return onJoin
";

fn fixture(name: &str, toml: Option<&str>) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rime-player-names-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("case.luau"), SOURCE).unwrap();
    if let Some(toml) = toml {
        std::fs::write(dir.join("rime.toml"), toml).unwrap();
    }
    dir
}

/// The table each R0802 names, in source order, and stderr.
fn run(dir: &PathBuf) -> (Vec<String>, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_rime"))
        .args(["check", "--format", "json", "--only", "player-map-never-cleared", "case.luau"])
        .current_dir(dir)
        .output()
        .expect("spawn rime");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("stdout must be pure JSON ({e}); stderr: {stderr}"));
    let tables = v["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            assert_eq!(d["code"], "R0802");
            let message = d["message"].as_str().unwrap();
            message.split('`').nth(1).unwrap().to_string()
        })
        .collect();
    (tables, stderr)
}

#[test]
fn the_default_names_are_player_and_plr() {
    let dir = fixture("default", None);
    assert_eq!(run(&dir).0, ["byPlayer"]);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn rime_toml_replaces_the_names() {
    let dir = fixture("client", Some("[names]\nplayer = [\"client\"]\n"));
    assert_eq!(run(&dir).0, ["byClient"], "`player` is no longer on the list");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_star_matches_any_run_of_characters() {
    let dir = fixture("star", Some("[names]\nplayer = [\"pl*\", \"cli*\"]\n"));
    assert_eq!(run(&dir).0, ["byClient", "byPlayer"]);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_empty_list_leaves_only_annotations() {
    let dir = fixture("empty", Some("[names]\nplayer = []\n"));
    assert_eq!(run(&dir).0, Vec::<String>::new());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_bad_list_is_an_error_that_names_the_setting() {
    let dir = fixture("bad", Some("[names]\nplayer = \"client\"\n"));
    let out = Command::new(env!("CARGO_BIN_EXE_rime"))
        .args(["check", "case.luau"])
        .current_dir(&dir)
        .output()
        .expect("spawn rime");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("[names] player"), "{stderr}");
    std::fs::remove_dir_all(&dir).ok();
}
