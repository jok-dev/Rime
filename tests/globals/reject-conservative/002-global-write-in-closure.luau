--!rime
local function setup()
    total = 0
end
function helper()
end
setup()
--~ ERROR[R0023]: global write: `total` is not declared `local`
--~ ERROR[R0023]: global write: `helper` is not declared `local`
