--!rime
--~ OPTION[R0312]: scripts = 193_*.luau, *.server.luau
-- The project says which other files are entry scripts. This one returns
-- a value and has no `.server` in its name, and is still left alone.

local started = os.clock()
print(started)

return started
