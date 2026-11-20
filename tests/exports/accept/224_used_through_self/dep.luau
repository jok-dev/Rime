--!rime
-- `step` and `limit` are used by `dep`'s own `run`, through `self`. Inside
-- a `:` method `self` is the export, so that is the module using its own
-- members, the same as `dep.step()` would be: no R0309.
local dep = {}

dep.limit = 3

function dep:step(count: number): number
	return count + 1
end

function dep:run(): number
	local count = 0
	while count < self.limit do
		count = self:step(count)
	end
	return count
end

return dep
