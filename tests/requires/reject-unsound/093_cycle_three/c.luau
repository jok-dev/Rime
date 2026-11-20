-- rime: allow-file(circular-require, "corpus: main.luau carries the expectation")
local Main = require(script.Parent.main)

local function value()
	return Main.value()
end

return table.freeze({ value = value })
