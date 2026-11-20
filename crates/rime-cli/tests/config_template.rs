//! `rime config-template` prints a rime.toml that lists every setting Rime
//! reads with its current default; `examples/rime.toml` is that output
//! checked in, and the drift test below fails until it is regenerated.

use std::process::Command;

fn template() -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_rime"))
        .args(["config-template"])
        .output()
        .expect("spawn rime");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).expect("template is UTF-8")
}

/// Lines under `[rules]` of the form `# name = "error"`, uncommented.
fn rule_lines(text: &str) -> Vec<String> {
    let mut in_rules = false;
    let mut out = Vec::new();
    for line in text.lines() {
        if line.trim() == "[rules]" {
            in_rules = true;
            continue;
        }
        if !in_rules {
            continue;
        }
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("# ") {
            let key_is_name = rest
                .split(' ')
                .next()
                .is_some_and(|k| !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
            if rest.contains(" = \"") && key_is_name {
                out.push(rest.to_string());
            }
        }
    }
    out
}

#[test]
fn template_lists_every_rule_with_a_default_severity() {
    let text = template();
    let lines = rule_lines(&text);
    for (name, code) in rime_diag::RULE_NAMES {
        let line = lines
            .iter()
            .find(|l| l.starts_with(&format!("{name} = ")))
            .unwrap_or_else(|| panic!("no `# {name} = ...` line under [rules]"));
        let value = line.split(" = ").nth(1).unwrap().trim();
        if rime_diag::off_by_default(rime_diag::Code(*code)) {
            assert_eq!(value, "\"off\"", "{name} is off until a level is set, and the template must say so");
        } else {
            assert!(value == "\"error\"" || value == "\"warn\"", "{name}: default must be error or warn, got {value}");
        }
        assert!(text.contains(&format!("R{code:04} ")), "R{code:04} is not named in the template");
    }
    assert_eq!(lines.len(), rime_diag::RULE_NAMES.len(), "exactly one line per rule");
}

#[test]
fn template_lists_the_options_of_the_rule_that_has_them() {
    let text = template();
    for needle in [
        "#     [rules.module-scope-computation]",
        "#     level = \"error\"",
        "#     allow-builtin = [\"luau\", \"roblox-datatypes\"]",
        "#     allow = []",
        "#     deny = []",
        "#     allow-loops = false",
        "#     scripts = [",
        "#         \"*.server.lua\", \"*.server.luau\",",
        "#         \"*.client.lua\", \"*.client.luau\",",
        "#         \"*.plugin.lua\", \"*.plugin.luau\",",
        "#     exclude = []",
        "`require` and `game:GetService` are allowed without being listed",
    ] {
        assert!(text.contains(needle), "template lacks {needle}");
    }
    for gone in ["legacy", "*.local.", "math.random"] {
        assert!(!text.contains(gone), "template still has {gone}");
    }
}

#[test]
fn the_options_block_is_a_rime_toml_rime_accepts() {
    // The commented block, uncommented, is the table form a project copies.
    let text = template();
    let block: String = text
        .lines()
        .skip_while(|l| l.trim() != "#     [rules.module-scope-computation]")
        .take_while(|l| !l.starts_with("# module-scope-computation = "))
        .filter(|l| l.starts_with("#     "))
        .map(|l| format!("{}\n", l.trim_start_matches("#     ")))
        .collect();
    assert!(block.contains("scripts = ["), "block not found:\n{block}");
    for key in ["level", "allow-builtin", "allow", "deny", "allow-loops", "scripts", "exclude"] {
        assert!(block.lines().any(|l| l.starts_with(&format!("{key} = "))), "no `{key}` in:\n{block}");
    }
    let dir = std::env::temp_dir().join(format!("rime-config-template-options-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("rime.toml"), block).unwrap();
    std::fs::write(dir.join("case.luau"), "local started = os.time()\nreturn started\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_rime"))
        .args(["check", "--format", "json", "case.luau"])
        .current_dir(&dir)
        .output()
        .expect("spawn rime");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("R0312"), "stdout: {stdout}\nstderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(out.status.code(), Some(1), "level = \"error\" in the block fails the run");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn template_documents_the_non_rule_settings() {
    let text = template();
    for needle in [
        "# ignore = [",
        "# [requires]",
        "# trust = [",
        "# [std]",
        "# files = [",
        "# globals = [",
        "rime-rules/",
        "# [names]",
        "# player = [\"player\", \"plr\"]",
        "# [conservative]",
        "# default = \"warn\"",
        "conservative = \"off\" }",
    ] {
        assert!(text.contains(needle), "template lacks {needle}");
    }
}

#[test]
fn checked_in_example_matches_the_generator() {
    let checked_in = include_str!("../../../examples/rime.toml").replace("\r\n", "\n");
    let generated = template().replace("\r\n", "\n");
    assert_eq!(
        checked_in, generated,
        "examples/rime.toml is stale — run `cargo run -p rime-cli -- config-template --out examples/rime.toml` and commit"
    );
}

#[test]
fn every_rule_line_is_a_key_rime_accepts() {
    let text = template();
    let dir = std::env::temp_dir().join(format!("rime-config-template-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut toml = String::from("[rules]\n");
    for line in rule_lines(&text) {
        toml.push_str(&line);
        toml.push('\n');
    }
    std::fs::write(dir.join("rime.toml"), toml).unwrap();
    std::fs::write(dir.join("case.luau"), "local x = 1\nprint(x)\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_rime"))
        .args(["check", "--format", "json", "case.luau"])
        .current_dir(&dir)
        .output()
        .expect("spawn rime");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("unknown rule"), "{stderr}");
    assert!(out.status.code() == Some(0) || out.status.code() == Some(1), "{stderr}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn out_flag_writes_the_same_text_to_a_file() {
    let dir = std::env::temp_dir().join(format!("rime-config-template-out-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("rime.toml");
    let out = Command::new(env!("CARGO_BIN_EXE_rime"))
        .args(["config-template", "--out", path.to_str().unwrap()])
        .output()
        .expect("spawn rime");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(out.stdout.is_empty(), "--out must not also print to stdout");
    assert_eq!(std::fs::read_to_string(&path).unwrap().replace("\r\n", "\n"), template().replace("\r\n", "\n"));
    std::fs::remove_dir_all(&dir).ok();
}
