-- rime: allow-file(circular-require, "corpus: main.luau carries the expectation")
local Main = require(script.Parent.main)

local function main()
	return Main.dep()
end

return table.freeze({ main = main })
