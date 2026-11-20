--!rime
-- The type-guard clause in its negated spelling: falling past
-- `if type(x) ~= "number" then return end` means the condition was false,
-- so `type(x) == "number"` — which excludes nil. The `==` spelling always
-- narrowed; this pins the `~=` form's false branch.
local function clamp(x: number?): number
	if type(x) ~= "number" then
		return 0
	end
	return x + 1
end

return table.freeze({ clamp = clamp(5) ~= 0 and clamp(nil) == 0 })
