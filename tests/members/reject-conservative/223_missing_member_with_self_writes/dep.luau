--!rime
-- Closed module. `helper` is set through `self` and is a member. `inner`
-- is set through the `self` of `dep.other:fill`, which is `dep.other`:
-- not a member of `dep`.
local dep = {}

dep.other = {}

function dep:init()
	self.helper = function()
		return 1
	end
end

function dep.other:fill()
	self.inner = 2
end

return dep
