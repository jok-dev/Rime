--!rime
-- Case 30 (plan §9): `cooldown` is declared in the type but never assigned in
-- the constructor, so it is nil at first read. The C# analogue is definite
-- assignment of `readonly` fields before the constructor returns.
--
-- NOT IMPLEMENTED YET (R0104 needs the signature pass): stays red as spec.
type Weapon = {
	damage: number,
	cooldown: number,
}

local Weapon = {}
Weapon.__index = Weapon

local function new(damage: number): Weapon
	local self = setmetatable({}, Weapon)
	self.damage = damage
	return self
	--~ ERROR[R0104]: constructor does not assign `cooldown`
end

return table.freeze({ new = new })
