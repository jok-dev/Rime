--!rime
-- A rule that matches `local d = {}` matches `const d = {}` too: the two
-- are one statement to the checker. Here it is the manual shallow copy
-- (R0704), and its advice keeps the keyword the code used — it says
-- `const copy = table.clone(...)`, not `local copy`.
--
-- WHY REJECTED: a fresh `{}` plus a pairs loop copying every entry is
-- `table.clone` spelled out.
-- WHAT WOULD FIX IT: `const copy = table.clone(source)`.
local function snapshot(source: { [string]: number }): { [string]: number }
	--~ WARNING[R0704]: manual shallow copy
	const copy = {}
	for key, value in pairs(source) do
		copy[key] = value
	end
	return copy
end

return table.freeze({ snapshot = snapshot })
