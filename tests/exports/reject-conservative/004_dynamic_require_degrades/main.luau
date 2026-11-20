--!rime
-- A dynamic require no longer disables R0309 — it DEGRADES it: any member
-- name dot-read anywhere in the project counts as used on every module.
-- loader.luau reads `OnInit` (silenced project-wide), but nothing in
-- the project ever reads `b` by name, so `b` is still flagged.
local main = {
	a = 1,
	b = 2,
}
--~ WARNING[R0309]: export `b` of module `main` is never used
return table.freeze(main)
