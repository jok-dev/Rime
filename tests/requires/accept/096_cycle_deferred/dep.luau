--!no-exec
local Main = require(script.Parent.main)

local function value()
	Main.init()
	return Main.value()
end

return table.freeze({ value = value })
