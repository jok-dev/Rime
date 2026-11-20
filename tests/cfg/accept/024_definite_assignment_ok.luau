--!rime
-- Case 25 (plan §9): every path assigns before the read.
local function classify(n: number): string
	local label: string
	if n > 0 then
		label = "positive"
	elseif n < 0 then
		label = "negative"
	else
		label = "zero"
	end
	return label
end

return table.freeze({ classify = classify })
