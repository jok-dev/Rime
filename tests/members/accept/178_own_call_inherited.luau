--!rime
-- The export has a metatable, so a member it does not define itself can
-- come through `__index`. `main.describe()` is `base.describe`: no R0306.
local base = {}

function base.describe()
	return "base"
end

local main = {}
setmetatable(main, { __index = base })

function main.run()
	return main.describe()
end

return main
