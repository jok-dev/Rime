--!rime
-- Same rule through the `return table.freeze(X)` idiom.
local Wrong = {
	a = 1,
}
--~ WARNING[R0310]: module local `Wrong` should be named `main`
return table.freeze(Wrong)
