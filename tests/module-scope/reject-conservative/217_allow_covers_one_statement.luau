--!rime
--~ OPTION[R0312]: on
-- The allow comment above a statement over several lines stops where the
-- statement stops: the next one is reported as usual.

local Builder = {}

function Builder.start()
	return Builder
end

function Builder:with(_part: string)
	return self
end

-- rime: allow(module-scope-computation, "joined once as the module loads")
local first = Builder
	.start()
	:with("first")

local second = Builder
	.start()
	--~ WARNING[R0312]: `Builder.start` is called at module scope
	:with("second")
	--~ WARNING[R0312]: `Builder.start():with` is called at module scope

return { first, second }
