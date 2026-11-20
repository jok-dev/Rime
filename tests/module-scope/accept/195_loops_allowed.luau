--!rime
--~ OPTION[R0312]: allow-loops = true
-- With loops allowed, a loop that calls nothing is fine.

local Squares = {}
for index = 1, 8 do
	Squares[index] = index * index
end

local total = 0
for _, square in Squares do
	total += square
end
Squares.Total = total

return Squares
