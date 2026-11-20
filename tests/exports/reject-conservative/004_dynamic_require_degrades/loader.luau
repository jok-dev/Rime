--!rime
--!no-exec
-- rime: allow-file(dynamic-require, "loader idiom under test")
-- The unattributable require that puts the project in degrade mode. Its
-- member reads (`OnInit`) enter the global name-set; `b` does not.
local target = nil

local function load()
	return require(target)
end

local m = load()
if m.OnInit then
	m.OnInit()
end
