--!rime
-- A typo'd rule name must never silently turn a suppression inert: the
-- original diagnostic stays and the directive gets R0021 naming the unknown
-- rule.
local function tally(items: { number }): number
	local count = 0
	for _, n in ipairs(items) do
		local count = n -- rime: allow(shadwoing, "typo in the rule name")
		--~ ERROR[R0011]: `count` shadows an earlier declaration
		--~ WARNING[R0021]: unknown rule
		print(count)
	end
	return count
end

return table.freeze({ tally = tally })
