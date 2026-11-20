--!rime
-- R0309 on function exports: dot and method forms both flag when no
-- consumer touches them; `alive` and `ping` are used by consumer.luau.
local main = {}

function main.alive(): number
	return 1
end

function main.dead()
end
--~ WARNING[R0309]: export `dead` of module `main` is never used

function main:ping()
end

function main:deadMethod()
end
--~ WARNING[R0309]: export `deadMethod` of module `main` is never used

return main
