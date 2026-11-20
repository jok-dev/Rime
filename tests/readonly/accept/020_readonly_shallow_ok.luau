--!rime
-- Case 20 (plan §9, §7.6): `Readonly` is shallow by design — the same
-- shallow-const property as C++. A nested table under a read-only handle
-- stays writable unless its own type says otherwise; the write below
-- targets the inner table, not the handle.
--
-- Deep immutability is spelled differently on purpose: `DeepFrozen` is
-- computed from the type (plan §5), not implied by `Readonly`.
type Readonly<T> = T -- shim alias (plan §2), so plain Luau accepts the file; Rime special-cases the name

local function state(): Readonly<{ inner: { count: number } }>
	return { inner = { count = 0 } }
end

local s = state()
s.inner.count += 1

return table.freeze({ count = s.inner.count })
