--!rime
-- `_hidden` is unused by every consumer, but the `_` prefix marks a
-- deliberate keep — same exemption contract as R0013/14/15.
local main = {
	shown = 1,
}
main._hidden = 2
return table.freeze(main)
