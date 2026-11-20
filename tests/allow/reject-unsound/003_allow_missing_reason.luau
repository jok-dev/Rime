--!rime
-- An allow without a reason suppresses NOTHING: the original diagnostic
-- stays, and the directive itself gets R0021 explaining what is missing.
-- Reasons are mandatory by design — an unexplained suppression is a future
-- mystery.
local function tally(items: { number }): number
	local count = 0
	for _, n in ipairs(items) do
		local count = n -- rime: allow(shadowing)
		--~ ERROR[R0011]: `count` shadows an earlier declaration
		--~ WARNING[R0021]: has no reason
		print(count)
	end
	return count
end

return table.freeze({ tally = tally })
