--!rime
-- No file requires this module — no consumers means no evidence, and
-- entry points / loader-discovered modules never show a require.
-- Silence (a dead-module finding is future work, not R0309).
local main = {
	unused = 1,
}
return table.freeze(main)
