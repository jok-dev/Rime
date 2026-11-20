--!rime
-- Near-misses of R0601 on fields and index expressions that must stay
-- silent.
local function swapSlots(t: { number }, i: number, j: number)
	t[i], t[j] = t[j], t[i] -- the real one-statement swap
end

local function swapViaTemp(t: { number }, i: number, j: number)
	local held = t[i]
	t[i] = t[j]
	t[j] = held
end

-- The first write moves where the second one lands: `links[i]` on the
-- second line is the slot of the NEW `i`, so this is not a no-op.
local function advance(links: { number }, i: number): number
	i = links[i]
	links[i] = i
	return i
end

local function shift(t: { number })
	t[1] = t[2]
	t[2] = t[3] -- a different slot on the right
end

return table.freeze({
	swapSlots = swapSlots,
	swapViaTemp = swapViaTemp,
	advance = advance,
	shift = shift,
})
