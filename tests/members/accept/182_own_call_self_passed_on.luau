--!rime
-- A `:` method hands `self` to other code, which is free to add members
-- to it. `main.extended()` may well exist: no R0306.
local registry = {}

function registry.extend(target)
	target.extended = function()
		return 1
	end
end

local main = {}

function main:init()
	registry.extend(self)
end

function main.run()
	main:init()
	return main.extended()
end

return main
