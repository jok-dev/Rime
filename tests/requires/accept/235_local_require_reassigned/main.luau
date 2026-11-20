--!rime
--!no-exec
-- `local require = require` starts as the builtin, and the file then
-- stores something else in it. Which calls run before the store is not
-- tracked, so the name is a rebinding: its calls are left alone.
local require = require
local loader = require(script.Parent.loader)

require = loader

local Alpha = require("Alpha")

return table.freeze({ Alpha = Alpha })
