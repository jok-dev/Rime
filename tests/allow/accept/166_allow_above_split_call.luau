--!rime
-- Inline allow above a call written over several lines. R0304 points at the
-- first extra argument, which sits lines below where the call starts. The
-- directive may go above the line the call starts on (or trail on it), the
-- same place it goes for a call written on one line.
local function describe(name: string): string
	return "item " .. name
end

local Shop = {}

function Shop.buy(itemId: string): string
	return describe(itemId)
end

-- rime: allow(call-extra-args, "old callers still pass the amount")
local first = describe(
	"sword",
	2
)

-- rime: allow(R0304, "old callers still pass the buyer")
local second = Shop.buy(
	"shield",
	"buyer"
)

local third = Shop.buy( -- rime: allow(call-extra-args, "trailing form, same line as the call")
	"bow",
	"buyer"
)

return table.freeze({ first = first, second = second, third = third })