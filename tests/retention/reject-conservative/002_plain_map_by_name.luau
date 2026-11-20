--!rime
--!no-exec
-- Plain per-player map with no teardown (R0802): no annotation anywhere,
-- the `player` key name alone is the signal (such a holder is usually
-- written from a function whose `player` parameter has no annotation).
--
-- WHY REJECTED: strong keys — the table itself holds every Player object
-- ever added, which is the keystone shape: one such table anywhere keeps
-- every weak-keyed cache entry for that Player alive too. A PlayerAdded
-- hook fills the map, and no PlayerRemoving hook drains it.
-- WHAT WOULD FIX IT: `lastRequestAt[player] = nil` on PlayerRemoving.
local Players = game:GetService("Players")

--~ WARNING[R0802]: `lastRequestAt` holds every Player ever added and is never cleared in this module
local lastRequestAt = {}

local function throttle(player)
	local last = lastRequestAt[player]
	local now = os.clock()
	lastRequestAt[player] = now
	return last ~= nil and now - last < 1
end

Players.PlayerAdded:Connect(function(player)
	throttle(player)
end)

return table.freeze({ throttle = throttle })
