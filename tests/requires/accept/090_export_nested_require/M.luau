--!rime
--!no-exec
-- `NS.luau` re-exports its own `script` instance; requiring it and then
-- stepping `:FindFirstChild("Inner")` walks into NS's own file tree on
-- disk — `NS/Inner.luau` lives in a subdirectory the corpus harness's
-- top-level `read_dir` never adds to the session's file set, but the
-- evaluator's instance-chain walk resolves purely through the resolver's
-- own disk check (`resolve_loc`), never needing NS/Inner's HIR at all.
local M = {}
M.Sub = require(script.Parent.NS):FindFirstChild("Inner")
return M
