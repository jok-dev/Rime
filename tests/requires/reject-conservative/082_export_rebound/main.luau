--!rime
-- M.Sub is written once, from a local that gets REBOUND before the write —
-- the evaluator poisons the rebound name (no SSA), so the export's field
-- value is Unknown at the require site. Never guess: R0502, conservative.
local M = require(script.Parent.M)
local Sub = require(M.Sub)
--~ ERROR[R0502]: require target is not statically resolvable
print(Sub)
