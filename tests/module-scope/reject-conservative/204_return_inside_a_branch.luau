--!rime
--~ OPTION[R0312]: on
-- A module may return from inside a branch; it is a module all the same.

local Legacy = { version = 1 }
local Current = { version = 2 }

local USE_LEGACY = false

local stamp = os.time()
--~ WARNING[R0312]: `os.time` is called at module scope
Current.stamp = stamp

if USE_LEGACY then
	return Legacy
else
	return Current
end
