--!rime
-- A value list shorter than the name list, ending in `nil` (R0608 stays
-- silent, as Selene's `unbalanced_assignments` does): the names past the
-- `nil` are nil either way, so nothing is lost or left out by accident.
local cached, cachedAt = nil
local first, second, third = 1, nil

local function reset()
	cached, cachedAt = nil
end

reset()

return table.freeze({
	count = select("#", cached, cachedAt, first, second, third),
})
