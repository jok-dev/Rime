--!rime
-- Unused local function (R0015, decided 2026-08-21).
--
-- WHY REJECTED: a `local function` nobody references is dead code; calling
-- ITSELF does not count — a function only its own body mentions is
-- unreachable all the same.
-- WHAT WOULD FIX IT: call it, export it, or delete it.
-- Downgrade with `unused-function = "off"` in rime.toml.
--
-- Both are nested: module top-level functions are exempt in v1.
local function build(): number
	local function helper(): number
		--~ WARNING[R0015]: `helper` is never used
		return 2
	end
	local function countdown(n: number): number
		--~ WARNING[R0015]: `countdown` is never used
		if n <= 0 then
			return 0
		end
		return countdown(n - 1)
	end
	return 1
end

return table.freeze({ build = build })
