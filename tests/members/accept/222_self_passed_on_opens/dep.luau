--!rime
-- OPEN module: a `:` method hands `self` to other code, which is free to
-- add members to it.
local registry = {}

function registry.extend(target)
	target.extended = function()
		return 1
	end
end

local dep = {}

function dep:init()
	registry.extend(self)
end

return dep
