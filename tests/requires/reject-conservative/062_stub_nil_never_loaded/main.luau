--!rime
-- The R0506 shape with a `nil` guard: path resolves, variable never
-- assigned, read at runtime. The message must say the variable is always
-- `nil` — a nil-guarded stub evaluates to nil, not false, and the
-- diagnostic may not misquote the program.
local Economy = nil and require(script.Parent.dep)
--~ ERROR[R0506]: never loaded

local function balance()
	if Economy then
		return Economy.balance("sam")
	end
	return 0
end

return table.freeze({ balance = balance })
