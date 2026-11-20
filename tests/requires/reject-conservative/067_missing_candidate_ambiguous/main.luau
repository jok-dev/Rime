--!rime
-- Same rot shape, but TWO modules carry the name: the diagnostic lists
-- both and offers NO replacement — a wrong guess written into the file
-- is worse than no fix. Uniqueness gates the auto-fix.
local Maid = require(script.Parent.Old.Maid)
--~ ERROR[R0501]: cannot resolve this require

local function get()
	return Maid
end

return table.freeze({ get = get })
