--!rime
--~ OPTION[R0312]: on
-- A function called where it is written is a call like any other: the
-- wrapper does not make its body something else.

local Table = (function()
--~ WARNING[R0312]: a function is called at module scope where it is written
	local out = {}
	for index = 1, 4 do
		out[index] = index * 2
	end
	return out
end)()

return Table
