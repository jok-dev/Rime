--!rime
-- A require whose target is not a static path — banned by dialect (settled:
-- every cross-file edge resolves statically; that is what powers
-- cross-file call conformance and cheap incrementality).
--
-- WHY CONSERVATIVE: the code may work at runtime — Rime cannot follow it,
-- and the resolver's own shape gaps land here too, so this class must never
-- masquerade as a proven error.
-- WHAT WOULD FIX IT: require each candidate statically and pick between the
-- bound results.
--
-- Reported as R0502, marked conservative.
local candidates = { script.Parent.A, script.Parent.B }
local picked = require(candidates[math.random(1, 2)])
--~ ERROR[R0502]: require target is not statically resolvable

return table.freeze({ picked = picked })
