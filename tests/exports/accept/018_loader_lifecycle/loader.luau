--!rime
--!no-exec
-- rime: allow-file(dynamic-require, "loader idiom under test")
-- The lifecycle loader: requires dynamically, guard-reads and calls
-- `OnInit` — both name-set reads that silence the member on every
-- module in the project.
local target = nil

local function load()
	return require(target)
end

local m = load()
if m.OnInit then
	m.OnInit()
end
