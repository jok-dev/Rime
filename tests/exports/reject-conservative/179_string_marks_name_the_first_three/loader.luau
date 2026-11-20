--!rime
--!no-exec
-- rime: allow-file(dynamic-require, "loader idiom under test")
-- The unattributable require that puts the project in degrade mode. Its
-- member read (`start`) enters the global name-set.
local target = nil

local function load()
	return require(target)
end

local m = load()
if m.start then
	m.start()
end
