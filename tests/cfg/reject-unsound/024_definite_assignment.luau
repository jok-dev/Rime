--!rime
-- Case 24 (plan §9): `label` is unassigned when n == 0, so the read yields nil.
local function classify(n: number): string
	local label: string
	if n > 0 then
		label = "positive"
	elseif n < 0 then
		label = "negative"
	end
	return label
	--~ ERROR[R0101]: `label` may be unassigned here
	--~ NOTE: no value is assigned when the condition on line 5 is false
end

return table.freeze({ classify = classify })
