--!rime
--!no-exec
-- Per-player table with weak keys and no teardown (R0803). The leak this
-- stands for: dead Player objects are retained somewhere else, so
-- `__mode = "k"` never releases a single entry, and each entry pins the
-- last character model that player had.
--
-- WHY REJECTED: entries are added per Player and nothing in this module
-- ever removes one. Weak keys are not a teardown — the table does not hold
-- the Player, but it holds the VALUE exactly as long as the Player object
-- is reachable from ANYWHERE. R0803, not R0802: the strong-key rule is for
-- tables that hold the Player itself.
-- WHAT WOULD FIX IT: `lastCharacter[player] = nil` from a
-- `Players.PlayerRemoving` handler.
local Players = game:GetService("Players")

--~ WARNING[R0803]: `lastCharacter` is weak-keyed but holds its values for every Player ever added, and is never cleared in this module
local lastCharacter: { [Player]: Model } = setmetatable({}, { __mode = "k" })

Players.PlayerAdded:Connect(function(player: Player)
	player.CharacterAdded:Connect(function(character: Model)
		lastCharacter[player] = character
	end)
end)

local function lastCharacterOf(player: Player): Model?
	return lastCharacter[player]
end

return table.freeze({ lastCharacterOf = lastCharacterOf })
