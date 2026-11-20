--!rime
-- Call conformance: missing argument. Luau fills the hole with nil
-- silently and the crash happens inside the callee, blaming the wrong line.
local function applyDamage(target: { health: number }, amount: number): ()
	target.health -= amount
end

local enemy = { health = 100 }
applyDamage(enemy)
--~ ERROR[R0301]: call to `applyDamage` is missing argument 2 `amount`

return table.freeze({ applyDamage = applyDamage })
