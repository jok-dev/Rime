--!rime
--!no-exec
-- Reads and calls through an export typed with a string indexer are not
-- membership-checked: `filledLater` may well be there.
local Dep = require(script.Parent.dep)

print(Dep.known)
print(Dep.filledLater)

return table.freeze({})
