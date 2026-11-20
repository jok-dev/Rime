--!rime
-- A const must be given its value where it is declared: there is no later
-- assignment that could supply one. Luau rejects the file ("Missing
-- initializer in const declaration"), and so does the parser Rime uses.
local M = {}

const LIMIT
--~ ERROR[R0900]: could not parse this file as Luau

function M.limit(): number
	return LIMIT
end

return table.freeze(M)
