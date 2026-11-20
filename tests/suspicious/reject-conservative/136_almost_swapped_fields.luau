--!rime
-- Almost-swapped on fields and index expressions (R0601, selene parity
-- `almost_swapped`, 2026-11-20).
--
-- WHY REJECTED: `t[i] = t[j]; t[j] = t[i]` reads as a swap of two slots,
-- but after the first assignment slot `i` already holds slot `j`'s value,
-- so the second line writes slot `j` back to itself and the old `t[i]` is
-- gone. The same slip with two fields.
-- WHAT WOULD FIX IT: swap in one statement: `t[i], t[j] = t[j], t[i]`.
local function swapSlots(t: { number }, i: number, j: number)
	t[i] = t[j]
	--~ ERROR[R0601]: `t[i]` and `t[j]` do not swap
	t[j] = t[i]
end

local function flip(box: { left: number, right: number })
	box.left = box.right
	--~ ERROR[R0601]: `box.left` and `box.right` do not swap
	box.right = box.left
end

return table.freeze({ swapSlots = swapSlots, flip = flip })
