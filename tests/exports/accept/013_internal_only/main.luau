--!rime
-- `helper` is read only inside this module (by `public`). Internal use
-- counts as use — "should be local" is a future, separate finding.
local main = {}

function main.helper(): number
	return 5
end

function main.public(): number
	return main.helper() + 1
end

return main
