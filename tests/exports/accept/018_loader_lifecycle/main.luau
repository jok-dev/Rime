--!rime
-- The lifecycle-loader shape: `OnInit` is touched only by the
-- dynamic loader (loader.luau), never by a static consumer. The loader's
-- guarded read + call is a project-wide name read under degrade mode, so
-- there is no R0309 here.
local main = {}

function main.run(): number
	return 1
end

function main.OnInit()
end

return table.freeze(main)
