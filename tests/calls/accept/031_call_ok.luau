--!rime
-- Call conformance: the shapes that must NOT fire.
local function pick(): (number, number)
	return 1, 2
end

local function sum(a: number, b: number): number
	return a + b
end

-- optional param omitted: nil is in the type
local function label(text: string, prefix: string?): string
	if prefix then
		return prefix .. text
	end
	return text
end

-- variadic callee: any arity goes
local function pack(...: number): number
	return select("#", ...)
end

-- method called correctly with `:`
local Shop = {}
function Shop:buy(itemId: string): ()
	print(itemId)
end

local x: number = 3
local ok = {
	sum(1, 2),
	sum(x, x),
	sum(pick()), -- multi-value tail fills both params: arity check suppressed
	label("hi"),
	label("hi", "["),
	pack(),
	pack(1, 2, 3),
}
Shop:buy("sword")

return table.freeze({ ok = ok })
