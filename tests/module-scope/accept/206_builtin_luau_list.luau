--!rime
--~ OPTION[R0312]: on
-- The built-in `luau` list, in use by default: the Luau library that works
-- on the values it is given, and the bare builtins of the same kind. A
-- module may build its constants with them as it loads.

local QUARTER = math.rad(90)
local LIMIT = math.max(4, math.floor(8.5))
local TITLE = string.format("%d of %d", 1, LIMIT)
local PARTS = string.split("first,second,third", ",")
local MASK = bit32.bor(bit32.lshift(1, 4), 3)
local LENGTH = utf8.len(TITLE)
local SCRATCH = buffer.create(8)
local KIND = typeof(PARTS) == type(PARTS) and tostring(MASK) or tonumber("0")

local Defaults = setmetatable({}, { __index = { limit = LIMIT } })
local Sorted = table.clone(PARTS)
table.sort(Sorted)

return table.freeze({
	Quarter = QUARTER,
	Title = TITLE,
	Length = LENGTH,
	Scratch = SCRATCH,
	Kind = KIND,
	Defaults = Defaults,
	Sorted = Sorted,
	First = select(1, unpack(Sorted)),
})
