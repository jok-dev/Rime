--!rime
--~ OPTION[R0312]: allow = *.new, Units.*
-- A project's own list, on top of the built-in ones. `*` stands for any
-- run of characters, so one pattern covers a whole module (`Units.*`) or
-- a whole shape (`*.new`).

local Units = {}

function Units.meters(studs: number): number
	return studs * 0.28
end

local Point = {}
Point.__index = Point

function Point.new(x: number, y: number)
	return setmetatable({ x = x, y = y }, Point)
end

local ORIGIN = Point.new(0, 0)
local REACH = Units.meters(40)

return table.freeze({
	Origin = ORIGIN,
	Reach = REACH,
})
