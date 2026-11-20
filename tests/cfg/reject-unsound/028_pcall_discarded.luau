--!rime
-- Case 28 (plan §9): the ok flag is dropped, so the error is silently swallowed.
local function save(store: any, key: string): ()
	pcall(function()
		store:SetAsync(key, true)
	end)
	--~ ERROR[R0110]: the result of `pcall` is discarded
	--~ NOTE: pcall returns (ok: boolean, ...) — dropping it swallows the error
end

return table.freeze({ save = save })
