--!rime
--!no-exec
-- An export whose type is given by a cast (`:: T`) rather than an
-- annotation, where T has a string indexer. None is membership-checked.
local Dep = require(script.Parent.dep)
local Returned = require(script.Parent.returned)
local Frozen = require(script.Parent.frozen)

print(Dep.known, Dep.filledLater)
print(Returned.known, Returned.filledLater)
print(Frozen.known, Frozen.filledLater)

return table.freeze({})
