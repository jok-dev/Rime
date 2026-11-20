--!rime
-- Case 12 (plan §9), and the open question in plan §7.2.
--
-- WHY REJECTED: safe as written, but Rime cannot prove `config` is unmodified
-- between the check and the use — tables are shared and mutable.
-- WHAT WOULD FIX IT: bind to a local first (`local retries = config.retries`).
--
-- If §7.2 resolves toward limited field narrowing, this case MOVES to accept/.
local function retriesFor(config: { retries: number? }): number
	if config.retries then
		return config.retries + 1
		--~ ERROR[R0021]: cannot narrow a table field
	end
	return 1
end

return table.freeze({ retriesFor = retriesFor })
