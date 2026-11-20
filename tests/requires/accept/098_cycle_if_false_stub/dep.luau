--!no-exec
local Main = require(script.Parent.main)

local function value()
	return Main.value()
end

return table.freeze({ value = value })
