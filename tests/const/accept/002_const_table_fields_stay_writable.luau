--!rime
--!no-exec
-- `const` fixes the BINDING, not the value: the table a const names is an
-- ordinary mutable table, so field writes, index writes and compound
-- writes through it are legal Luau and must not be reported as a
-- reassignment. Only `registry = ...` itself would be.
--
-- Static-only: the vendored Luau (0.663) predates `const`.
const registry = {}
const counters = { hits = 0 }

const function record(name: string, value: number)
	registry[name] = value
	counters.hits += 1
end

record("a", 1)

return table.freeze({ registry = registry, counters = counters, record = record })
