--!rime
-- `...` where Luau allows it (R0022 stays silent):
--   * at the top level of a file, inside its `do` and `if` blocks too
--     (Selene's `undefined_variable` reports those two; it accepts `...`
--     only in the file's outermost scope, and the code is valid);
--   * anywhere in a function that declares `...`, typed or not.
-- A nested function that needs the values takes a capture of them.
local first = ...
local count = select("#", ...)
if first then
	count += #{ ... }
end
do
	local again = ...
	count += if again then 1 else 0
end

local function sum(...: number): number
	local total = 0
	for _, n in { ... } do
		total += n
	end
	if total > 0 then
		return total + select("#", ...)
	end
	return total
end

local function wrap(...)
	local packed = table.pack(...)
	return function()
		return table.unpack(packed, 1, packed.n)
	end
end

return table.freeze({ count = count, sum = sum(1, 2), wrap = wrap })