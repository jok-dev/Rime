--!rime
--!no-exec
-- Method form is membership-checked the same way: `rest` is a typo
-- of `reset` on a closed interface.
local Session = require(script.Parent.dep)

Session:rest()
--~ ERROR[R0306]: `rest` is not a member of module `dep`

-- The real member is used, so R0309 (unused export) stays out of this case.
Session:reset()

return table.freeze({})
