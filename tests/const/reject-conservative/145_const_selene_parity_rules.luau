--!rime
-- The Selene-parity rules read `const` as they read `local` (checked when
-- deps/full-moon-upgrade met feat/selene-parity-lints, 2026-11-20):
--   * a const table that is only written to is R0013, as corpus 143;
--   * fields of a const table that do not swap are R0601, as corpus 136;
--   * more values than names, ending in a call, is R0608, as corpus 140.
--     (The parser itself refuses every other count mismatch on a const,
--     so that is the one shape R0608 can meet.)
--
-- WHY REJECTED: each is the mistake it would be with `local`.
-- WHAT WOULD FIX IT: return or use the table; swap in one statement; drop
-- the extra value.
local function read(): (number, number)
	return 1, 2
end

local function tally(scores: { number }): number
	const totals = {}
	--~ WARNING[R0013]: `totals` is written to, but never read
	local sum = 0
	for index, score in scores do
		sum += score
		totals[index] = sum
	end
	return sum
end

local function flip(): { left: number, right: number }
	const pair = { left = 1, right = 2 }
	pair.left = pair.right
	pair.right = pair.left
	--~ ERROR[R0601]: `pair.left` and `pair.right` do not swap
	return pair
end

local function spare(): number
	const first, second = 1, 2, read()
	--~ ERROR[R0608]: 2 names but 3 values
	return first + second
end

return table.freeze({ tally = tally, flip = flip, spare = spare })
