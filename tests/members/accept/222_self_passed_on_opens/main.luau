--!rime
--!no-exec
-- `Dep.extended` was added by the code `self` was handed to. The
-- interface is open, so the call stays silent.
local Dep = require(script.Parent.dep)

Dep:init()
print(Dep.extended())

return table.freeze({})
