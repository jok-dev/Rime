--!rime
-- A broken require whose LAST SEGMENT names a module that does exist —
-- somewhere else in the project. The dominant rot shape (the require
-- names the folder the module used to be in, and the module now sits in
-- another one): the module moved, the require did not. With
-- exactly ONE candidate, the diagnostic names it and carries a real
-- replacement — Rime's second auto-fix.
local Maid = require(script.Parent.Old.Maid)
--~ ERROR[R0501]: cannot resolve this require

local function get()
	return Maid
end

return table.freeze({ get = get })
