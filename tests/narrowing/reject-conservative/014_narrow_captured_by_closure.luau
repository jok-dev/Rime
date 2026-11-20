--!rime
-- Case 14 (plan §9, §"Aliasing"): a local captured by a closure never
-- narrows — the closure is an alias that can write the binding at any time
-- Rime cannot see (here: whenever someone calls the stored handler).
--
-- WHY REJECTED: no call stands between the guard and the use; the capture
-- ALONE poisons the narrowing. Distinct from case 13 on purpose.
-- WHAT WOULD FIX IT: copy to an uncaptured local first
-- (`local t = target; if t ~= nil then ...`).
--
-- Reported as R0201, marked conservative.
local handlers: { () -> () } = {}

local function attach(target: Instance?): ()
	table.insert(handlers, function()
		target = nil
	end)
	if target ~= nil then
		print(target.Name)
		--~ ERROR[R0201]: `target` may be nil
	end
end

return table.freeze({ attach = attach })
