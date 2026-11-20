--!rime
--!no-exec
-- The only consumer, and it uses `run` alone.
local Dep = require(script.Parent.dep)

print(Dep:run())

return table.freeze({})
