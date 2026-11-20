//! Fidelity oracle: parse every corpus file with BOTH full-moon and real Luau,
//! and report disagreements.
//!
//! This is the decision-relevant measurement, not the speed one. Plan §2 makes
//! "we are a strict subset of Luau" the founding premise, so being unable to
//! parse valid Luau is an existential bug rather than a missing feature.
//!
//! Run this as a CI gate from day one and grammar drift shows up the day it
//! appears, which closes most of the fidelity gap that would otherwise argue
//! for implementing Rime in C++ against `Luau.Ast` directly.
//!
//! The category that matters is FM_FAIL_LUAU_OK: valid Luau that full-moon
//! rejects.
//!
//! Also checks the specific open question from plan §7.5 — whether
//! `{ read [string]: boolean }` (read/write modifiers on INDEXERS, as opposed
//! to named properties) parses in the current toolchain. If it does not,
//! `Readonly<T>` stays a shim alias.

use mlua::Lua;
use std::path::PathBuf;

#[derive(Default, Debug)]
struct Tally {
    both_ok: usize,
    both_fail: usize,
    fm_fail_luau_ok: Vec<(String, String)>,
    fm_ok_luau_fail: Vec<(String, String)>,
}

fn luau_parses(lua: &Lua, src: &str) -> Result<(), String> {
    // NOTE: this is parse + compile, so a bytecode-generation error would also
    // land here. Good enough as a syntax oracle; not a parse-only timing.
    lua.load(src)
        .set_name("oracle")
        .into_function()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::args().nth(1).unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../corpus")
            .to_string_lossy()
            .into_owned()
    });

    let lua = Lua::new();
    let mut tally = Tally::default();

    for entry in walkdir::WalkDir::new(&dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|x| x == "luau" || x == "lua")
        })
    {
        let path = entry.path().to_string_lossy().into_owned();
        let Ok(src) = std::fs::read_to_string(entry.path()) else {
            continue;
        };

        let fm = full_moon::parse(&src).is_ok();
        let lu = luau_parses(&lua, &src);

        match (fm, lu.is_ok()) {
            (true, true) => tally.both_ok += 1,
            (false, false) => tally.both_fail += 1,
            (false, true) => {
                let err = full_moon::parse(&src)
                    .err()
                    .map(|e| format!("{e:?}"))
                    .unwrap_or_default();
                tally.fm_fail_luau_ok.push((path, err));
            }
            (true, false) => {
                tally.fm_ok_luau_fail.push((path, lu.unwrap_err()));
            }
        }
    }

    println!("corpus: {dir}");
    println!("  both parse ok   : {}", tally.both_ok);
    println!("  both reject     : {}", tally.both_fail);
    println!("  FM ok, Luau fail: {}", tally.fm_ok_luau_fail.len());
    println!("  FM FAIL, Luau ok: {}   <-- the number that matters", tally.fm_fail_luau_ok.len());

    for (path, err) in tally.fm_fail_luau_ok.iter().take(25) {
        println!("\n  {path}\n    {}", err.lines().next().unwrap_or(""));
    }

    // The reverse direction is not existential but still wrong: a file Rime
    // would happily check that real Luau refuses to load.
    for (path, err) in tally.fm_ok_luau_fail.iter().take(25) {
        println!("\n  [fm ok, luau fail] {path}\n    {}", err.lines().next().unwrap_or(""));
    }

    println!("\n--- targeted syntax probes (plan §7.5) ---");
    let probes: &[(&str, &str)] = &[
        ("read prop", "type T = { read x: number }\nreturn nil"),
        ("write prop", "type T = { write x: number }\nreturn nil"),
        ("read indexer", "type T = { read [string]: boolean }\nreturn nil"),
        ("plain indexer", "type T = { [string]: boolean }\nreturn nil"),
        ("generic alias", "type Frozen<T> = T\nreturn nil"),
        ("type fn", "type function f(t) return t end\nreturn nil"),
        ("singleton union", "type Tag = \"hit\" | \"miss\"\nreturn nil"),
        ("variadic type", "type F = (...number) -> ...string\nreturn nil"),
    ];
    for (name, src) in probes {
        let fm = full_moon::parse(src).is_ok();
        let lu = luau_parses(&lua, src).is_ok();
        let verdict = match (fm, lu) {
            (true, true) => "both ok",
            (false, true) => "FULL-MOON GAP",
            (true, false) => "fm only",
            (false, false) => "neither",
        };
        println!("  {name:<18} fm={fm:<5} luau={lu:<5} {verdict}");
    }

    if !tally.fm_fail_luau_ok.is_empty() {
        std::process::exit(1);
    }
    Ok(())
}
