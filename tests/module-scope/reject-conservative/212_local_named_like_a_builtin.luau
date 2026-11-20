--!rime
--~ OPTION[R0312]: on
-- The built-in lists name globals. A local of the same name is the
-- project's own code, and a call through it is reported; `allow` is where
-- a project says its own is fine.

local Fixed = { floor = function(value: number): number
	return value - value % 1
end }

local math = Fixed

local whole = math.floor(2.5)
--~ WARNING[R0312]: `math.floor` is called at module scope
--~ NOTE: `math` is a local here

local text = string.rep("-", whole)

return { whole = whole, text = text }
