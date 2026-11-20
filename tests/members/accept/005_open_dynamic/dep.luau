--!rime
-- OPEN module: fields assigned through a dynamic key in a loop. The
-- extractor cannot know the member set — membership stays unjudged.
local dep = {}

for _, name in ipairs({ "alpha", "beta" }) do
	dep[name] = function()
		print(name)
	end
end

return table.freeze(dep)
