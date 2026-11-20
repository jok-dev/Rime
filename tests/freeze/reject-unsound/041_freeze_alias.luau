--!rime
-- Case 1 (plan §9): table.freeze returns the SAME table, so `t` is frozen too.
local t = {}
local f = table.freeze(t)
t.x = 1
--~ ERROR[R0041]: cannot write to a frozen table
--~ NOTE: frozen at line 4

return f
