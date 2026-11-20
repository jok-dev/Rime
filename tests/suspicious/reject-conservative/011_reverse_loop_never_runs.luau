--!rime
-- Reverse loop whose range is provably empty (R0605 widened 2026-09-04
-- to cover `for i = #t, -1, 1`: a backwards walk written this way has
-- never run once, and whatever it was meant to do to the list has never
-- happened).
--
-- WHY REJECTED: `#t` is never below 0, so with a positive step the start
-- sits above the end on every entry — the body is dead code. Luau's own
-- ForRange lint goes silent whenever ANY step is written, which is exactly
-- how a loop like this goes unnoticed.
-- WHAT WOULD FIX IT: `for i = #lines, 1, -1`.
local function dropBlankTail(lines: { string })
	--~ ERROR[R0605]: `for` from `#lines` down to -1 never runs
	for i = #lines, -1, 1 do
		if lines[i] ~= "" then
			break
		end
		lines[i] = nil
	end
end

-- Same defect without the redundant `1` step: the default step is +1.
local function reversed(lines: { string }): { string }
	local out = {}
	--~ ERROR[R0605]: `for` from `#lines` down to -1 never runs
	for i = #lines, -1 do
		table.insert(out, lines[i])
	end
	return out
end

return table.freeze({ dropBlankTail = dropBlankTail, reversed = reversed })
