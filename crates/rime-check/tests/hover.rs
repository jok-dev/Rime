//! `Session::hover`: the documentation popup for a token.
//! Reuses go-to-definition to find the declaration, then renders its
//! leading `--` comment block and its declaration line as Markdown.

#[path = "common/sibling.rs"]
mod sibling;
use sibling::SiblingResolver;

use rime_check::session::Session;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

const DEP: &str = "\
local Dep = {}

--- The maximum speed, in studs per second.
Dep.MAX_SPEED = 100

--- Applies `amount` of damage and returns the survivor's new health.
--- Clamped at zero.
function Dep.hurt(amount: number): number
	return amount
end

Dep.Limits = {
	Retries = 3,
}

--[[
	Resets the module to its defaults.
	Idempotent.
]]
function Dep.reset()
end

--[=[
	Level-one long bracket. Contains ]] inside.
]=]
function Dep.tricky()
end

--[=[
	Dep.split
	@param text string The text to cut up
	@param sep string Where to cut; never empty
	@returns { string } The pieces, in order

	@desc Cuts `text` at every `sep`. Two separators in a row
		give an empty piece.
]=]
function Dep.split(text: string, sep: string): { string }
	return { text, sep }
end

return Dep
";

const MAIN: &str = "\
local Dep = require(script.Parent.dep)

local greeting: string = \"hi\"
print(greeting)

local total = Dep.hurt(5)
print(total, Dep.MAX_SPEED, Dep.Limits.Retries)

return Dep
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
    let dir = std::env::temp_dir().join(format!("rime-hover-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("main.luau"), MAIN).unwrap();
    std::fs::write(dir.join("dep.luau"), DEP).unwrap();
    let dir = dir.canonicalize().unwrap();
    let main = dir.join("main.luau");
    let dep = dir.join("dep.luau");
    let files = vec![(main.clone(), MAIN.to_string()), (dep.clone(), DEP.to_string())];
    let session =
        Session::new(files, Some(Box::new(SiblingResolver)), rime_std::StdNames::bundled().clone(), None);
    Fixture { dir, main, dep, session }
}

/// Byte offset of the `nth` occurrence of `needle`, plus `plus`.
fn at(src: &str, needle: &str, nth: usize, plus: usize) -> usize {
    let mut from = 0;
    for _ in 0..nth {
        from = src[from..].find(needle).map(|i| from + i + needle.len()).unwrap();
    }
    src[from..].find(needle).map(|i| from + i + plus).unwrap()
}

#[test]
fn function_hover_shows_signature_and_doc() {
    let f = fixture();
    // Hover the call `Dep.hurt(5)` on the `hurt`.
    let h = f.session.hover(&f.main, at(MAIN, "Dep.hurt(5)", 0, 4)).expect("hover");
    assert!(h.contents.contains("function Dep.hurt(amount: number): number"), "signature: {}", h.contents);
    assert!(h.contents.contains("Applies `amount` of damage"), "doc line 1: {}", h.contents);
    assert!(h.contents.contains("Clamped at zero."), "doc line 2: {}", h.contents);
    // The hovered range covers the whole `hurt` identifier.
    assert_eq!(&MAIN[h.range.clone()], "hurt");
}

#[test]
fn typed_local_hover_shows_its_declaration() {
    let f = fixture();
    // Hover the use `print(greeting)`.
    let h = f.session.hover(&f.main, at(MAIN, "print(greeting)", 0, 6)).expect("hover");
    assert!(h.contents.contains("local greeting: string"), "{}", h.contents);
    assert_eq!(&MAIN[h.range.clone()], "greeting");
}

#[test]
fn field_hover_shows_the_member_line() {
    let f = fixture();
    // A doc-commented static field.
    let h = f.session.hover(&f.main, at(MAIN, "Dep.MAX_SPEED", 0, 4)).expect("hover");
    assert!(h.contents.contains("Dep.MAX_SPEED = 100"), "{}", h.contents);
    assert!(h.contents.contains("maximum speed"), "doc: {}", h.contents);
    // A nested key inside a table literal.
    let h = f.session.hover(&f.main, at(MAIN, "Dep.Limits.Retries", 0, 11)).expect("hover");
    assert!(h.contents.contains("Retries = 3"), "{}", h.contents);
}

#[test]
fn hovering_a_declaration_shows_its_own_doc() {
    let f = fixture();
    // Hover the function's OWN declaration name in dep.luau (not a call).
    let h = f.session.hover(&f.dep, at(DEP, "function Dep.hurt", 0, 13)).expect("hover decl");
    assert!(h.contents.contains("function Dep.hurt(amount: number): number"), "{}", h.contents);
    assert!(h.contents.contains("Applies `amount` of damage"), "doc: {}", h.contents);
    assert_eq!(&DEP[h.range.clone()], "hurt");
    // Hover a `local` at its declaration.
    let h = f.session.hover(&f.main, at(MAIN, "local greeting", 0, 6)).expect("hover local decl");
    assert!(h.contents.contains("local greeting: string"), "{}", h.contents);
}

#[test]
fn block_comments_are_shown_as_docs() {
    let f = fixture();
    // `--[[ ]]` block comment above a declaration.
    let h = f.session.hover(&f.dep, at(DEP, "function Dep.reset", 0, 13)).expect("reset");
    assert!(h.contents.contains("Resets the module to its defaults."), "{}", h.contents);
    assert!(h.contents.contains("Idempotent."), "{}", h.contents);
    assert!(h.contents.contains("function Dep.reset()"), "sig: {}", h.contents);
    // `--[=[ ]=]` long bracket, whose body contains a bare `]]`.
    let h = f.session.hover(&f.dep, at(DEP, "function Dep.tricky", 0, 13)).expect("tricky");
    assert!(h.contents.contains("Level-one long bracket. Contains ]] inside."), "{}", h.contents);
}

#[test]
fn moonwave_tags_render_as_structured_sections() {
    let f = fixture();
    let h = f.session.hover(&f.dep, at(DEP, "function Dep.split", 0, 13)).expect("split");
    let c = &h.contents;
    // Signature fence.
    assert!(c.contains("function Dep.split(text: string, sep: string): { string }"), "sig: {c}");
    // The redundant `Dep.split` name header is dropped.
    assert!(!c.contains("Dep.split\n"), "name header should be dropped: {c}");
    // Description prose (the two source lines collapse to one paragraph).
    assert!(c.contains("Cuts `text` at every `sep`. Two separators in a row give an empty piece."), "{c}");
    // Structured parameter and return sections.
    assert!(c.contains("**Parameters**"), "{c}");
    assert!(c.contains("`text` `string` — The text to cut up"), "{c}");
    assert!(c.contains("`sep` `string` — Where to cut; never empty"), "{c}");
    assert!(c.contains("**Returns**"), "{c}");
    assert!(c.contains("`{ string }` — The pieces, in order"), "{c}");
}

#[test]
fn require_hover_names_the_module() {
    let f = fixture();
    let h = f.session.hover(&f.main, at(MAIN, "script.Parent.dep", 0, 15)).expect("hover");
    assert!(h.contents.contains("dep"), "should name the module file: {}", h.contents);
}

#[test]
fn hover_on_nothing_is_none() {
    let f = fixture();
    // A builtin has no declaration.
    assert!(f.session.hover(&f.main, at(MAIN, "print(greeting)", 0, 0)).is_none());
    // Whitespace between statements.
    assert!(f.session.hover(&f.main, at(MAIN, "\n\nlocal greeting", 0, 1)).is_none());
    // A path the session does not hold.
    assert!(f.session.hover(&f.main.with_file_name("nope.luau"), 0).is_none());
}
