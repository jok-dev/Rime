--!rime
-- Call conformance: extra arguments. Luau discards them silently —
-- usually a refactored signature with a stale call site.
local function greet(name: string): ()
	print(name)
end

greet("rime", "extra")
--~ ERROR[R0304]: call to `greet` has 2 arguments, but `greet` takes 1

return table.freeze({ greet = greet })
