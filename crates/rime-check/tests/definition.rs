//! `Session::definition`: what Ctrl+click lands on. Two
//! files on disk (the corpus SiblingResolver resolves `script.Parent.X` to
//! `<dir>/X.luau` and checks existence), one Session, offsets computed
//! from the fixture text so the cases read as source, not numbers.

#[path = "common/sibling.rs"]
mod sibling;
use sibling::SiblingResolver;

use rime_check::session::Session;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

const DEP: &str = "\
local Dep = {}

Dep.CONST = 1

Dep.Limits = {
	Retries = 3,
	Timeout = 30,
}

function Dep.foo(a: number): number
	return a
end

function Dep:bar(): number
	return 2
end

return Dep
";

/// A module whose exported table re-exports another module's instance —
/// the shape only the require-target evaluator can follow (cheap `Resolver::resolve`
/// answers `Dynamic` for a target rooted at a `require(...)` call).
const HUB: &str = "\
local Hub = {}

Hub.Sub = script.Parent.dep

return Hub
";

const MAIN: &str = "\
local Dep = require(script.Parent.dep)
local Deep = require(require(script.Parent.hub).Sub)
print(Deep)

local x = 1
print(x)

local function fact(n: number): number
	if n <= 1 then
		return 1
	end
	return n * fact(n - 1)
end

local function outer()
	local y = x
	return function()
		return y + fact(2)
	end
end

do
	local x = 2
	print(x)
end

local z = x
local z = z

function helper()
	return 0
end

for i = 1, 3 do
	print(i)
end

local function shadow()
	local Dep = {}
	return Dep.foo
end

local M = {}

M.Config = {
	Enabled = true,
}

function M.go(): number
	local a = Dep.Limits.Retries
	local b = M.Config.Enabled
	return Dep.foo(x) + Dep:bar() + Dep.CONST + helper()
end

M.go()
outer()
return M
";

struct Fixture {
    dir: PathBuf,
    main: PathBuf,
    dep: PathBuf,
    session: Session<'static>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fixture() -> Fixture {
    // A folder of its own for every call. The tests of this file run in
    // parallel in one process, and the resolver looks files up on disk, so a
    // folder named after the process alone is emptied by one test while
    // another is using it.
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("rime-definition-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("main.luau"), MAIN).unwrap();
    std::fs::write(dir.join("dep.luau"), DEP).unwrap();
    std::fs::write(dir.join("hub.luau"), HUB).unwrap();
    let dir = dir.canonicalize().unwrap();
    let main = dir.join("main.luau");
    let dep = dir.join("dep.luau");
    let files = vec![
        (main.clone(), MAIN.to_string()),
        (dep.clone(), DEP.to_string()),
        (dir.join("hub.luau"), HUB.to_string()),
    ];
    let session =
        Session::new(files, Some(Box::new(SiblingResolver)), rime_std::StdNames::bundled().clone(), None);
    Fixture { dir, main, dep, session }
}

/// Byte offset of the `nth` occurrence of `needle` in `src`, plus `plus`.
fn at(src: &str, needle: &str, nth: usize, plus: usize) -> usize {
    let mut from = 0;
    for _ in 0..nth {
        from = src[from..].find(needle).map(|i| from + i + needle.len()).expect("occurrence");
    }
    src[from..].find(needle).map(|i| from + i + plus).expect("occurrence")
}

/// The span of the `nth` occurrence of `needle`.
fn span_of(src: &str, needle: &str, nth: usize) -> std::ops::Range<usize> {
    let s = at(src, needle, nth, 0);
    s..s + needle.len()
}

#[test]
fn require_argument_jumps_to_the_file() {
    let f = fixture();
    // Anywhere inside the argument, including on `Parent`.
    let off = at(MAIN, "script.Parent.dep", 0, 8);
    let d = f.session.definition(&f.main, off).expect("a definition");
    assert_eq!(d.path, f.dep);
    assert_eq!(d.span, 0..0);
}

/// Go-to-definition and the checker must agree about the SAME require: a
/// target the checker resolves only through the require-target evaluator has to be
/// Ctrl+clickable too, or the editor silently refuses to navigate a site
/// it reports no diagnostic for.
#[test]
fn require_through_an_exported_value_jumps_to_the_file() {
    let f = fixture();
    // `require(require(script.Parent.hub).Sub)` — the outer require's
    // argument is rooted at a call, so cheap resolution gives up.
    let off = at(MAIN, "require(script.Parent.hub).Sub", 0, 27);
    let d = f.session.definition(&f.main, off).expect("a definition");
    assert_eq!(d.path, f.dep);
    assert_eq!(d.span, 0..0);
}

#[test]
fn members_of_a_required_module_jump_to_their_declaration() {
    let f = fixture();
    // `Dep.foo(x)` → the `foo` in `function Dep.foo(`.
    let d = f.session.definition(&f.main, at(MAIN, "Dep.foo(x)", 0, 4)).expect("foo");
    assert_eq!((d.path.as_path(), d.span), (f.dep.as_path(), span_of(DEP, "foo", 0)));
    // `Dep:bar()` — the method name has no HIR span of its own; the click
    // lands in the call but outside the base and the arguments.
    let d = f.session.definition(&f.main, at(MAIN, "Dep:bar()", 0, 4)).expect("bar");
    assert_eq!((d.path.as_path(), d.span), (f.dep.as_path(), span_of(DEP, "bar", 0)));
    // `Dep.CONST` → the `CONST` in `Dep.CONST = 1`.
    let d = f.session.definition(&f.main, at(MAIN, "Dep.CONST", 0, 4)).expect("CONST");
    assert_eq!(d.path, f.dep);
    assert_eq!(d.span.start, at(DEP, "CONST", 0, 0));
}

#[test]
fn the_require_binding_itself_is_a_local() {
    let f = fixture();
    // The `Dep` base of `Dep.foo` is a plain local read.
    let d = f.session.definition(&f.main, at(MAIN, "Dep.foo(x)", 0, 0)).expect("Dep");
    assert_eq!((d.path.as_path(), d.span), (f.main.as_path(), span_of(MAIN, "Dep", 0)));
}

#[test]
fn a_shadowed_require_binding_is_not_the_module() {
    let f = fixture();
    // Inside `shadow`, `Dep` is a fresh table: `Dep.foo` there must not
    // resolve through the top-level require.
    let off = at(MAIN, "return Dep.foo\n", 0, 11);
    assert!(f.session.definition(&f.main, off).is_none());
}

#[test]
fn own_export_members_jump_within_the_file() {
    let f = fixture();
    let d = f.session.definition(&f.main, at(MAIN, "M.go()\n", 0, 2)).expect("go");
    assert_eq!((d.path.as_path(), d.span), (f.main.as_path(), span_of(MAIN, "go", 0)));
}

#[test]
fn nested_member_paths_jump_to_the_leaf_key() {
    let f = fixture();
    // `Dep.Limits.Retries` — two levels into a required module: the
    // leaf key `Retries` inside the `Dep.Limits` table literal in
    // dep.luau. (The reported gap: only `.Limits` resolved before.)
    let d = f.session.definition(&f.main, at(MAIN, "Dep.Limits.Retries", 0, 11)).expect("nested");
    assert_eq!(d.path, f.dep);
    assert_eq!(d.span, span_of(DEP, "Retries", 0));
    // The intermediate `.Limits` still resolves to the member write.
    let d = f.session.definition(&f.main, at(MAIN, "Dep.Limits.Retries", 0, 4)).expect("Limits");
    assert_eq!(d.path, f.dep);
    assert_eq!(d.span.start, at(DEP, "Limits = {", 0, 0));
    // Own-export nested path: `M.Config.Enabled` in this file.
    let d = f.session.definition(&f.main, at(MAIN, "M.Config.Enabled", 0, 9)).expect("Enabled");
    assert_eq!((d.path.as_path(), d.span), (f.main.as_path(), span_of(MAIN, "Enabled", 0)));
}

#[test]
fn locals_resolve_by_scope() {
    let f = fixture();
    let x_decl = span_of(MAIN, "x", 0); // `local x = 1`
    // First `print(x)`: the top-level `x`.
    let d = f.session.definition(&f.main, at(MAIN, "print(x)", 0, 6)).unwrap();
    assert_eq!(d.span, x_decl);
    // Second `print(x)`, inside `do`: the inner `local x = 2`.
    let d = f.session.definition(&f.main, at(MAIN, "print(x)", 1, 6)).unwrap();
    assert_eq!(d.span, span_of(MAIN, "local x = 2", 0).start + 6..span_of(MAIN, "local x = 2", 0).start + 7);
    // `local z = z`: the initializer reads the PREVIOUS `z`.
    let d = f.session.definition(&f.main, at(MAIN, "local z = z", 0, 10)).unwrap();
    assert_eq!(d.span, span_of(MAIN, "local z = x", 0).start + 6..span_of(MAIN, "local z = x", 0).start + 7);
    // Loop variable.
    let d = f.session.definition(&f.main, at(MAIN, "print(i)", 0, 6)).unwrap();
    assert_eq!(d.span, span_of(MAIN, "for i = 1", 0).start + 4..span_of(MAIN, "for i = 1", 0).start + 5);
}

#[test]
fn params_recursion_and_upvalues() {
    let f = fixture();
    // `n * fact(n - 1)`: `n` is the parameter, `fact` the local function
    // (bound before its body, so recursion resolves).
    let d = f.session.definition(&f.main, at(MAIN, "return n * fact(n - 1)", 0, 7)).unwrap();
    assert_eq!(d.span, span_of(MAIN, "fact(n: number)", 0).start + 5..span_of(MAIN, "fact(n: number)", 0).start + 6);
    let d = f.session.definition(&f.main, at(MAIN, "return n * fact(n - 1)", 0, 11)).unwrap();
    assert_eq!(d.span, span_of(MAIN, "fact", 0));
    // Inside the closure returned by `outer`: `y` is an upvalue one body
    // out, `fact` two bodies out.
    let d = f.session.definition(&f.main, at(MAIN, "return y + fact(2)", 0, 7)).unwrap();
    assert_eq!(d.span, span_of(MAIN, "local y = x", 0).start + 6..span_of(MAIN, "local y = x", 0).start + 7);
    let d = f.session.definition(&f.main, at(MAIN, "return y + fact(2)", 0, 11)).unwrap();
    assert_eq!(d.span, span_of(MAIN, "fact", 0));
    // `outer()` at the top level.
    let d = f.session.definition(&f.main, at(MAIN, "outer()", 1, 0)).unwrap();
    assert_eq!(d.span, span_of(MAIN, "outer", 0));
}

#[test]
fn global_functions_jump_to_their_statement() {
    let f = fixture();
    // `helper()` inside `M.go`: a global `function helper()` — the jump
    // lands on its declaration statement, not on a lexical binding.
    let d = f.session.definition(&f.main, at(MAIN, "helper()", 1, 0)).unwrap();
    assert_eq!(d.path, f.main);
    assert_eq!(d.span.start, at(MAIN, "function helper", 0, 0));
}

#[test]
fn clicking_a_declaration_resolves_to_itself() {
    let f = fixture();
    // A `local` name.
    let d = f.session.definition(&f.main, at(MAIN, "local x = 1", 0, 6)).expect("local x");
    assert_eq!((d.path.as_path(), d.span), (f.main.as_path(), span_of(MAIN, "x", 0)));
    // A parameter in a header.
    let d = f.session.definition(&f.main, at(MAIN, "fact(n: number)", 0, 5)).expect("param n");
    assert_eq!(d.span, span_of(MAIN, "fact(n: number)", 0).start + 5..span_of(MAIN, "fact(n: number)", 0).start + 6);
    // A `local function` name at its own declaration.
    let d = f.session.definition(&f.main, at(MAIN, "local function fact", 0, 15)).expect("fact decl");
    assert_eq!(d.span, span_of(MAIN, "fact", 0));
    // A member-function name at its declaration (`function M.go`).
    let d = f.session.definition(&f.main, at(MAIN, "function M.go", 0, 11)).expect("go decl");
    assert_eq!(d.span, span_of(MAIN, "go", 0));
    // A loop variable at its declaration.
    let d = f.session.definition(&f.main, at(MAIN, "for i = 1", 0, 4)).expect("loop i");
    assert_eq!(d.span, span_of(MAIN, "for i = 1", 0).start + 4..span_of(MAIN, "for i = 1", 0).start + 5);
}

#[test]
fn builtins_and_unknown_names_have_no_definition() {
    let f = fixture();
    assert!(f.session.definition(&f.main, at(MAIN, "print(x)", 0, 0)).is_none());
    // Whitespace between statements.
    assert!(f.session.definition(&f.main, at(MAIN, "\n\nlocal x = 1", 0, 1)).is_none());
    // A path the session does not hold.
    assert!(f.session.definition(&f.main.with_file_name("nope.luau"), 0).is_none());
}
