--!rime
--!no-exec
-- A require that is not a top-level binding (the loader idiom) puts the
-- project in degrade mode: member names read ANYWHERE count as used on
-- every module. `load().b` is such a read — dep.luau's `b` stays silent
-- because of it, not because the rule turned off.
local function load()
	return require(script.Parent.dep)
end

print(load().b)
