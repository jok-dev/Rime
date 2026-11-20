--!rime
-- Dead store across a loop back-edge (R0104).
--
-- WHY REJECTED: valid Luau, but nothing ever reads the fallback: the loop
-- either exits, or the next iteration's `local prefix = ...` re-initializes
-- the binding before any read. `prefix` itself is NOT faint — the print
-- consumes it — so this stays R0104 territory with the loop-carried wording.
-- (The faint variant of this shape — no external read at all — is R0017's:
-- tests/faint/reject-conservative/001_guard_in_loop.)
-- WHAT WOULD FIX IT: use `prefix` after the branch, or delete the fallback.
-- Downgrade with `dead-store = "off"` in rime.toml.
local function echoPrefixes(lines: { string })
	local index = 0
	while index < #lines do
		index += 1
		local prefix = string.match(lines[index], "^%a+")
		print(prefix)
		if not prefix then
			--~ WARNING[R0104]: value assigned to `prefix` is never read
			prefix = "none"
		end
	end
end

return table.freeze({ echoPrefixes = echoPrefixes })
