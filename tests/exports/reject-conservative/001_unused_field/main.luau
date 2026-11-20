--!rime
-- R0309: `dead` is exported but no consumer reads it, and this module
-- never reads it itself. `used` is read by consumer.luau.
local main = {
	used = 1,
	dead = 2,
}
--~ WARNING[R0309]: export `dead` of module `main` is never used
return table.freeze(main)
