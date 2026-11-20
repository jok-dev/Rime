--!rime
-- Comparison with a table constructor (R0607, selene parity 2026-08-21).
--
-- WHY REJECTED: `==` on tables compares identity, and a just-constructed
-- table is never the same object as an existing one — the comparison is
-- constant. The common intent, "is this table empty", is `next(t) == nil`.
-- WHAT WOULD FIX IT: `next(t) == nil` for emptiness, field comparison for
-- contents.
local function isEmpty(t: { number }): boolean
	--~ ERROR[R0607]: comparison with a table constructor
	return t == {}
end

return table.freeze({ isEmpty = isEmpty })
