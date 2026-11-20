--!rime
--~ OPTION[R0312]: on
-- A loop is one report, at its keyword. What is inside it is not
-- reported one by one, so one allow comment covers the loop.

local Lookup = {}
local names = { "first", "second", "third" }

for index, name in names do
--~ WARNING[R0312]: `for` loop at module scope
	Lookup[name] = tostring(index)
end

local count = 0
while count < 3 do
--~ WARNING[R0312]: `while` loop at module scope
	count += 1
end

repeat
--~ WARNING[R0312]: `repeat` loop at module scope
	count -= 1
until count == 0

return Lookup
