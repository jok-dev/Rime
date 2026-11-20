--!rime
--!no-exec
-- rime: allow-file(require-outside-project, "out-of-view subtree under test")
-- `G` resolves OutsideView: its target cannot be an in-view module, so the
-- project stays in STRICT mode and `G.b` is not a name-set read — main's
-- unused `b` is still flagged.
local Mod = require(script.Parent.main)
local G = require(game.ServerStorage.Stuff)

print(Mod.a)
print(G.b)
