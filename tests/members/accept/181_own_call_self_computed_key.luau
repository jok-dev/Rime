--!rime
-- A `:` method stores members on `self` under computed keys, so this file
-- does not say what the export's members are. `main.logger()` may well
-- exist: no R0306.
local main = {}

function main:inject(dependencies)
	for name, dependency in dependencies do
		self[name] = dependency
	end
end

function main.run()
	main:inject({ logger = print })
	return main.logger("ready")
end

return main
