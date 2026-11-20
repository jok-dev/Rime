--!rime
-- Constant range counting the wrong way (R0605 widened 2026-09-04,
-- parity with Luau's ForRange `for i=8,1 do`).
--
-- WHY REJECTED: start above end with the default +1 step — the body never
-- runs, and the shape reads as a countdown that forgot its `-1`.
-- WHAT WOULD FIX IT: `for i = 8, 1, -1`.
local function countdown(): { number }
	local out = {}
	--~ ERROR[R0605]: `for` from 8 down to 1 without a `-1` step
	for i = 8, 1 do
		table.insert(out, i)
	end
	return out
end

-- Explicit positive step, same defect.
local function countdownByTwo(): { number }
	local out = {}
	--~ ERROR[R0605]: `for` from 10 down to 2 without a `-1` step
	for i = 10, 2, 2 do
		table.insert(out, i)
	end
	return out
end

return table.freeze({ countdown = countdown, countdownByTwo = countdownByTwo })
