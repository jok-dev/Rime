--!rime
-- The unused-* rules stay silent here: every binding is read somewhere —
-- including reads that happen only inside a nested closure (capture
-- machinery counts cross-body reads) — or is a deliberate `_`-prefixed
-- discard.
local function sum(items: { number }): number
	local total = 0
	for _i, n in ipairs(items) do
		total += n
	end
	return total
end

-- `count` and `start` are read only by the returned closure: capture is use.
local function makeCounter(start: number): () -> number
	local count = start
	return function(): number
		count += 1
		return count
	end
end

-- Param read only inside a nested closure: capture is use for params too.
local function delayGreet(name: string): () -> string
	return function(): string
		return "hi " .. name
	end
end

-- `t` is read as the base of a table write (`t.name = ...` reads `t`).
local function tag(t: { name: string }, name: string)
	t.name = name
end

local record = { name = "a" }
tag(record, "b")

local counter = makeCounter(0)
counter()

local greeter = delayGreet("bob")
greeter()

return table.freeze({ sum = sum, value = sum({ 1, 2, 3 }) })
