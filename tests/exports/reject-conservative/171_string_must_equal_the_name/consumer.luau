--!rime
--!no-exec
-- The static consumer that makes main.luau "required".
local Mod = require(script.Parent.main)

print(Mod.used, Mod.notes, Mod.other)
