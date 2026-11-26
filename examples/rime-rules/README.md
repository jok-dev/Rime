# Custom project rules — examples

Rime runs project-specific lint rules written in Luau. Put one rule per
file in a `rime-rules/` directory next to your `rime.toml`; `rime check`
discovers them (sorted by file name), loads them into one sandboxed Luau VM,
and runs each rule's `check` over every file its `files` glob matches.
Diagnostics come out through the normal renderer, JSON output, inline
`-- rime: allow(<rule-id>, "reason")` directives, and `[rules]` keys in
`rime.toml` — the rule id is the rule name.

These three files are real rules, verified by `crates/rime-cli/tests/custom_rules.rs`:

| file | shape | what it shows |
|---|---|---|
| `no-direct-destroy.luau` | presence check | walk every call, match by callee name, skip one file by path |
| `module-tag.luau` | absence check | scan root-level assignments, report at the top of the file when nothing matches |
| `cmdr-command-name.luau` | shape check | read the module's `return`, follow a returned name back to its root declaration, inspect table fields |

Copy one into your project's `rime-rules/`, change `files` to your folders,
and run `rime check`.

## The contract

A rule file returns a table:

```lua
return {
	id = "my-rule",            -- optional; defaults to the file stem
	severity = "warn",         -- "error" (default) or "warn"
	files = "src/**/*.lua",    -- one glob or a list; `*` does not cross `/`
	check = function(file, ctx)
		-- file.path  project-relative path with `/` separators
		-- file.src   the full source text
		-- file.root  the module's root node (kind "Body")
		-- ctx:fail(node_or_span, message, note?)  one diagnostic per call
		-- ctx:line(node_or_span)                  1-based line number
	end,
}
```

`file.path`, and what `files` globs match, is relative to the directory
of the rime.toml in use. A file outside that directory, as when the config
lives in a subfolder (`rime check src --config tools/rime-ci.toml`, with
`tools/rime-rules/` beside it), is relative to the Rojo project file's
directory instead, so `files = "src/**"` still matches. With no Rojo project
file, such a file is not checked by custom rules.

`node_or_span` is any node from the tree (its `span` is used) or a table
`{ start = <byte>, ["end"] = <byte> }`. Every `ctx:fail` is its own
diagnostic — never merge occurrences.

Ids matching `R####` or a core rule name are rejected. `check_project` is
not supported yet. A rule file that fails to load, or a `check` that
throws, produces an `R0025 custom-rule-error` on the rule file; the other
rules keep running.

## The node model and helpers

The tree a rule walks is Rime's HIR, converted to plain Lua tables:
`{ kind = "Call", span = { start, ["end"] }, parent = <node>, ...fields }`.
The full list of node kinds and their fields, and the `rime.*` helpers
(`walk`, `find`, `calls`, `callee_name`, `assignments`, `root_assignments`,
`module_return`, `children`), are documented at the top of
[`crates/rime-custom/lua/rime.luau`](../../crates/rime-custom/lua/rime.luau)
— that file is the stdlib itself.

## Sandbox

Rule files see the Luau standard library minus `require`, `os` and `io`,
plus the `rime` global. They cannot reach the filesystem or spawn anything.
Globals are read-only once the VM is sandboxed; keep state in locals.
`--no-custom-rules` skips running rules (files are still loaded so their
names resolve in `rime.toml`); `--timings` shows one `custom: <id>` bucket
per rule.

## Why the `allow-file` line at the top of each example

Inside this repository the example files are ordinary `.luau` files as far
as `rime check` is concerned, and `rime` is not a known global there. The
first line of each example suppresses that one diagnostic for the file.
In your own project's `rime-rules/` the same line is harmless; a future Rime
will recognise `rime-rules/` and not need it.
