--!rime
local base = 10

local function scaled(x)
    return base * x
end

local function outer()
    local mid = scaled(2)
    return function()
        return mid + base
    end
end

repeat
    local done = outer()() > 0
until done
