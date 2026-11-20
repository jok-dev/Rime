--!rime
-- `b` is only read through main.luau's nested require — invisible to
-- per-binding attribution, but visible to the degrade-mode name-set.
local dep = {
	a = 1,
	b = 2,
}
return table.freeze(dep)
