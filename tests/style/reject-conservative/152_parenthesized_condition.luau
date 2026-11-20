--!rime
-- A condition wholly inside parentheses (R0706, Selene's
-- `parenthese_conditions`).
--
-- WHY REJECTED: `if`, `elseif`, `while` and `until` take a bare
-- expression. Parentheses around all of it change nothing.
-- WHAT WOULD FIX IT: remove them (each report carries the edit).
local function classify(health: number, shielded: boolean): string
	--~ WARNING[R0706]: parentheses around the whole `if` condition
	if (health <= 0) then
		return "down"
	--~ WARNING[R0706]: parentheses around the whole `elseif` condition
	elseif ((shielded)) then
		return "shielded"
	end
	return "up"
end

local function drain(queue: { number }): number
	local total = 0
	--~ WARNING[R0706]: parentheses around the whole `while` condition
	while (#queue > 0) do
		total += table.remove(queue) :: number
	end
	repeat
		total -= 1
	--~ WARNING[R0706]: parentheses around the whole `until` condition
	until (total < 10)
	return total
end

return table.freeze({ classify = classify, drain = drain })