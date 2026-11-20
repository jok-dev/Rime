--!rime
--~ OPTION[R0312]: allow-builtin = luau
-- `allow-builtin` says which built-in lists are in use. With `luau`
-- alone, the Roblox constructors are reported like any other call.

local Layout = {}

Layout.Columns = math.floor(7.5)
Layout.Title = string.rep("-", Layout.Columns)

Layout.Origin = Vector3.new(0, 0, 0)
--~ WARNING[R0312]: `Vector3.new` is called at module scope

Layout.Size = UDim2.fromScale(1, 1)
--~ WARNING[R0312]: `UDim2.fromScale` is called at module scope

return Layout
