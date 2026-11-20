--!rime
-- Almost-swapped (R0601, selene parity 2026-08-21).
--
-- WHY REJECTED: `a = b; b = a` reads as a swap, but after the first
-- assignment `a` already holds `b`'s value — the second line assigns `b`
-- its own value back, and `a`'s original value is gone.
-- WHAT WOULD FIX IT: swap in one statement: `a, b = b, a`.
local function reorder(a: number, b: number): (number, number)
	a = b
	--~ ERROR[R0601]: `a` and `b` do not swap
	b = a
	return a, b
end

return table.freeze({ reorder = reorder })
