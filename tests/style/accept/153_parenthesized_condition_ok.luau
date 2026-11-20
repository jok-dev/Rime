--!rime
-- Parentheses that R0706 leaves alone: they hold only part of the
-- condition, group an operand, or sit in an `if` expression (which
-- Selene's `parenthese_conditions` does not check either).
local function pick(a: number?, b: number?, call: (number) -> boolean): number
	if (a) and (b) then
		return a + b
	elseif not (a) then
		return 0
	end
	while (a or 0) > 10 do
		a = (a or 0) - 10
	end
	if (call)(a or 0) then
		return 1
	end
	local chosen = if (a) then a else 2
	repeat
		chosen += 1
	until (chosen) > 3
	return chosen
end

return table.freeze({ picked = pick(1, nil, function(n)
	return n > 5
end) })