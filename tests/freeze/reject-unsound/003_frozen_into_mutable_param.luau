--!rime
-- Case 3 (plan §9, §7.4): a frozen table passed where a mutable one is
-- expected lets the callee write to it — the write typechecks in the callee
-- and throws at runtime. Definitionally an error, and the most common
-- false-positive-FEELING rejection, so the message has to carry the why.
--
-- NOT IMPLEMENTED YET (R0043 needs Frozen<T> in the type system and
-- signature checking): stays red as spec.
type Frozen<T> = T -- shim alias (plan §2), so plain Luau accepts the file; Rime special-cases the name

local function bump(counter: { calls: number }): ()
	counter.calls += 1
end

local stats: Frozen<{ calls: number }> = table.freeze({ calls = 0 })
bump(stats)
--~ ERROR[R0043]: frozen table passed to a parameter that expects a mutable table

return stats
