--!rime
-- Written in one closure, read in another: the read keeps the binding
-- alive, wherever it happens. Counterpart to reject-conservative/010
-- (write-only capture is dead state).
local mode = "idle"

local function set(m: string)
	mode = m
end

return table.freeze({
	set = set,
	get = function(): string
		return mode
	end,
})
