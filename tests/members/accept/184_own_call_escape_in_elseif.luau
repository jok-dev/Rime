--!rime
-- The export is handed to other code in an `elseif` condition. That is
-- as much an escape as in any other place: `register` may add members,
-- so `main.added()` may well exist. No R0306.
local main = {}

local function register(target)
	target.added = function()
		return 1
	end
	return true
end

function main.run(x: number)
	if x == 1 then
		return 1
	elseif register(main) then
		return main.added()
	end
	return 0
end

return main
