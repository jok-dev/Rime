--!rime
--~ OPTION[R0312]: on
-- An allow comment above a statement covers every call in that statement,
-- however many lines it is written over.

local Builder = {}
Builder.__index = Builder

function Builder.start()
	return setmetatable({ parts = {} }, Builder)
end

function Builder:with(part: string)
	table.insert(self.parts, part)
	return self
end

-- rime: allow(module-scope-computation, "three parts, joined once as the module loads")
local built = Builder
	.start()
	:with("first")
	:with("second")
	:with("third")

return built
