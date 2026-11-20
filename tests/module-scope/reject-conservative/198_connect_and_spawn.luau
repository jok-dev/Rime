--!rime
--~ OPTION[R0312]: on
-- The work the rule is for: a module that connects a signal, starts a
-- thread and waits for an instance as a side effect of being required.

local Players = game:GetService("Players")
local Storage = game:GetService("ReplicatedStorage")

local Tracker = {}
local seen: { [number]: boolean } = {}

local Assets = Storage:WaitForChild("Assets")
--~ WARNING[R0312]: `Storage:WaitForChild` is called at module scope

Players.PlayerAdded:Connect(function(player)
--~ WARNING[R0312]: `Players.PlayerAdded:Connect` is called at module scope
	seen[player.UserId] = true
end)

task.spawn(function()
--~ WARNING[R0312]: `task.spawn` is called at module scope
	while true do
		task.wait(60)
		table.clear(seen)
	end
end)

function Tracker.Seen(userId: number): boolean
	return seen[userId] == true and Assets ~= nil
end

return Tracker
