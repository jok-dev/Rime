--!rime
-- Unused parameter (R0014, decided 2026-08-21).
--
-- WHY REJECTED: valid Luau, but a parameter nobody reads is either dead
-- signature weight or a handler argument the author forgot to use — and a
-- positional parameter often cannot be removed, only marked.
-- WHAT WOULD FIX IT: use it, or prefix with `_` if the position is required.
-- Downgrade with `unused-param = "off"` in rime.toml.
local function greet(name: string, punctuation: string): string
	--~ WARNING[R0014]: parameter `punctuation` is never read
	return "hello, " .. name
end

local handlers = {
	onScore = function(player: string, amount: number): number
		--~ WARNING[R0014]: parameter `player` is never read
		return amount * 2
	end,
}

return table.freeze({ greet = greet, handlers = handlers })
