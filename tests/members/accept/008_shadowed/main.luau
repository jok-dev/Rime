--!rime
--!no-exec
-- Inside `fake`, `Economy` is a different local — membership of the module
-- interface must not be checked through the shadow.
local Economy = require(script.Parent.dep)

local function fake()
	local Economy = {
		grantt = function()
			print("fake")
		end,
	}
	Economy.grantt("x")
	print(Economy.bogus)
end

fake()
Economy.grant("sam")

return table.freeze({})
