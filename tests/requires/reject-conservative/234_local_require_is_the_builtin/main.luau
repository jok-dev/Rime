--!rime
--!no-exec
-- `local require = require` gives the builtin a local name (a common
-- speed habit). The name still means the builtin, so every call through
-- it is checked like any other require: a path to nothing is reported, and
-- member checks reach through the ones that resolve.
--
-- WHY CONSERVATIVE: same as 046 - a loader can put the instance there at
-- runtime, and Rime cannot see that.
local require = require

local Lib = require(script.Parent.lib)
local Gone = require(script.Parent.gone)
--~ ERROR[R0501]: cannot resolve this require

Lib.okk()
--~ ERROR[R0306]: `okk` is not a member of module `lib`

local function later()
	-- A second local of the same kind, in an inner scope.
	local require = require
	return require(script.Parent.alsoGone)
	--~ ERROR[R0501]: cannot resolve this require
end

return table.freeze({ Lib = Lib, Gone = Gone, later = later })
