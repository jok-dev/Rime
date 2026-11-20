--!rime
-- A loop-variable require whose iterator is NOT a children enumeration
-- (a plain table) stays R0502: the shape exception is exactly
-- folder:GetChildren()/GetDescendants(), nothing looser.
local list = {}
local main = {}

for _, m in ipairs(list) do
	main[m] = require(m)
	--~ ERROR[R0502]: require target is not statically resolvable
end

return table.freeze(main)
