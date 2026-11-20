--!rime
--~ OPTION[R0312]: allow = *.new
--~ OPTION[R0312]: deny = math.floor, Random.new, game:GetService
-- `deny` takes single callees back out of what is allowed: out of a
-- built-in list (`math.floor`), out of the project's own pattern
-- (`Random.new` matches `*.new`), and out of the two calls that are
-- allowed without a list.

local Storage = game:GetService("ReplicatedStorage")
--~ WARNING[R0312]: `game:GetService` is called at module scope

local Signal = require(Storage.Shared.Signal)

local Dice = {}
Dice.Rolled = Signal.new()
Dice.Sides = math.max(6, 1)

Dice.Half = math.floor(Dice.Sides / 2)
--~ WARNING[R0312]: `math.floor` is called at module scope

Dice.Source = Random.new()
--~ WARNING[R0312]: `Random.new` is called at module scope

return Dice
