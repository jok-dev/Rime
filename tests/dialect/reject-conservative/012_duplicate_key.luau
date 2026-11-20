--!rime
-- Duplicate table-constructor keys rejected (decided 2026-08-21).
--
-- WHY REJECTED: valid Luau — the last value silently wins — but the earlier
-- entry is dead on arrival, and Rime cannot tell a stale edit from intent.
-- WHAT WOULD FIX IT: delete one of the entries, or assign the slot after
-- construction. Downgrade with `duplicate-table-key = "warn"` if migrating.
local config = {
	speed = 10,
	drag = 0.5,
	speed = 20,
	--~ ERROR[R0012]: duplicate key `speed` in table constructor
}

-- A bracket key with a string literal names the same slot as the sugar form.
local mixed = {
	["mode"] = "race",
	mode = "chase",
	--~ ERROR[R0012]: duplicate key `mode` in table constructor
}

return table.freeze({ config = config, mixed = mixed })
