--!rime
--!no-exec
-- The static consumer that makes main.luau "required". It reads `used` and
-- `hooks`; `reset` appears only as a string.
local Mod = require(script.Parent.main)

local function schedule(_name: string) end

print(Mod.used, Mod.hooks)
schedule("reset")
