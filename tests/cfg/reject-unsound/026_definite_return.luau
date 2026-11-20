--!rime
-- Case 26 (plan §9): falls off the end when the guard fails, returning nil.
local function double(n: number?): number
	if n ~= nil then
		return n * 2
	end
	--~ ERROR[R0102]: not all paths return a value
end

return table.freeze({ double = double })
