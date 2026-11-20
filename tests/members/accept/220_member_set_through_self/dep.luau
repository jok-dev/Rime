--!rime
-- A `:` method of the export runs with the export as `self`, so what it
-- sets on `self` is a member of the module:
--   `helper`  set in a `:` method
--   `cache`   a value, set in a closure inside a `:` method
--   `label`   set through an explicit `self` parameter
local dep = {}

function dep:init()
	self.helper = function()
		return 1
	end
	local function fill()
		self.cache = {}
	end
	fill()
end

function dep.name(self)
	self.label = "dep"
end

return dep
