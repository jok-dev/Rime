--!rime
--~ OPTION[R0312]: on
-- The built-in lists are for calls that give the same answer every time a
-- module loads. A random number is not that, and seeding changes the
-- generator every script shares. `Random.new` only makes a generator; a
-- project that wants any of the others says so in `allow`.

local Dice = {}

Dice.First = math.random(1, 6)
--~ WARNING[R0312]: `math.random` is called at module scope

math.randomseed(42)
--~ WARNING[R0312]: `math.randomseed` is called at module scope

Dice.Tint = BrickColor.random()
--~ WARNING[R0312]: `BrickColor.random` is called at module scope

Dice.Source = Random.new(42)
Dice.Sides = math.max(6, 1)

return Dice
