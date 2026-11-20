--!rime
-- Same shape as 087, but A.luau/B.luau differ — the union does not
-- collapse: R0502, note naming both files.
local M = require(script.Parent.M)
local Sub = require(M.Sub)
--~ ERROR[R0502]: require target is not statically resolvable
--~ NOTE: resolves to A.luau / B.luau, which differ
print(Sub)
