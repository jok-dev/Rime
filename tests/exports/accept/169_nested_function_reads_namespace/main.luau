--!rime
-- `function main.codec.encode()` stores into the table `main.codec` holds,
-- so it reads `codec` the way `main.codec.encode = function() end` does.
-- No consumer touches `codec`, but it is not an unused export: with the
-- `main.codec = {}` line deleted, the declaration below it throws when the
-- module loads.
local main = {}

main.used = 1
main.codec = {}

function main.codec.encode(value: number): number
	return value + 1
end

return main
