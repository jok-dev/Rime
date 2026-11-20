--!rime
-- Case 27 (plan §9): everything after the `do return end` is dead. A warning
-- rather than an error — Rust errors on this, Luau says nothing at all.
local function greet(name: string): string
	do
		return "hello " .. name
	end
	print("never runs")
	--~ WARNING[R0103]: unreachable code
end

return table.freeze({ greet = greet })
