--!rime
--~ OPTION[R0312]: on
-- A call at a module's top level runs when the module is first required.
-- Wherever the call sits, it is one report.

local Catalog = {}

local function describe(value: number): string
	return "at " .. value
end

local started = os.clock()
--~ WARNING[R0312]: `os.clock` is called at module scope

Catalog.Stamp = describe(started)
--~ WARNING[R0312]: `describe` is called at module scope

Catalog.Limits = {
	low = describe(2.5),
	--~ WARNING[R0312]: `describe` is called at module scope
}

print("catalog loaded")
--~ WARNING[R0312]: `print` is called at module scope

return Catalog
