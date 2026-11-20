--!rime
-- R0306 inside the defining module. `main` defines `LIMIT`, `start` and
-- `finish` and nothing else, and the table never leaves this file, so
-- `main.stop` and `main:reset` name members that cannot exist: what is
-- left behind when a function is deleted and a caller in its own file is
-- missed. One report per call, wherever the call sits.
local main = {}

main.LIMIT = 3

function main.start()
	main.stop()
	--~ ERROR[R0306]: `stop` is not a member of this module's export `main`
	task.defer(function()
		main:reset()
		--~ ERROR[R0306]: `reset` is not a member of this module's export `main`
	end)
end

function main.finish()
	return main.stop(main.LIMIT)
end

return main
