--!rime
-- TWO arms match the `else` fallback: each arm gets its own diagnostic at
-- its own site (CLAUDE.md: one diagnostic per occurrence — a hidden second
-- arm is an unnavigable one). Regression case for the `return` that used
-- to stop after the first report.
local function classify(kind: string, value: number): number
	if kind == "a" then
		return value * 2
	elseif kind == "b" then
		return value
	elseif kind == "c" then
		return value
	else
		return value
	end
end

return classify
--~ ERROR[R0603]: this branch matches the `else` branch
