--!rime
-- Case 9, accept half (plan §9): the exported table is frozen, so sharing it
-- through require's cache is safe.
local M = {}

function M.greet(): string
	return "hello"
end

return table.freeze(M)
