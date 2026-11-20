--!rime
-- Calls R0708 leaves alone: the result is used, the call is a method or
-- goes through a local, or it does its work through an argument.
local function tally(text: string): number
	local count = 0
	-- The function is called once per match: the call is there for that.
	string.gsub(text, "%a+", function()
		count += 1
	end)
	local longest = math.max(count, 1)
	-- A method call, and a call on a call's result.
	text:rep(2)
	tostring(count):len()
	return longest
end

-- A local named like a library is not the library.
local function scale(math: { max: (number, number) -> () }): number
	math.max(1, 2)
	return 1
end

return table.freeze({ tally = tally("a bc def"), scale = scale({ max = function() end }) })