--!rime
--!no-exec
-- The static consumer that makes main.luau "required": only `a` is read.
local Mod = require(script.Parent.main)

print(Mod.a)
