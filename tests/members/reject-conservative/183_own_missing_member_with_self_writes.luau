--!rime
-- Members set through `self` are known by name, like any other member, so
-- R0306 still reports a call to a name the file never defines. `self` in
-- `main.other:method` is `main.other`, not the export: what it sets is
-- not a member of `main`.
local main = {}

main.other = {}

function main:init()
	self.helper = function()
		return 1
	end
end

function main.other:fill()
	self.inner = function()
		return 2
	end
end

function main.run()
	main:init()
	main.other:fill()
	main.gone()
	--~ ERROR[R0306]: `gone` is not a member of this module's export `main`
	main.inner()
	--~ ERROR[R0306]: `inner` is not a member of this module's export `main`
	return main.helper() + main.other.inner()
end

return main
