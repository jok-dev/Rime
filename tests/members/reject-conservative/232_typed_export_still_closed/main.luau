--!rime
--!no-exec
-- A type annotation alone does not open an export: only a string indexer
-- on the export's own declared type does.
local Dep = require(script.Parent.dep)
local List = require(script.Parent.list)
local Inner = require(script.Parent.inner)

print(Dep.maxLevel, List[1], Inner.byName)

print(Dep.maxLevl)
--~ ERROR[R0307]: `maxLevl` is not a member of module `dep`
print(List.first)
--~ ERROR[R0307]: `first` is not a member of module `list`
print(Inner.byNam)
--~ ERROR[R0307]: `byNam` is not a member of module `inner`

return table.freeze({})
