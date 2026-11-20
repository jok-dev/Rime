--!rime
-- `b` has no static read anywhere, but the consumer indexes the module
-- with a computed key — every member is possibly used. Silence.
local main = {
	a = 1,
	b = 2,
}
return table.freeze(main)
