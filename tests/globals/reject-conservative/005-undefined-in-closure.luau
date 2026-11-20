--!rime
local function run(callback)
    return callback()
end
run(function()
    return speling
end)
--~ ERROR[R0022]: `speling` is not defined
