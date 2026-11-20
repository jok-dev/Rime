--!no-exec
local C = require(script.Parent.c)

local function value()
	return C.value()
end

return table.freeze({ value = value })
