--!rime
-- Call conformance: the `.`/`:` bug. Dot-calling a method shifts
-- every argument left by one: "sword" lands in self, itemId gets nil.
local Shop = {}

function Shop:buy(itemId: string): ()
	print(itemId)
end

Shop.buy("sword")
--~ ERROR[R0303]: `Shop.buy` is a method — call it with `:`

return table.freeze({ Shop = Shop })
