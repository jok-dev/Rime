--!rime
--!no-exec
-- The static consumer that makes main.luau "required": only `used` is read.
local Mod = require(script.Parent.main)

print(Mod.used)
