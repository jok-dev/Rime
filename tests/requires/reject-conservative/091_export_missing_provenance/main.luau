--!rime
-- The evaluator interprets M.Sub through M's export and PROVES the target
-- missing (Nope.luau does not exist): R0501, and the diagnostic carries a
-- provenance note — the rendered path came from interpreting exported
-- values, not from a literal chain at this site.
local M = require(script.Parent.M)
local Sub = require(M.Sub)
--~ ERROR[R0501]: cannot resolve this require
--~ NOTE: path interpreted through module-exported values
print(Sub)
