--!rime
--!no-exec
-- The members `dep` sets through `self` exist for a consumer as much as
-- the ones it writes as `dep.x = ...`: no R0306 for the calls, no R0307
-- for the reads.
local Dep = require(script.Parent.dep)

Dep:init()
Dep:name()
print(Dep.helper(), Dep:helper(), Dep.cache, Dep.label)

return table.freeze({})
