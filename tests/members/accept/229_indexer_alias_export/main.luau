--!rime
--!no-exec
-- Each of these exports has a string indexer somewhere behind its
-- declared type. None is membership-checked.
local Dep = require(script.Parent.dep)
local Generic = require(script.Parent.generic)
local Mixed = require(script.Parent.mixed)

print(Dep.known(), Dep.filledLater())
print(Generic.known, Generic.filledLater)
print(Mixed.name, Mixed.filledLater)

return table.freeze({})
