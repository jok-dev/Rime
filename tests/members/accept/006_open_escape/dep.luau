--!rime
-- OPEN module: the export escapes into a helper before freeze — the helper
-- may add members the extractor cannot see.
local dep = {}

local function setup(t)
	t.ready = true
end

setup(dep)

function dep.go()
	print("go")
end

return table.freeze(dep)
