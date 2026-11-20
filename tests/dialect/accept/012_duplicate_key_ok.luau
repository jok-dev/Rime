--!rime
-- Distinct keys stay legal: sugar and bracket forms naming different slots,
-- and the same name reused one constructor level down.
local a = {
	speed = 10,
	drag = 0.5,
	["mode"] = "race",
	nested = { speed = 20 },
}

return table.freeze({ a = a })
