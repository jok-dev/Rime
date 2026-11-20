--!rime
--!no-exec
-- Writes near a const that are NOT a const reassignment:
--   * a `local` initialised from a const is its own binding, and writing
--     to it leaves the const alone;
--   * a const declared in one block is gone when the block ends, so the
--     same name can then be declared as a `local` and assigned.
--
-- Static-only: the vendored Luau (0.663) predates `const`.
local M = {}

function M.sum(items: { number }): number
	const START = 0
	local total = START
	for _, n in ipairs(items) do
		total += n
	end
	return total
end

function M.pick(flag: boolean): number
	if flag then
		const chosen = 1
		return chosen
	end
	local chosen = 2
	chosen += 1
	return chosen
end

return table.freeze(M)
