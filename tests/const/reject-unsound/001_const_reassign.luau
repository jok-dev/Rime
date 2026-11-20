--!rime
-- Luau refuses to COMPILE a file that assigns to a `const` binding
-- ("Variable 'LIMIT' is constant and may not be reassigned" — a parse
-- error, so the whole script fails to load, not just this line). Every
-- form of assignment counts, each occurrence is its own diagnostic:
--   * a plain assignment;
--   * a compound assignment;
--   * one target of a multiple assignment (only the const one is flagged);
--   * an assignment from inside a closure (the const is an upvalue there);
--   * `function name() end`, which is sugar for `name = function() end`;
--   * an assignment to a `const function`'s own name.
const LIMIT = 10
const function handler(): number
	return LIMIT
end

local other = 0
print(other, handler())

LIMIT = 20
--~ ERROR[R0043]: cannot assign to `LIMIT`, it is declared `const`

LIMIT += 1
--~ ERROR[R0043]: cannot assign to `LIMIT`, it is declared `const`

other, LIMIT = 1, 2
--~ ERROR[R0043]: cannot assign to `LIMIT`, it is declared `const`

local function bump(): number
	LIMIT = LIMIT + 1
	--~ ERROR[R0043]: cannot assign to `LIMIT`, it is declared `const`
	return LIMIT
end

function handler(): number
--~ ERROR[R0043]: cannot assign to `handler`, it is declared `const`
	return 0
end
print(handler())

handler = bump
--~ ERROR[R0043]: cannot assign to `handler`, it is declared `const`

return table.freeze({ other = other, handler = handler, bump = bump })
