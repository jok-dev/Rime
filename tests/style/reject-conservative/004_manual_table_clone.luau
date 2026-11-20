--!rime
-- Manual shallow copy (R0704, selene parity 2026-08-21).
--
-- WHY REJECTED: `local d = {}` plus a pairs loop copying every entry is
-- `table.clone` spelled out — slower and noisier. The fresh-`{}` start is
-- what makes it a clone; copying into a table with existing entries is a
-- merge, and stays silent.
-- WHAT WOULD FIX IT: `local copy = table.clone(source)`.
local function snapshot(source: { [string]: number }): { [string]: number }
	--~ WARNING[R0704]: manual shallow copy
	local copy = {}
	for key, value in pairs(source) do
		copy[key] = value
	end
	return copy
end

return table.freeze({ snapshot = snapshot })
