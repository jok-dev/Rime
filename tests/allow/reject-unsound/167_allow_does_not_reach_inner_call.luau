--!rime
-- An allow above a statement reaches the call that starts on the next line.
-- A call that starts further down has a line of its own to put it on.
--
-- WHY REJECTED: the directive is above `wrap(`. The call with the extra
-- argument is `describe(`, which starts one line later, and nothing vouches
-- for it.
-- WHAT WOULD FIX IT: move the directive to the line above `describe(`, or
-- above the argument itself.
local function describe(name: string): string
	return "item " .. name
end

local function wrap(text: string): string
	return "[" .. text .. "]"
end

-- rime: allow(call-extra-args, "sits one line too high for the inner call")
local label = wrap(
	describe(
		"sword",
		--~ ERROR[R0304]: call to `describe` has 2 arguments, but `describe` takes 1
		2
	)
)

return table.freeze({ label = label })