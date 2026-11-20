--!rime
-- Duplicate number keys and positional collisions (R0012, selene parity
-- `duplicate_keys`, 2026-11-20).
--
-- WHY REJECTED: a positional item fills the slot its position names, so
-- `{ "low", "mid", [2] = "high" }` writes slot 2 twice and "mid" is gone
-- before the table exists. The same holds for a number key written twice,
-- however it is spelled: `[16]` and `[0x10]` are one slot.
-- WHAT WOULD FIX IT: delete one of the entries, or give the key the slot
-- that was meant.
--
-- Each list below also draws R0703 mixed-table (a warning): a positional
-- collision needs a positional item and a key in one constructor.
local gears = {
	"low",
	"mid",
	[2] = "high",
	--~ ERROR[R0012]: duplicate key `2` in table constructor
}

local flags = {
	[16] = "read",
	[32] = "write",
	[0x10] = "execute",
	--~ ERROR[R0012]: duplicate key `16` in table constructor
}

local ranks = {
	[1] = "gold",
	"silver",
	--~ ERROR[R0012]: duplicate key `1` in table constructor
}

return table.freeze({ gears = gears, flags = flags, ranks = ranks })
