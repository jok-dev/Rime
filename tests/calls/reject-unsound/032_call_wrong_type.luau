--!rime
-- Call conformance: argument type mismatch on a literal. The classic
-- source is a TextBox.Text handed to arithmetic.
local function applyDamage(target: { health: number }, amount: number): ()
	target.health -= amount
end

local enemy = { health = 100 }
applyDamage(enemy, "50")
--~ ERROR[R0302]: argument 2 of `applyDamage` expects `number`, got `string`

return table.freeze({ applyDamage = applyDamage })
