--!rime
-- R0202 on a loop variable: every iteration rebinds `p`, then indexes it,
-- then asks whether it was nil — the `p == nil` spelling, with `continue`
-- as the guard's exit. Same defect as case 068 in loop clothing.
local function total(parts: { { pos: number } }): number
	local sum = 0
	for _, p in parts do
		local pos = p.pos
		if p == nil then
			--~ ERROR[R0202]: `p` is checked for nil after it was already indexed
			continue
		end
		sum += pos
	end
	return sum
end

return table.freeze({ total = total })
