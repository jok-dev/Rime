--!rime
--!no-exec
-- A diamond is not a ring: main loads a and b, both load c. Every module
-- loads once and nothing waits on itself.
local A = require(script.Parent.a)
local B = require(script.Parent.b)

local function sum()
	return A.value() + B.value()
end

return table.freeze({ sum = sum })
