--!rime
-- Each R0309 report carries the extent of the definition it is about (the
-- `extent:` line of the snapshot; `extent` in `--format json`), so a tool
-- can count its lines or delete it. One report per form:
--   `field`, `last`  a table field, with the separator after it if any
--   `LIMIT`          a one-target assignment
--   `handler`        an assigned function, to its `end`
--   `run`, `step`    a function statement, to its `end`
--   `TIMEOUT`, `stop` the same two forms ended by a `;`, which is part of
--                    the extent: Luau does not allow a `;` on its own
-- and two with NO extent, because one span would not remove the member:
--   `mode`           written in two places
--   `left`, `right`  one statement defining two members
-- The comment above `run` is not part of its extent.
local main = {
	used = 1,
	field = { 1, 2 },
	last = "x"
}
--~ WARNING[R0309]: export `field` of module `main` is never used
--~ WARNING[R0309]: export `last` of module `main` is never used

main.LIMIT = 5
--~ WARNING[R0309]: export `LIMIT` of module `main` is never used

main.handler = function()
	return 1
end
--~ WARNING[R0309]: export `handler` of module `main` is never used

-- Runs it.
function main.run(count: number): number
	local total = count
	return total
end
--~ WARNING[R0309]: export `run` of module `main` is never used

function main:step() end
--~ WARNING[R0309]: export `step` of module `main` is never used

if main.used > 0 then
	main.mode = "a"
else
	main.mode = "b"
end
--~ WARNING[R0309]: export `mode` of module `main` is never used

main.left, main.right = 1, 2
--~ WARNING[R0309]: export `left` of module `main` is never used
--~ WARNING[R0309]: export `right` of module `main` is never used

main.TIMEOUT = 9;
--~ WARNING[R0309]: export `TIMEOUT` of module `main` is never used

function main.stop() end;
--~ WARNING[R0309]: export `stop` of module `main` is never used

return main
