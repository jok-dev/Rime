--!rime
-- get() has a live branch that returns nil (the guard is
-- `math.random() > 0.5` rather than an undefined global — an unseen global would
-- dirty the dep file via the globals rules; `math.random()` evals Unknown
-- truth, same union behavior, no diagnostics on M.luau). The evaluator
-- cannot prove the guard dead, so `M.Sub` is nil-capable: R0502, not silence.
local M = require(script.Parent.M)
local Sub = require(M.Sub)
--~ ERROR[R0502]: require target is not statically resolvable
--~ NOTE: target can be nil
print(Sub)
