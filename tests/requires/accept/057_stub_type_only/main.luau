--!rime
--!no-exec
-- A `false and require(...)` stub used ONLY in type position: the variable
-- is never assigned and never read at runtime — it exists so `type X =
-- Var.Something` can import a type without a runtime (possibly cyclic)
-- require. The idiom is common in large codebases. Type
-- positions are invisible to HIR, so the R0506 gate is RUNTIME reads:
-- none here, no diagnostic.
local Camera = false and require(script.Parent.dep)

type Grant = Camera.Grant

local function noop(): Grant?
	return nil
end

return table.freeze({ noop = noop })
