--!rime
--!no-exec
--~ OPTION[R0312]: on
-- A built-in list names libraries and types, not each function in them,
-- so a function newer than the definitions this checker was built with
-- is allowed like the rest of its library.

local Checks = {}

Checks.Missing = math.isnan(0 / 0)
Checks.Endless = math.isinf(math.huge)
Checks.Usable = math.isfinite(1)
Checks.Midway = vector.lerp(vector.create(0, 0, 0), vector.create(2, 2, 2), 0.5)

local scratch = buffer.create(4)
buffer.writebits(scratch, 0, 4, 9)
Checks.Nibble = buffer.readbits(scratch, 0, 4)

return Checks
