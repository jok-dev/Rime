-- rime: allow-file(circular-require, "corpus: main.luau carries the expectation")
local Main = require(script.Parent.main)

local function y()
	return Main.hello()
end

return table.freeze({ x = 1, y = y })
