--!rime
--!no-exec
-- Case 11 (plan §9, §5): narrowing on a DEEPLY frozen value survives a
-- yield — the world cannot mutate what cannot change. This is the escape
-- valve that makes the yield rule tolerable.
--
-- EXPECTED TO GO RED when the milestone narrowing analysis lands: flow
-- analysis alone only proves Shallow, and only Deep licenses surviving a
-- yield, so the frozen exemption waits for the type system. Back to green
-- when DeepFrozen-by-type exists. (`--!no-exec`: `task.wait` is not in
-- vanilla Luau, so the differential runner must not execute this case.)
local config: { speed: number }? = table.freeze({ speed = 16 })

local function report(): ()
	if config ~= nil then
		task.wait(1)
		print(config.speed)
	end
end

return table.freeze({ report = report })
