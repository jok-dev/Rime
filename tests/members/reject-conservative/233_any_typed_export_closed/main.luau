--!rime
--!no-exec
-- An export typed `any`, by annotation or by cast, is still closed: its
-- members are the ones its file defines.
local Dep = require(script.Parent.dep)
local Cast = require(script.Parent.cast)
local Returned = require(script.Parent.returned)
local Alias = require(script.Parent.alias)

print(Dep.known, Cast.known(), Returned.known, Alias.known)

print(Dep.filledLater)
--~ ERROR[R0307]: `filledLater` is not a member of module `dep`
Cast.filledLater()
--~ ERROR[R0306]: `filledLater` is not a member of module `cast`
print(Returned.filledLater)
--~ ERROR[R0307]: `filledLater` is not a member of module `returned`
print(Alias.filledLater)
--~ ERROR[R0307]: `filledLater` is not a member of module `alias`

return table.freeze({})
