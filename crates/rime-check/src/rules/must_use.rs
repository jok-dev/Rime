//! R0708 must-use: a call statement to a library function that does
//! nothing but compute a value, with the value dropped (selene parity
//! `must_use`, 2026-11-20).
//!
//! ```lua
//! string.format("%d coins", coins)   -- formats, and throws the text away
//! math.max(a, b)
//! ```
//!
//! Warning, conservative. The usual cause is a lost `local x =` or a call
//! pasted without its use, but the statement still runs: an argument can
//! have side effects, and the call itself can be there to throw.
//!
//! Which functions: `MUST_USE` below is every function Selene
//! flags `must_use` under `std = "lua53+roblox"` with a generated
//! `roblox.yml`. The list was made by running Selene on one call
//! statement per function in its std files, not by reading the YAML: a
//! later std file can replace an entry of an earlier one. The names are the
//! only std data this rule has. No argument is checked against anything.
//!
//! Same as Selene:
//!   * only a call in statement position, called through its global name:
//!     `math.max(a, b)`, `tostring(x)`, also under a block or a function
//!   * `Enum.<Name>.GetEnumItems()`, `Enum.GetEnums()` and
//!     `game.GetService(game, name)` called with a dot
//!   * a method call (`s:rep(3)`), a call on a call's result
//!     (`tostring(x):upper()`), a bracket index and a parenthesized callee
//!     are not looked at
//!
//! Differences from Selene, all deliberate:
//!   * Selene looks the written name up in its std without asking what the
//!     name is. Rime is silent when the first name is a local
//!     (`local math = Fixed`, a parameter named `string`).
//!   * Selene reports `math.max:foo()` and `tostring:foo()`: it drops the
//!     method name and looks up what is left. Rime does not.
//!   * `Instance.new(class, parent)` puts the new instance in the game, and
//!     `string.gsub(s, pattern, f)` calls `f` once per match. Selene reports
//!     both; Rime is silent when `Instance.new` has a second argument and
//!     when the third argument of `string.gsub` is not a string or a table
//!     written in place.
//!   * Selene checks that the enum in `Enum.<Name>.GetEnumItems()` exists.
//!     Rime does not have the enum list and takes any name.
//!
//! Known and left as Selene has it: `math.random()` on its own line moves
//! the shared random generator on, so it is not dead, and it is reported.

use crate::scope;
use rime_diag::{Code, Diagnostic, Label};
use rime_hir as hir;

/// Sorted by byte value (the test below checks), for `binary_search`.
const MUST_USE: &[&str] = &[
    "Axes.new", "BrickColor.Black", "BrickColor.Blue", "BrickColor.DarkGray", "BrickColor.Gray",
    "BrickColor.Green", "BrickColor.Red", "BrickColor.White", "BrickColor.Yellow",
    "BrickColor.new", "BrickColor.palette", "BrickColor.random", "CFrame.Angles",
    "CFrame.fromAxisAngle", "CFrame.fromEulerAngles", "CFrame.fromEulerAnglesXYZ",
    "CFrame.fromEulerAnglesYXZ", "CFrame.fromMatrix", "CFrame.fromOrientation",
    "CFrame.fromRotationBetweenVectors", "CFrame.lookAlong", "CFrame.lookAt", "CFrame.new",
    "CatalogSearchParams.new", "Color3.fromHSV", "Color3.fromHex", "Color3.fromRGB",
    "Color3.new", "Color3.toHSV", "ColorSequence.new", "ColorSequenceKeypoint.new",
    "Content.fromObject", "Content.fromUri", "DateTime.fromIsoDate", "DateTime.fromLocalTime",
    "DateTime.fromUniversalTime", "DateTime.fromUnixTimestamp",
    "DateTime.fromUnixTimestampMillis", "DateTime.now", "DebuggerManager",
    "DockWidgetPluginGuiInfo.new", "Faces.new", "FloatCurveKey.new", "Font.fromEnum",
    "Font.fromId", "Font.fromName", "Font.new", "Instance.fromExisting", "Instance.new",
    "NumberRange.new", "NumberSequence.new", "NumberSequenceKeypoint.new", "OverlapParams.new",
    "Path2DControlPoint.new", "PathWaypoint.new", "PhysicalProperties.new", "Random.new",
    "Ray.new", "RaycastParams.new", "Rect.new", "Region3.new", "Region3int16.new",
    "RotationCurveKey.new", "SharedTable.clone", "SharedTable.cloneAndFreeze",
    "SharedTable.isFrozen", "SharedTable.new", "SharedTable.size", "TweenInfo.new", "UDim.new",
    "UDim2.fromOffset", "UDim2.fromScale", "UDim2.new", "UserSettings", "Vector2.new",
    "Vector2int16.new", "Vector3.FromAxis", "Vector3.FromNormalId", "Vector3.new",
    "bit32.arshift", "bit32.band", "bit32.bnot", "bit32.bor", "bit32.btest", "bit32.bxor",
    "bit32.byteswap", "bit32.countlz", "bit32.countrz", "bit32.extract", "bit32.lrotate",
    "bit32.lshift", "bit32.replace", "bit32.rrotate", "bit32.rshift", "buffer.create",
    "buffer.fromstring", "buffer.len", "buffer.readf32", "buffer.readf64", "buffer.readi16",
    "buffer.readi32", "buffer.readi8", "buffer.readstring", "buffer.readu16", "buffer.readu32",
    "buffer.readu8", "buffer.tostring", "coroutine.create", "coroutine.running",
    "coroutine.status", "debug.getfenv", "debug.gethook", "debug.getinfo", "debug.getlocal",
    "debug.getmetatable", "debug.getregistry", "debug.getupvalue", "debug.info",
    "debug.traceback", "gcinfo", "getfenv", "getmetatable", "ipairs", "math.abs", "math.acos",
    "math.asin", "math.atan", "math.atan2", "math.ceil", "math.clamp", "math.cos", "math.cosh",
    "math.deg", "math.exp", "math.floor", "math.fmod", "math.frexp", "math.ldexp", "math.log10",
    "math.map", "math.max", "math.min", "math.modf", "math.noise", "math.pow", "math.rad",
    "math.random", "math.round", "math.sign", "math.sin", "math.sinh", "math.sqrt", "math.tan",
    "math.tanh", "math.tointeger", "newproxy", "os.clock", "os.date", "os.difftime", "os.time",
    "os.tmpname", "pairs", "rawequal", "rawget", "rawlen", "select", "string.char",
    "string.dump", "string.find", "string.format", "string.gmatch", "string.gsub", "string.len",
    "string.lower", "string.match", "string.pack", "string.packsize", "string.rep",
    "string.reverse", "string.split", "string.sub", "string.unpack", "string.upper",
    "table.clone", "table.concat", "table.create", "table.find", "table.getn", "table.isfrozen",
    "table.maxn", "table.pack", "tonumber", "tostring", "unpack", "utf8.char", "utf8.codepoint",
    "utf8.codes", "utf8.graphemes", "utf8.len", "utf8.nfcnormalize", "utf8.nfdnormalize",
    "utf8.offest", "utf8.offset", "vector.abs", "vector.angle", "vector.ceil", "vector.clamp",
    "vector.create", "vector.cross", "vector.dot", "vector.floor", "vector.magnitude",
    "vector.max", "vector.min", "vector.normalize", "vector.sign",
];

/// One call statement whose callee is on the list.
struct Finding<'a> {
    /// `math.max`, as written.
    path: String,
    call: &'a hir::Span,
}

pub(crate) fn check_module(
    module: &hir::Module,
    res: &scope::Resolution,
    diags: &mut Vec<Diagnostic>,
) {
    for body in &module.bodies {
        scope::each_stmt(&body.block, &mut |stmt| {
            let Some(found) = find(stmt, res) else { return };
            diags.push(
                Diagnostic::warning(
                    Code(708),
                    format!("the result of `{}` is not used", found.path),
                )
                .with_label(Label::primary(
                    found.call.clone(),
                    "this call only computes a value, and the value is dropped",
                ))
                .conservative(
                    "the call still runs: an argument can have side effects, and \
                     the call can be there to throw",
                )
                .with_help("use the result, or remove the call"),
            );
        });
    }
}

fn find<'a>(stmt: &'a hir::Stmt, res: &scope::Resolution) -> Option<Finding<'a>> {
    let hir::StmtKind::Call(hir::Expr::Call {
        base,
        args,
        method: None,
        ..
    }) = &stmt.kind
    else {
        return None;
    };
    let mut path = Vec::new();
    let root = name_path(base, &mut path)?;
    // `(tostring)(1)`: the statement starts at the parenthesis, not at the
    // name.
    if root.start != stmt.span.start || res.is_local(root) {
        return None;
    }
    let listed = match path.as_slice() {
        ["Enum", "GetEnums"] | ["Enum", _, "GetEnumItems"] | ["game", "GetService"] => true,
        _ => MUST_USE.binary_search(&path.join(".").as_str()).is_ok(),
    };
    if !listed {
        return None;
    }
    let acts = match path.as_slice() {
        // The second argument is the parent: the instance goes into the game.
        ["Instance", "new"] => args.len() >= 2,
        // A function as the replacement is called once per match. Only a
        // string or a table written in place is known not to be one.
        ["string", "gsub"] => !matches!(
            args.get(2),
            None | Some(hir::Expr::Literal {
                prim: hir::Prim::String,
                ..
            }) | Some(hir::Expr::Table(_))
        ),
        _ => false,
    };
    (!acts).then(|| Finding {
        path: path.join("."),
        call: &stmt.span,
    })
}

/// `a.b.c` as its names, root first. Returns the root name's span. `None`
/// for anything that is not a plain name followed by dot fields.
fn name_path<'a>(e: &'a hir::Expr, path: &mut Vec<&'a str>) -> Option<&'a hir::Span> {
    match e {
        hir::Expr::Name { name, span } => {
            path.push(name);
            Some(span)
        }
        hir::Expr::Index {
            base,
            key: None,
            name: Some(field),
            ..
        } => {
            let root = name_path(base, path)?;
            path.push(field);
            Some(root)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diags(src: &str) -> Vec<Diagnostic> {
        let module = rime_syntax::parse(src).unwrap();
        let res = scope::resolve(&module);
        let mut diags = Vec::new();
        check_module(&module, &res, &mut diags);
        diags.sort_by_key(|d| d.primary_span().start);
        diags
    }

    /// (line, message) of every report, in source order.
    fn reports(src: &str) -> Vec<(usize, String)> {
        diags(src)
            .iter()
            .map(|d| {
                assert_eq!(d.code.to_string(), "R0708");
                (
                    src[..d.primary_span().start].matches('\n').count() + 1,
                    d.message.clone(),
                )
            })
            .collect()
    }

    fn lines(src: &str) -> Vec<usize> {
        reports(src).into_iter().map(|(line, _)| line).collect()
    }

    #[test]
    fn the_list_is_sorted_and_is_what_selene_flags() {
        let mut sorted = MUST_USE.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted, MUST_USE, "MUST_USE must be sorted, without repeats");
        // 216 functions reported by Selene with std = "lua53+roblox"
        // and a roblox.yml generated by `selene generate-roblox-std`
        // (2026-11-20).
        assert_eq!(MUST_USE.len(), 216);
        for name in ["math.max", "tostring", "Instance.new", "Vector3.new", "string.format", "os.time"] {
            assert!(MUST_USE.binary_search(&name).is_ok(), "{name}");
        }
        for name in ["print", "table.insert", "table.sort", "math.randomseed", "task.wait", "pcall"] {
            assert!(MUST_USE.binary_search(&name).is_err(), "{name}");
        }
    }

    // selene-lib/tests/lints/must_use/must_use.lua: Selene reports line 1.
    #[test]
    fn selene_fixture() {
        let src = "\
math.max(x, y)
print(math.max(x, y))

print(\"You don't need to use my return value\")
";
        assert_eq!(
            reports(src),
            vec![(1, "the result of `math.max` is not used".to_string())]
        );
    }

    // Each line checked against Selene (std = "lua53+roblox"):
    // it reports exactly these lines too.
    #[test]
    fn the_call_shapes_selene_reports() {
        let src = "\
Enum.AccessoryType.GetEnumItems()
game.GetService(game, \"Players\")
Enum.GetEnums()
string.len \"x\"
string.len(\"x\")
local function f()
	string.rep(\"a\", 2)
end
if true then tostring(1) end
os.time()
select(\"#\")
DateTime.now()
Vector3.new(1, 2, 3)
return f
";
        assert_eq!(lines(src), vec![1, 2, 3, 4, 5, 7, 9, 10, 11, 12, 13]);
    }

    // Selene is silent on every one of these as well.
    #[test]
    fn the_call_shapes_selene_leaves_alone() {
        let src = "\
Enum.AccessoryType:GetEnumItems()
game:GetService(\"Players\")
string.format(\"x\"):len()
tostring(1):upper()
Instance.new(\"Part\"):Destroy()
local t = { string.rep(\"a\", 2) }
(tostring)(1)
tostring(1)()
table.concat({})[\"x\"]()
table[\"concat\"]({})
pcall(tostring, 1)
workspace.Foo.Clone()
DateTime.now():ToIsoDate()
Vector3.new(1, 2, 3):Dot()
Color3.new().r()
CFrame.new().Position.X()
print(math.max(1, 2))
local n = math.max(1, 2)
return t, n
";
        assert_eq!(lines(src), Vec::<usize>::new());
    }

    // Deliberate difference: Selene looks the written name up in its std
    // and reports lines 2, 5, 8 and 10 here. The name is a local in each.
    #[test]
    fn a_local_named_like_a_library_is_not_the_library() {
        let src = "\
local math = { max = print }
math.max(1, 2)
local function f(string)
	local tostring = print
	tostring(1)
	do
		local os = string
		os.time()
	end
	string.rep(\"a\", 2)
end
return f
";
        assert_eq!(lines(src), Vec::<usize>::new());
    }

    // A local declared after the call, or in a scope that has closed, hides
    // nothing.
    #[test]
    fn a_local_not_in_scope_at_the_call_changes_nothing() {
        let src = "\
do
	local math = {}
	print(math)
end
math.max(1, 2)
local math = math
return math
";
        assert_eq!(lines(src), vec![5]);
    }

    // Deliberate difference: Selene drops the method name and looks up what
    // is left, so it reports both lines.
    #[test]
    fn a_method_call_on_a_listed_name_is_not_reported() {
        let src = "math.max:foo()\ntostring:foo()\n";
        assert_eq!(lines(src), Vec::<usize>::new());
    }

    // Deliberate difference: Selene reports all seven lines. Lines 1 and 3
    // do something besides return a value, and line 4 may: `visit` can be
    // a function.
    #[test]
    fn calls_that_act_through_an_argument_are_silent() {
        let src = "\
Instance.new(\"Folder\", workspace)
Instance.new(\"Folder\")
string.gsub(text, \"%w+\", function(word) count += 1 end)
string.gsub(text, \"%w+\", visit)
string.gsub(text, \"%w+\", \"x\")
string.gsub(text, \"%w+\", { a = \"b\" })
string.gsub(text, \"%w+\")
";
        assert_eq!(lines(src), vec![2, 5, 6, 7]);
    }

    // Known, and the same in Selene: this moves the random generator on.
    #[test]
    fn math_random_is_reported_as_selene_reports_it() {
        assert_eq!(lines("math.random()\nmath.random(1, 6)\n"), vec![1, 2]);
    }

    #[test]
    fn the_report_is_a_conservative_warning_on_the_whole_call() {
        let src = "local a, b = 1, 2\nmath.max(a, b)\nreturn a\n";
        let got = diags(src);
        assert_eq!(got.len(), 1, "{got:?}");
        let d = &got[0];
        assert_eq!(d.severity, rime_diag::Severity::Warning);
        assert!(d.conservative);
        assert_eq!(&src[d.primary_span()], "math.max(a, b)");
        assert_eq!(d.message, "the result of `math.max` is not used");
        assert!(d.suggestions.is_empty(), "no edit: Rime cannot tell which fix is meant");
    }

    #[test]
    fn one_report_per_statement_in_every_body() {
        let src = "\
local M = {}
function M.a()
	tostring(1)
	return function()
		tonumber(\"1\")
	end
end
function M:b()
	while true do
		repeat
			rawequal(1, 2)
		until true
	end
end
return M
";
        assert_eq!(lines(src), vec![3, 5, 11]);
    }
}
