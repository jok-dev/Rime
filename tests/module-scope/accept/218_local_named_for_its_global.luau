--!rime
--~ OPTION[R0312]: on
-- `local math = math` at the top of a file is the same library under the
-- same name. As long as the local is never given another value, calls
-- through it are the built-in ones.

local math = math
local string = string
local table = table
local tostring = tostring

local LIMIT = math.max(4, math.floor(8.5))
local TITLE = string.format("%d", LIMIT)
local PARTS = table.freeze({ TITLE, tostring(LIMIT) })

return PARTS
