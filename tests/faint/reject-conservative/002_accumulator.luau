--!rime
-- Self-feeding accumulator: `total` is read only by its own `+=`, so every
-- store is kept "live" by a read that feeds nothing — invisible to both
-- R0013 (a read exists) and R0104 (every store looks live) until faint
-- analysis. The running-total shape.
local function survey(list: { number }): number
	local total = 0
	local seen = 0
	for _, n in pairs(list) do
		total += n
		seen += 1
	end
	return seen
end

return table.freeze({ survey = survey })
--~ WARNING[R0017]: `total` only feeds itself; its value is never used
