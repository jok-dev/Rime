--!rime
--!no-exec
-- The static consumer that makes dep.luau "required": only `a` is read
-- here; `b`'s silence comes from main.luau's name-set read.
local Mod = require(script.Parent.dep)

print(Mod.a)
