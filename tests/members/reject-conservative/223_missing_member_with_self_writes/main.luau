--!rime
--!no-exec
-- Members set through `self` are known by name, like any other member, so
-- a name the module never defines is still reported: R0306 for a call,
-- R0307 for a read.
local Dep = require(script.Parent.dep)

Dep:init()
Dep.other:fill()
print(Dep.helper(), Dep.other.inner)

Dep.gone()
--~ ERROR[R0306]: `gone` is not a member of module `dep`
print(Dep.inner)
--~ ERROR[R0307]: `inner` is not a member of module `dep`

return table.freeze({})
