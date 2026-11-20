--!rime
-- Degrade mode (loader.luau requires dynamically). A string equal to a
-- member's name marks the report, except one written inside that member's
-- own function. For it to reach a lookup the function must already be
-- running, and nothing else names it: `save` logging its own name is no
-- sign that `save` is used, so its report carries no mark. The nested
-- closure is still inside `save`.
-- `load` has the same string in its own body AND in `relay`, a different
-- function: a live `relay` could look `load` up by it. The report for
-- `load` is marked, with the line in `relay` only.
local main = {}

local function log(...: string) end

main.used = 1

function main.save()
	log("save", "started")
	task.defer(function()
		log("save", "done")
	end)
end
--~ WARNING[R0309]: export `save` of module `main` is never used

function main.load()
	log("load")
end
--~ WARNING[R0309]: export `load` of module `main` is never used

function main.relay()
	log("load")
end

return main
