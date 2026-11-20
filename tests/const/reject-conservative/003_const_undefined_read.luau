--!rime
-- A const is in scope from the statement AFTER its declaration to the end
-- of its block, like a local. Reads outside that range resolve to nothing:
--   * a read before the declaration;
--   * a read after the declaring block has ended;
--   * the declaration's own initializer (`const depth = depth + 1` reads
--     an outer `depth`, and there is none).
-- The reads inside the range are silent.
--
-- WHY REJECTED: valid Luau that reads a nil global at runtime.
-- WHAT WOULD FIX IT: declare the const before, and in the scope of, its use.
local M = {}

function M.early(): number
	return LIMIT
	--~ ERROR[R0022]: `LIMIT` is not defined
end

const LIMIT = 10

function M.inside(): number
	do
		const inner = LIMIT * 2
		print(inner)
	end
	return inner
	--~ ERROR[R0022]: `inner` is not defined
end

function M.selfRead(): number
	const depth = depth + 1
	--~ ERROR[R0022]: `depth` is not defined
	return depth
end

return table.freeze(M)
