--!rime
--~ OPTION[R0312]: allow = *.new
-- `allow` adds to what is already allowed: `require`, `game:GetService`
-- and the built-in lists stay, and the project's `*.new` joins them.
-- Anything on none of them is still reported.

local Storage = game:GetService("ReplicatedStorage")

local Shapes = require(Storage.Shared.Shapes)
local Signal = require(Storage.Shared.Signal)

local Board = {}
Board.Changed = Signal.new()
Board.Origin = Vector3.new(0, 0, 0)
Board.Width = math.max(8, 16)
Board.Shape = Shapes.create("square")
--~ WARNING[R0312]: `Shapes.create` is called at module scope

return Board
