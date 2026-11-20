--!rime
--!no-exec
--~ OPTION[R0312]: allow-builtin = none
-- `require` and `game:GetService` are allowed without being on any list:
-- naming what a module depends on is the one thing nearly every module
-- does as it loads. Switching the built-in lists off does not change that.

local Shapes = require(game:GetService("ReplicatedStorage").Shared.Shapes)
local Units = require(script.Parent.Units)
local Players = game:GetService("Players")

local function count(): number
	return #Players:GetPlayers() + Shapes.Count() + Units.Count()
end

return { Count = count }
