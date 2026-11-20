--!rime
--!no-exec
-- Color3.new bounds (R0801, selene parity 2026-08-21).
--
-- WHY REJECTED: Color3.new channels are 0–1 and CLAMP: `Color3.new(255,
-- 128, 0)` renders as (1, 1, 0) — yellow, not orange. 0–255 values belong
-- to Color3.fromRGB.
-- WHAT WOULD FIX IT: `Color3.fromRGB(255, 128, 0)`.
local function orange(): Color3
	--~ WARNING[R0801]: `Color3.new` takes channels from 0 to 1
	return Color3.new(255, 128, 0)
end

return table.freeze({ orange = orange })
