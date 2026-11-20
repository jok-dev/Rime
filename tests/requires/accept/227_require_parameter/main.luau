--!rime
--!no-exec
-- A parameter named `require` is a local like any other: the call inside
-- the function goes to whatever the caller passed, not to the builtin.
local function load(require: (string) -> string): string
	return require("Alpha")
end

return table.freeze({ load = load })
