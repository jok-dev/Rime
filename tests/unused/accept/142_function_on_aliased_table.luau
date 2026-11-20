--!rime
-- A local whose only mention is a function declared on it (R0013 stays
-- silent, as Selene's `unused_variable` does on its `mutating_functions`
-- fixture). `function extended.describe() end` is `extended.describe =
-- function() end`: it writes a field of the table `extended` holds, and
-- the caller, who holds the same table, sees it. The usual shape is
-- `local Base = require(...)` followed by `function Base.extra() end`.
local function attach(target: { [string]: () -> string })
	local extended = target
	function extended.describe(): string
		return "attached"
	end
end

local shared: { [string]: () -> string } = {}
attach(shared)

return table.freeze({ described = shared.describe() })
