--!rime
--!no-exec
-- `local require = require(X)`: the right-hand `require` is still the
-- builtin, so a broken path there is reported. The calls after it go
-- through the rebound name and are left alone.
--
-- WHY CONSERVATIVE: same as 046 - a loader can put the instance there at
-- runtime, and Rime cannot see that.
local require = require(script.Parent.Ghost)
--~ ERROR[R0501]: cannot resolve this require

local Alpha = require("Alpha")

return table.freeze({ Alpha = Alpha })
