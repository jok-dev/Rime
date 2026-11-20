--!rime
-- Both variant shapes: a plain (no ipairs wrapper) GetDescendants call,
-- inside a function — scope does not matter, shape does. Same R0508.
local Handlers = {}

local function load()
	for _, m in script.Parent.Deep:GetDescendants() do
		Handlers[m.Name] = require(m)
		--~ ERROR[R0508]: require target is a loop variable over a folder's children
	end
end

return table.freeze({ load = load, Handlers = Handlers })
