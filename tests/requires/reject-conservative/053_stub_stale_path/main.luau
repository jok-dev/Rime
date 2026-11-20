--!rime
-- A broken stub whose variable IS live-required elsewhere — but at a
-- DIFFERENT path. The module moved; the live require was updated; the
-- stub was not. The stub is stale documentation and dead type info —
-- error on the stub, and point at the live path.
--
-- WHY CONSERVATIVE: same caveat as 046 — a loader can clone instances in
-- at runtime; Rime cannot see that.
local Vouchers = false and require(script.Parent.Archive.Vouchers)
--~ ERROR[R0501]: cannot resolve this require

local function give(player: string, voucherId: string): boolean
	if not Vouchers then
		Vouchers = require(script.Parent.Vouchers)
	end
	return Vouchers.give(player, voucherId)
end

return table.freeze({ give = give })
