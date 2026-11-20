-- rime: allow-file(undefined-variable, "rule files run inside Rime's rule VM, where `rime` is a global")
-- Every module under a folder declares `Module.Tag = "<string>"` at the top
-- level. Absence check: scan the root block's assignments; if none match,
-- report at the top of the file (span 0..0 = line 1).
--
-- Point `files` at the folder that carries the invariant.
return {
	files = "src/Tagged/**/*.lua",
	check = function(file, ctx)
		for _, a in rime.root_assignments(file.root) do
			local t = a.target
			if t.kind == "Index" and t.name == "Tag" and a.value
				and a.value.kind == "Literal" and a.value.prim == "String" then
				return
			end
		end
		ctx:fail({ start = 0, ["end"] = 0 }, "module does not declare Module.Tag = \"<string>\"")
	end,
}
