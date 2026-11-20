--!rime
-- Case 13, REVERSED from plan §9 by a later decision (2026-08-21): an
-- intervening call does not kill narrowing on an UNCAPTURED local — no
-- alias exists for the callee to write through, so `name` is still the
-- checked value at the use. The blunt kill reported this shape over and
-- over, and every report read as a false positive.
local function log(msg: string): ()
	print(msg)
end

local function greet(name: string?): ()
	if name ~= nil then
		log("greeting")
		print(name .. "!")
	end
end

return table.freeze({ greet = greet })
