--!rime
-- Only `dead` is unused: `used` is read by the direct consumer and
-- `viaLinker` is read through the linker stub — usage attribution must
-- chase `return require(...)` linkers to the terminal module.
local main = {
	used = 1,
	viaLinker = 2,
	dead = 3,
}
--~ WARNING[R0309]: export `dead` of module `main` is never used
return table.freeze(main)
