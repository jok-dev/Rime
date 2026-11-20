--!rime
-- An OutsideView require proves its target is OUTSIDE the project view:
-- it neither disables nor degrades R0309, and reads through its binding
-- attribute nothing in-view — consumer.luau's `G.b` cannot silence `b`.
local main = {
	a = 1,
	b = 2,
}
--~ WARNING[R0309]: export `b` of module `main` is never used
return table.freeze(main)
