--!rime
--!no-exec
-- dep.luau `return script` — the module exports its own INSTANCE (the
-- wally SDK namespace idiom), so its members are child ModuleScripts,
-- unknowable to the interface extractor. The interface must be OPEN:
-- no member read through the binding can miss.
local Sdk = require(script.Parent.dep)

local Serial = Sdk.Modules
print(Serial)
