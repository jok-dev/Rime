--!rime
-- Closed module exporting a `:` method.
local dep = {}

function dep:reset()
	print("reset")
end

return table.freeze(dep)
