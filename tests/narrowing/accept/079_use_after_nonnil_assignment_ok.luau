--!rime
-- NOT R0202: the only path that indexes `blur` before the check first
-- assigned it a syntactically non-nil value (a table constructor). That
-- index was never unguarded — the value was proven non-nil by the write,
-- the same way R0201 reads such assignments — so the `blur and blur.size`
-- after the join is at most redundant, never late.
local function size(blur): number?
	if not blur then
		blur = { size = 1 }
		blur.size = 2
	end
	local s = blur and blur.size
	return s
end

return table.freeze({ size = size(nil) == 2 and size({ size = 3 }) == 3 })
