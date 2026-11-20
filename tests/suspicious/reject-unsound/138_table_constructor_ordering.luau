--!rime
--!no-exec
-- Ordering against a table constructor, and two constructors compared
-- (R0607, selene parity `constant_table_comparison`, 2026-11-20).
--
-- WHY REJECTED: Luau orders two numbers, two strings, or two values that
-- share one `__lt`/`__le` metamethod. A just-constructed table has no
-- metatable, so `scores > {}` raises "attempt to compare" every time it
-- runs. And `{} == {}` is false whatever the program does: each
-- constructor makes its own table, and neither can carry an `__eq`.
-- WHAT WOULD FIX IT: compare lengths (`#scores > 0`), or test for
-- emptiness with `next(scores) ~= nil`.
local function hasScores(scores: { number }): boolean
	--~ ERROR[R0607]: `>` with a table constructor
	return scores > {}
end

local function alwaysFalse(): boolean
	--~ ERROR[R0607]: comparison with a table constructor
	return {} == {}
end

return table.freeze({ hasScores = hasScores, alwaysFalse = alwaysFalse })
