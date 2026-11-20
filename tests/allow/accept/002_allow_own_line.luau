--!rime
-- Inline allow, own-line form: a directive alone on its line suppresses the
-- NEXT line. The raw R#### code spelling works the same as the rule name.
local function tally(items: { number }): number
	local count = 0
	for _, n in ipairs(items) do
		-- rime: allow(R0011, "deliberate rebind; exercises the own-line form")
		local count = n
		print(count)
	end
	return count
end

return table.freeze({ tally = tally })
