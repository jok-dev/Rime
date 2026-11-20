--!rime
-- A require whose path Rime fully interprets and whose destination does not
-- exist. If nothing creates that instance, this throws (or hangs forever on
-- a WaitForChild) the moment the module loads.
--
-- WHY CONSERVATIVE: a loader can clone instances in at runtime (wally's
-- cmdr does exactly this) — Rime cannot see that, says so, and fails by
-- default: what cannot be verified at check time fails.
-- Downgrade `broken-require` if the pattern is deliberate.
local Ghost = require(script.Parent.Ghost)
--~ ERROR[R0501]: cannot resolve this require

return table.freeze({ Ghost = Ghost })
