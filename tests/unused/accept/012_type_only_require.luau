--!rime
--!no-exec
-- (static-only: the requires resolve nowhere at runtime, so the mlua
-- differential oracle cannot execute this case.)
-- Top-level requires read ONLY in type positions are still uses: HIR drops
-- types, so lowering collects type-referenced names (module.type_refs) and
-- the unused-* rules count them as reads. Each require below appears in
-- exactly one kind of type position and nowhere else.
local Types = require(script.Parent.Types) -- `type Car = Types.Car`
local Config = require(script.Config) -- param annotation
local Result = require(script.Parent.Parent.Shared.Result) -- return annotation
local Enums = require(game:GetService("ReplicatedStorage").Enums) -- `::` assertion

type Car = Types.Car
export type Ride = { car: Car }

local function describe(config: Config.CarConfig): Result.Outcome
	local kind = config.kind :: Enums.Kind
	return { ok = true, kind = kind }
end

return table.freeze({ describe = describe })
