--!rime
--!no-exec
-- A const whose only mention is a function declared on it is not unused
-- (R0013 stays silent, as it does for a `local` in corpus 142):
-- `function extended.describe() end` writes a field of the table the
-- const names, and the caller holds the same table. It is not a
-- reassignment of the const either (R0043): the binding is untouched.
--
-- A const table that is read once is not write-only (corpus 144).
--
-- Static-only: the vendored Luau (0.663) predates `const`.
local function attach(target: { [string]: () -> string })
	const extended = target
	function extended.describe(): string
		return "attached"
	end
end

local function tally(scores: { number }): { number }
	const totals = {}
	for index, score in scores do
		totals[index] = score
	end
	return totals
end

local shared: { [string]: () -> string } = {}
attach(shared)

return table.freeze({ described = shared.describe(), tally = tally })
