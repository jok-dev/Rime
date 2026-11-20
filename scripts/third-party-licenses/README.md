# License texts for packages that ship none

`scripts/third-party-notices.py` copies the license files out of the source of
every package rime is built from. The packages below publish no license file
in their crates.io package, so their text is kept here, in a folder named
after the package.

| Package | File | Copied from |
|---|---|---|
| `full_moon` | `LICENSE.md` | <https://github.com/Kampfkarren/full-moon/blob/83b6f2957ed63132abcb270dd6f75e685273c02b/LICENSE.md>, the commit `full_moon` 3.0.0 was published from |
| `full_moon_derive` | `LICENSE.md` | The same file. The package comes from the same repository. |
| `mlua-sys` | `LICENSE` | <https://github.com/mlua-rs/mlua/blob/31efd0cf727c3aee0b733a8dedac024589e1700c/LICENSE>, the commit `mlua-sys` 0.6.8 was published from |

The script fails when a package it needs ships no license file and has no
folder here: copy the license file from the package's repository into a new
folder. It warns when a package here has started to ship its own file: delete
the folder then.
