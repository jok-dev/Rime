--!rime
-- Case 4 (plan §9): table.clone of a frozen table is a fresh, mutable copy —
-- writing to the clone is the sanctioned way to "modify" frozen data.
local defaults = table.freeze({ volume = 5, muted = false })
local settings = table.clone(defaults)
settings.volume = 7

return table.freeze({ defaults = defaults, settings = settings })
