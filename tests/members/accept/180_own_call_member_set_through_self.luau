--!rime
-- A `:` method of the export runs with the export as `self`, so
-- `self.helper = ...` defines a member of the export just as
-- `main.helper = ...` would. Calls to those members are fine: no R0306.
--   `helper`   set in a `:` method
--   `nested`   set in a closure inside a `:` method
--   `viaParam` set through an explicit `self` parameter
--   `viaField` the same, in a function assigned to a member
local main = {}

function main:init()
	self.helper = function()
		return 1
	end
	local function later()
		self.nested = function()
			return 2
		end
	end
	later()
end

function main.setup(self)
	self.viaParam = function()
		return 3
	end
end

main.attach = function(self)
	self.viaField = function()
		return 4
	end
end

function main.run()
	main:init()
	main:setup()
	main:attach()
	return main.helper() + main:helper() + main.nested() + main.viaParam() + main.viaField()
end

return main
