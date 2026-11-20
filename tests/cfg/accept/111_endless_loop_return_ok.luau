--!rime
-- An endless loop has no exit edge: the only ways out are `return` and
-- `break`. With no `break`, control never reaches the function's `end`, so
-- a declared return type is satisfied by the returns inside the loop.
local function firstAbove(n: number, limit: number): number
	while true do
		n += 1
		if n > limit then
			return n
		end
	end
end

local function firstAboveRepeat(n: number, limit: number): number
	repeat
		n += 1
		if n > limit then
			return n
		end
	until false
end

-- A nested loop's `break` leaves the inner loop only.
local function firstMultiple(of: number): number
	local n = 0
	while true do
		for i = 1, 3 do
			n += 1
			if i == 2 then
				break
			end
		end
		if n % of == 0 then
			return n
		end
	end
end

-- `assert(false, ...)` never returns, like `error(...)`.
local function codeFor(kind: string): number
	if kind == "a" then
		return 1
	elseif kind == "b" then
		return 2
	end
	assert(false, "unknown kind")
end

return table.freeze({
	firstAbove = firstAbove(1, 10),
	firstAboveRepeat = firstAboveRepeat(1, 10),
	firstMultiple = firstMultiple(4),
	codeFor = codeFor("b"),
})
