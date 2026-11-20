--!rime
-- Case 19 (plan §9, §7.6): a `Readonly<T>` value passed where a mutable
-- table is expected. Same rule as case 3's Frozen version, same code —
-- read-only is not assignable to mutable, or the guarantee leaks the first
-- time a result reaches a helper that writes.
--
-- NOT IMPLEMENTED YET (R0043 needs Readonly<T> in the type system and
-- signature checking): stays red as spec.
type Readonly<T> = T -- shim alias (plan §2), so plain Luau accepts the file; Rime special-cases the name

local function bump(counter: { calls: number }): ()
	counter.calls += 1
end

local function track(stats: Readonly<{ calls: number }>): ()
	bump(stats)
	--~ ERROR[R0043]: read-only table passed to a parameter that expects a mutable table
end

return table.freeze({ track = track })
