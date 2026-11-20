--!rime
-- Case 18 (plan §9, §7.6): `pairs` over a `Readonly<T>` value is a read and
-- stays legal — `next`/`pairs` never write. The read-only view restricts
-- writes, not traversal.
type Readonly<T> = T -- shim alias (plan §2), so plain Luau accepts the file; Rime special-cases the name

local function config(): Readonly<{ speed: number, drag: number }>
	return { speed = 16, drag = 2 }
end

local total = 0
for _, v in pairs(config()) do
	total += v
end

return table.freeze({ total = total })
