--!rime
-- Case 8 (plan §9, §5 "deliberate divergence"): legal Luau, error in Rime —
-- the annotation promises a mutable table but the value is frozen, so every
-- later write typechecks in Luau and throws at runtime. The message must
-- point the user at Frozen<{ a: number }>.
--
-- NOT IMPLEMENTED YET (R0044 needs annotation checking against the inferred
-- frozen state): stays red as spec.
local t: { a: number } = table.freeze({ a = 1 })
--~ ERROR[R0044]: the annotation says this table is mutable, but the value is frozen
--~ NOTE: annotate as Frozen<{ a: number }>

return t
