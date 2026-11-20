--!rime
--!no-exec
-- Per-player tables R0802 must leave alone: every teardown shape it
-- recognises, plus tables that are not module-lifetime or not Player-keyed.
local Players = game:GetService("Players")

-- (a) cleared on leave
local sessions: { [Player]: number } = {}

-- (b) cleared through a helper whose value parameter admits nil: the
--     leave path calls `assign(player, nil)`
local teams: { [Player]: { string } } = {}
local function assign(player: Player, members: { string }?)
	teams[player] = members
end

-- (c) weak VALUES do release: once the value is collected the entry goes
local backpacks: { [Player]: Instance } = setmetatable({}, { __mode = "v" })

-- (d) wholesale clears
local debounces = {}
local pings = {}

-- (e) handed to a janitor: something else owns its lifetime
local janitor = { tables = {} }
local function track(t)
	table.insert(janitor.tables, t)
end
local tracked = {}
track(tracked)

-- (f) not keyed by Player
local byName: { [string]: number } = {}

-- (h) `flag or nil`: storing false removes the entry
local muted: { [Player]: boolean } = {}

local function touch(player: Player, name: string)
	sessions[player] = os.clock()
	assign(player, { name })
	backpacks[player] = player:FindFirstChildOfClass("Backpack")
	debounces[player] = true
	pings[player] = 0
	tracked[player] = true
	byName[name] = 1
	muted[player] = (#name > 20) or nil
end

local function reset()
	table.clear(debounces)
	pings = {}
end

Players.PlayerRemoving:Connect(function(player)
	sessions[player] = nil
	assign(player, nil)
	reset()
end)

-- (g) a function-local map lives only as long as the call
local function perCall(players: { Player }): number
	local order: { [Player]: number } = {}
	for index = #players, 1, -1 do
		order[players[index]] = index
	end
	return order[players[1]] or 0
end

return table.freeze({ touch = touch, perCall = perCall })
