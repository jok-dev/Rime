--!rime
-- The registry idiom: enumerate a folder's ModuleScript children and
-- require each. The target is a loop variable — unresolvable per-file —
-- but the SHAPE is the systematically-deliberate loader pattern, so it
-- gets its own code R0508, wholesale
-- downgradeable, instead of drowning in per-site R0502 allows.
local main = {}

for _, child in ipairs(script.Parent.Effects:GetChildren()) do
	main[child.Name] = require(child)
	--~ ERROR[R0508]: require target is a loop variable over a folder's children
end

return table.freeze(main)
