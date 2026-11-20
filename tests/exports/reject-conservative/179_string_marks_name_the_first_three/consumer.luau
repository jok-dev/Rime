--!rime
--!no-exec
-- The static consumer that makes main.luau "required". It reads `used`,
-- `order` and `first`; `tick` appears only as a string.
local Mod = require(script.Parent.main)

local function schedule(_name: string) end

print(Mod.used, Mod.order, Mod.first)
schedule("tick")
schedule("tick")
schedule("tick")
