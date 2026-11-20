--!rime
-- OPEN module: a `:` method stores members on `self` under computed keys,
-- so this file does not say what the module's members are.
local dep = {}

function dep:inject(dependencies)
	for name, dependency in dependencies do
		self[name] = dependency
	end
end

return dep
