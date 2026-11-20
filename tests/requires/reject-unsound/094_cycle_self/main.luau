--!rime
-- A module that requires itself at load time: the one-node ring. Throws
-- the same way a two-module ring does.
local Me = require(script.Parent.main)
--~ ERROR[R0509]: circular require
--~ NOTE: cycle: main.luau → main.luau

local function me()
	return Me
end

return table.freeze({ me = me })
