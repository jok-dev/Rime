--!rime
-- Case 17 (plan §9, §7.6): a `Readonly<T>` return written by the caller.
--
-- Return position is the cheapest place to hand out read-only views —
-- covariance means a mutable table flows into a read-only return with no
-- variance work. The guarantee only holds if the caller's write errors AT
-- THE WRITE SITE; nothing throws at runtime (Readonly is type-level only),
-- which is exactly why the checker must catch it.
--
-- NOT IMPLEMENTED YET (R0044 needs Readonly<T> in the type system): stays
-- red as spec.
type Readonly<T> = T -- shim alias (plan §2), so plain Luau accepts the file; Rime special-cases the name

local defaults = { speed = 16 }

local function config(): Readonly<{ speed: number }>
	return defaults
end

local c = config()
c.speed = 99
--~ ERROR[R0044]: cannot write through a read-only value

return table.freeze({ config = config })
