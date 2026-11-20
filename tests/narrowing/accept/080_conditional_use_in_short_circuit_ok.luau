--!rime
-- NOT R0202: `response > 1` runs only when `ok` is truthy — it is the
-- right operand of an `and`. On the path where `ok` is false, `response`
-- was never used, so the `response or "none"` below is a legitimate
-- check on that path. A use inside a short-circuit's right operand (or an
-- if-expression arm) is conditional and must not count as "used on every
-- path".
local function describe(ok: boolean, response): string
	if ok and response > 1 then
		return "big"
	end
	return "small: " .. tostring(response or "none")
end

return table.freeze({ big = describe(true, 5), small = describe(false, nil) })
