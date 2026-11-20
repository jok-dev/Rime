--!rime
-- The guard-in-a-loop case: `label`'s only read guards its own fallback,
-- and nothing consumes the value — the whole binding is faint. R0017
-- points at the DECLARATION, and the misleading loop-carried R0104 on the
-- fallback store is suppressed (one defect, one diagnostic).
local LABELS = { "low", "mid", "high" }

local function countLevels(levels: { number }): number
	local count = 0
	for index = 1, #levels do
		local label = LABELS[levels[index]]
		if not label then
			label = "?"
		end
		count += 1
	end
	return count
end

return table.freeze({ countLevels = countLevels })
--~ WARNING[R0017]: `label` only feeds itself; its value is never used
