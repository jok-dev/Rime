--!rime
-- `b` is never read statically, but the consumer passes the bare module
-- table onward — an escaped alias can read anything. Silence.
local main = {
	a = 1,
	b = 2,
}
return table.freeze(main)
