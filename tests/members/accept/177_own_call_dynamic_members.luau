--!rime
-- The export gets members under computed keys, so this file does not say
-- what its members are. `main.alpha()` may well exist: no R0306.
local main = {}

for _, name in { "alpha", "beta" } do
	main[name] = function()
		return name
	end
end

function main.run()
	return main.alpha()
end

return main
