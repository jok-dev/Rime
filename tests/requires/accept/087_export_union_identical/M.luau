--!rime
--!no-exec
-- Both branches are live (`math.random()`'s truth is Unknown — never
-- provable, so neither `if` arm prunes) and A.luau/B.luau are
-- byte-identical: the union collapses to `Found(canonical)` rather than
-- `Diverged` (`collapse` step 5).
local M = {}
local function get()
	if math.random() > 0.5 then
		return script.Parent.A
	end
	return script.Parent.B
end
M.Sub = get()
return M
