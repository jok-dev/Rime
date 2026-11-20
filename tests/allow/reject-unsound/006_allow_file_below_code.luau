--!rime
-- A file-level allow buried below code suppresses NOTHING: suppressions
-- must be discoverable at the top of the file. The original diagnostic
-- stays and the directive gets R0021 saying where it belongs.
local function tally(items: { number }): number
	local count = 0
	-- rime: allow-file(shadowing, "too late down here")
	--~ WARNING[R0021]: must appear before any code
	for _, n in ipairs(items) do
		local count = n
		--~ ERROR[R0011]: `count` shadows an earlier declaration
		print(count)
	end
	return count
end

return table.freeze({ tally = tally })
