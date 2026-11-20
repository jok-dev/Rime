--!rime
-- `const` is a CONTEXTUAL keyword: only `const <name>` and
-- `const function` start a declaration. Code that already uses `const` as
-- an ordinary variable keeps its meaning — a local named `const`, an
-- assignment to it, a field write through it, a call of it.
--
-- This one runs under any Luau, old or new, so it is executed.
local const = { n = 1 }
const.n = 2
const = { n = const.n + 1 }

local function id(x: number): number
	return x
end

local function call(const: (number) -> number): number
	return const(4)
end

return table.freeze({ n = const.n, four = call(id) })
