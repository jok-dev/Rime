--!rime
-- A parameter or an enclosing function's local that shares its name with a
-- module-level function or table is a different binding. A call through it
-- must not be checked against the module-level signature: the callee is
-- whatever the caller passed in.
local function double(n: number): number
	return n * 2
end

local Shop = {}
function Shop.price(item: string): number
	return #item
end
function Shop:restock(count: number): number
	return count
end

type Pricing = {
	price: (item: string, discount: number) -> number,
	restock: (count: number) -> number,
}

-- a parameter named like the module-level function
local function apply(double: (number, number) -> number): number
	return double(1, 2)
end

-- the same parameter, called from a closure
local function deferred(double: (number, number) -> number): () -> number
	return function(): number
		return double(3, 4)
	end
end

-- an enclosing function's local, called from a closure
local function viaLocal(): () -> number
	local double = math.max
	return function(): number
		return double(5, 6)
	end
end

-- a parameter named like the module-level table: `.` and `:` both differ
local function withShop(Shop: Pricing): number
	return Shop.price("ab", 2) + Shop.restock(3)
end

-- the same parameter, called from a closure
local function deferredShop(Shop: Pricing): () -> number
	return function(): number
		return Shop.price("ab", 2) + Shop.restock(3)
	end
end

-- a method body: the parameter still hides the module-level function
function Shop:quote(double: (number, number) -> number): number
	return double(7, 8)
end

-- `until` sees the locals its body declared
local function untilSeesBody(): number
	local tries = 0
	repeat
		local double = math.max
		tries += 1
	until double(tries, 0) > 1
	return tries
end

local function add(a: number, b: number): number
	return a + b
end

local other: Pricing = {
	price = function(item: string, discount: number): number
		return #item - discount
	end,
	restock = function(count: number): number
		return count + 1
	end,
}

return table.freeze({
	module = double(2) + Shop.price("abc") + Shop:restock(1),
	apply = apply(add),
	deferred = deferred(add)(),
	viaLocal = viaLocal()(),
	withShop = withShop(other),
	deferredShop = deferredShop(other)(),
	quote = Shop:quote(add),
	untilSeesBody = untilSeesBody(),
})
