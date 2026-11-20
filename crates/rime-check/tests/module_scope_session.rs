//! R0312 through a `Session`: the rule settings a session is built with are
//! the ones every recheck uses, with and without a project, and
//! `set_options` + `reset` is how a changed rime.toml reaches a warm one
//! (the language server's path).

use rime_check::project::Resolver;
use rime_check::rules::module_scope;
use rime_check::session::Session;
use rime_check::RuleOptions;
use std::path::PathBuf;

#[path = "common/sibling.rs"]
mod sibling;
use sibling::SiblingResolver;

const MODULE: &str = "local Board = {}\nBoard.Stamp = os.time()\nreturn Board\n";

fn files() -> Vec<(PathBuf, String)> {
    vec![
        (PathBuf::from("case/Board.luau"), MODULE.to_string()),
        (PathBuf::from("case/start.server.luau"), "print(os.time())\n".to_string()),
    ]
}

fn on() -> RuleOptions {
    RuleOptions { module_scope: Some(module_scope::Options::default()), ..Default::default() }
}

fn std() -> rime_std::StdNames {
    rime_std::StdNames::bundled().clone()
}

/// (file name, message) of every R0312 the session holds.
fn found(session: &Session<'_>) -> Vec<(String, String)> {
    session
        .reports()
        .into_iter()
        .flat_map(|r| {
            let name = r.path.file_name().unwrap().to_string_lossy().into_owned();
            r.diagnostics
                .into_iter()
                .filter(|d| d.code == rime_diag::Code(312))
                .map(move |d| (name.clone(), d.message))
        })
        .collect()
}

fn expected() -> Vec<(String, String)> {
    vec![("Board.luau".to_string(), "`os.time` is called at module scope".to_string())]
}

fn resolvers() -> [Option<Box<dyn Resolver>>; 2] {
    [None, Some(Box::new(SiblingResolver))]
}

#[test]
fn a_plain_session_does_not_run_the_rule() {
    for resolver in resolvers() {
        let session = Session::new(files(), resolver, std(), None);
        assert_eq!(found(&session), vec![]);
    }
}

#[test]
fn a_session_built_with_the_rule_on_reports_it() {
    for resolver in resolvers() {
        let session = Session::with_options(files(), resolver, std(), on(), None);
        assert_eq!(found(&session), expected());
    }
}

#[test]
fn an_edit_is_rechecked_under_the_same_settings() {
    for resolver in resolvers() {
        let mut session = Session::with_options(files(), resolver, std(), on(), None);
        let path = PathBuf::from("case/Board.luau");
        session.update(&path, "local Board = {}\nBoard.Stamp = os.clock()\nreturn Board\n".to_string());
        assert_eq!(
            found(&session),
            vec![("Board.luau".to_string(), "`os.clock` is called at module scope".to_string())]
        );
    }
}

#[test]
fn reset_keeps_the_settings_and_set_options_changes_them() {
    for with_project in [false, true] {
        let resolver = || -> Option<Box<dyn Resolver>> {
            with_project.then(|| Box::new(SiblingResolver) as Box<dyn Resolver>)
        };
        let mut session = Session::with_options(files(), resolver(), std(), on(), None);

        session.reset(files(), resolver(), std());
        assert_eq!(found(&session), expected(), "reset carries the settings over");

        // rime.toml now allows the call: unchanged text, different answer.
        let mut allowing = module_scope::Options::default();
        allowing.allow.push("os.*".to_string());
        session.set_options(RuleOptions { module_scope: Some(allowing), ..Default::default() });
        let changed = session.reset(files(), resolver(), std());
        assert_eq!(found(&session), vec![]);
        assert!(
            changed.iter().any(|p| p.ends_with("Board.luau")),
            "the file whose report went away is in the changed set: {changed:?}"
        );

        // And off again.
        session.set_options(RuleOptions::default());
        session.reset(files(), resolver(), std());
        assert_eq!(found(&session), vec![]);

        session.set_options(on());
        let changed = session.reset(files(), resolver(), std());
        assert_eq!(found(&session), expected());
        assert!(changed.iter().any(|p| p.ends_with("Board.luau")), "{changed:?}");
    }
}
