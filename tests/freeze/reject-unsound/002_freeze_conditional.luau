--!rime
-- Case 2 (plan §9, §7.3): conditional freeze is a must-analysis — frozen only
-- when EVERY path froze. Both branches freeze `t`, so the write below is a
-- guaranteed runtime throw.
local function build(flag: boolean): { n: number }
	local t = { n = 0 }
	if flag then
		table.freeze(t)
	else
		t.n = 2 -- differs from the then-arm so R0603 stays quiet
		table.freeze(t)
	end
	t.n = 1
	--~ ERROR[R0041]: cannot write to a frozen table
	--~ NOTE: frozen at line 8
	return t
end

-- One-sided freeze: `t2` is frozen only when `flag` is true, so the write may
-- or may not throw. The must-analysis does not flag it — catching a
-- maybe-frozen write needs a may-analysis (documented milestone gap, not a
-- decision).
local function partial(flag: boolean): { n: number }
	local t2 = { n = 0 }
	if flag then
		table.freeze(t2)
	end
	t2.n = 1
	return t2
end

return table.freeze({ build = build, partial = partial })
