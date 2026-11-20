--!rime
-- Case 7 (plan §9): table.isfrozen is the queryable predicate — inside the
-- guard, `t` is known Frozen even though it arrived as a plain mutable
-- parameter, so the write must be rejected. (The accept half of this case —
-- isfrozen-guarded narrowing surviving a yield — lands with the yield rules.)
--
-- NOT IMPLEMENTED YET (isfrozen guard narrowing needs the type system;
-- parameter freezing is signature territory either way): stays red as spec.
local function touch(t: { n: number }): ()
	if table.isfrozen(t) then
		t.n = 1
		--~ ERROR[R0041]: cannot write to a frozen table
	end
end

return table.freeze({ touch = touch })
