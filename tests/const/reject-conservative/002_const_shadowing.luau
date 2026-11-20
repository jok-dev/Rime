--!rime
-- Shadowing is judged by name, not by keyword: a `const` that redeclares a
-- name still in scope shadows it (R0011), and so does a `local` or a
-- `const function` that redeclares a `const`.
--
-- WHY REJECTED: valid Luau — the inner binding simply wins — but later
-- reads silently pick it, and Rime cannot tell a deliberate rebind from a
-- mistake.
-- WHAT WOULD FIX IT: rename one of them.
local function tally(items: { number }): number
	local count = 0
	for _, n in ipairs(items) do
		const count = n
		--~ ERROR[R0011]: `count` shadows an earlier declaration
		print(count)
	end
	return count
end

local function scale(n: number): number
	const factor = 2
	if n > 10 then
		local factor = 3
		--~ ERROR[R0011]: `factor` shadows an earlier declaration
		return n * factor
	end
	return n * factor
end

local function wrap(n: number): number
	local helper = n
	do
		const function helper(): number
		--~ ERROR[R0011]: `helper` shadows an earlier declaration
			return 1
		end
		print(helper())
	end
	return helper
end

return table.freeze({ tally = tally, scale = scale, wrap = wrap })
