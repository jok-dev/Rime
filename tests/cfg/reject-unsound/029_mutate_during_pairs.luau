--!rime
-- Case 29 (plan §9): inserting during traversal is undefined behavior in Lua.
local function tag(set: { [string]: boolean }): ()
	for key in pairs(set) do
		set[key .. "_seen"] = true
		--~ ERROR[R0111]: table modified while being iterated by `pairs`
	end
end

return table.freeze({ tag = tag })
