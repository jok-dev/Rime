--!rime
-- Which binding a write hits is decided by scope. Only the write that
-- reaches a `const` is an error:
--   * `tune` copies the const into its own local and writes the local —
--     silent;
--   * `reset` writes its parameter — silent;
--   * the last write is two function bodies away from the const it
--     reaches, and is the one error in this file.
const rate = 5

local function tune(step: number): number
	local current = rate
	current += step
	return current
end

local function reset(limit: number): number
	limit = rate
	return limit
end

local function outer(): () -> ()
	return function()
		rate = 0
		--~ ERROR[R0043]: cannot assign to `rate`, it is declared `const`
	end
end

return table.freeze({ tune = tune, reset = reset, outer = outer })
