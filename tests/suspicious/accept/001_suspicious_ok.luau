--!rime
-- Near-misses of the suspicious family (R0601–R0608) that must stay silent.
local function swap(a: number, b: number): (number, number)
	a, b = b, a -- the real one-statement swap idiom
	return a, b
end

local function scale(n: number, d: number): number
	return n / d -- divisor is not a literal zero
end

local function classify(n: number): string
	if n > 10 then
		return "big"
	elseif n > 5 then -- different condition, different branch
		return "mid"
	end
	return "small"
end

local function wordStart(prev: string, ch: string): boolean
	-- Multi-condition dispatch: two CONDITIONAL arms sharing a body is the
	-- deliberate "any of these cases → do X" idiom, not an R0603 slip.
	if prev == "_" then
		return true
	elseif prev == "-" then
		return true
	elseif ch == "" then
		return false
	end
	return false
end

local function reversed(items: { number }): number
	local total = 0
	for i = #items, 1, -1 do -- explicit -1 step: a real reverse walk
		total += items[i]
	end
	return total
end

local function unpackPair(pair: { number }): (number, number)
	local first, second = table.unpack(pair) -- call tail: multi-value, silent
	return first, second
end

-- Numeric ranges R0605/R0609 must leave alone (widening 2026-09-04).
local function ranges(items: { number }, n: number): number
	local total = 0
	for i = 1, #items do -- forward walk
		total += items[i]
	end
	for i = 8, 1, -1 do -- constant countdown with its -1
		total += i
	end
	for i = 1, 8, 2 do -- positive step, forward range
		total += i
	end
	for i = #items, n do -- non-constant end: unknown, silent
		total += i
	end
	for i = #items, 5 do -- end above 1: a fill-up loop, silent
		total += i
	end
	for i = 1, 8, -n do -- non-constant step: silent
		total += i
	end
	for i = 0, #items do -- zero start is Luau's ForRange business, not ours
		total += i
	end
	return total
end

return table.freeze({
	swap = swap,
	scale = scale,
	classify = classify,
	wordStart = wordStart,
	reversed = reversed,
	unpackPair = unpackPair,
	ranges = ranges,
})
