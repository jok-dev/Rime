--!rime
--!no-exec
-- A PlayerRemoving handler exists but forgets one of the module's
-- per-player maps (R0802). The shape is a common one: cleanup code that
-- reaches some tables and not others.
--
-- WHY REJECTED: `scores` is cleared on leave; `lastSaved` never is.
-- WHAT WOULD FIX IT: `lastSaved[player] = nil` in the same handler.
local Players = game:GetService("Players")

local scores: { [Player]: number } = {}
--~ WARNING[R0802]: `lastSaved` holds every Player ever added and is never cleared in this module
local lastSaved = {}

local function save(player: Player, score: number): boolean
	if scores[player] == score then
		return false
	end
	scores[player] = score
	lastSaved[player] = os.time()
	return true
end

Players.PlayerRemoving:Connect(function(player)
	scores[player] = nil
end)

return table.freeze({ save = save })
