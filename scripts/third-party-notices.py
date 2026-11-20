#!/usr/bin/env python3
"""Writes the third-party notices file that a release publishes.

A release publishes bare executables. They contain code from the packages in
Cargo.lock, and most of those licenses ask that their text goes with every
copy. This script puts those texts into one file to publish beside the
executables.

    scripts/third-party-notices.py --target <triple>... --output <file>

It asks `cargo metadata --locked` which packages `rime-cli` is built from on
each target, and copies every license file out of each package's source. A
package that ships no license file needs one under
scripts/third-party-licenses/<package name>/. The script fails when a package
ends up without a text, so a release cannot go out with one missing.

The executables also contain files that were taken from other projects and
are kept in this repository. Each such folder has a NOTICE file beside the
files, listed in IN_TREE_NOTICES below; the script copies it into the output
and fails when one is missing. The header of the output says where the
source of all of this is: the repository in Cargo.toml, at the tag of the
version.

Needs Python 3.8, cargo and git. Nothing else.
"""
import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FALLBACK_DIR = ROOT / "scripts" / "third-party-licenses"
BIN_PACKAGE = "rime-cli"
# Notices for files from other projects that are built into the executables.
# The output must contain every one of them. When such files are added under
# crates/, put a NOTICE file beside them and add it here; take an entry out
# only when the files it covers are no longer built in.
IN_TREE_NOTICES = ("crates/rime-std/std/NOTICE.md",)

LICENSE_FILE = re.compile(r"(?i)^(licen[cs]e|copying|copyright|notice|unlicense|patents)")
LICENSE_DIR = re.compile(r"(?i)^licen[cs]es?$")
# A package can carry test data and examples that have licenses of their own.
# Those are not built into rime.
SKIPPED_DIRS = {"tests", "test", "testdata", "benches", "bench", "examples", "example", "docs", "doc", "fixtures"}
# `license.rs` is code, not a license.
CODE_SUFFIXES = {".rs", ".c", ".h", ".cc", ".cpp", ".hpp", ".py", ".sh", ".js", ".ts", ".toml", ".json", ".yml", ".yaml"}

RULE = "=" * 78


class Failure(Exception):
    pass


def shown(path):
    """A path as an error message names it: relative to the repository if it is inside."""
    try:
        return path.relative_to(ROOT).as_posix()
    except ValueError:
        return str(path)


def warn(message):
    prefix = "::warning::" if os.environ.get("GITHUB_ACTIONS") == "true" else "third-party-notices: warning: "
    print(prefix + message, file=sys.stderr)


def cargo_metadata(target):
    proc = subprocess.run(
        ["cargo", "metadata", "--locked", "--format-version", "1", "--filter-platform", target],
        cwd=ROOT,
        stdout=subprocess.PIPE,
    )
    if proc.returncode != 0:
        raise Failure(f"cargo metadata failed for {target}")
    return json.loads(proc.stdout)


def reachable(meta, kinds):
    """Ids of the packages `rime-cli` reaches through dependencies of these kinds."""
    nodes = {node["id"]: node for node in meta["resolve"]["nodes"]}
    names = {package["id"]: package["name"] for package in meta["packages"]}
    stack = [member for member in meta["workspace_members"] if names[member] == BIN_PACKAGE]
    if not stack:
        raise Failure(f"the workspace has no package named {BIN_PACKAGE}")
    seen = set()
    while stack:
        package_id = stack.pop()
        if package_id in seen:
            continue
        seen.add(package_id)
        for dep in nodes[package_id]["deps"]:
            if {kind["kind"] or "normal" for kind in dep["dep_kinds"]} & kinds:
                stack.append(dep["pkg"])
    return seen


def is_license_file(relative):
    """Whether a file, given by its path inside a package, holds license text."""
    if relative.suffix.lower() in CODE_SUFFIXES:
        return False
    if any(part.lower() in SKIPPED_DIRS for part in relative.parts[:-1]):
        return False
    in_license_dir = any(LICENSE_DIR.match(part) for part in relative.parts[:-1])
    return in_license_dir or bool(LICENSE_FILE.match(relative.name))


def license_files(directory):
    found = []
    for current, dirs, files in os.walk(directory):
        dirs[:] = sorted(d for d in dirs if d.lower() not in SKIPPED_DIRS and d != ".git")
        for name in sorted(files):
            relative = (Path(current) / name).relative_to(directory)
            if is_license_file(relative):
                found.append(relative)
    return found


def read_text(path):
    raw = path.read_bytes()
    try:
        text = raw.decode("utf-8-sig")
    except UnicodeDecodeError:
        text = raw.decode("latin-1")
    lines = [line.rstrip() for line in text.replace("\r\n", "\n").replace("\r", "\n").split("\n")]
    return "\n".join(lines).strip("\n")


def package_texts(package):
    """[(where the text came from, text)] for one package, and whether the
    fallback folder supplied them."""
    directory = Path(package["manifest_path"]).parent
    if not directory.is_dir():
        raise Failure(f"the source of {package['name']} {package['version']} is not at {directory}")
    relatives = license_files(directory)
    declared = package.get("license_file")
    if declared and Path(declared) not in relatives:
        if not (directory / declared).is_file():
            raise Failure(f"{package['name']} {package['version']} names the license file {declared}, which is not in the package")
        relatives.append(Path(declared))

    texts = [(relative.as_posix(), read_text(directory / relative)) for relative in relatives]
    texts = [(origin, text) for origin, text in texts if text]
    fallback = FALLBACK_DIR / package["name"]
    if texts:
        if fallback.is_dir():
            warn(f"{package['name']} {package['version']} ships its own license text now; delete {shown(fallback)}")
        return texts, False
    if fallback.is_dir():
        texts = [
            (f"{path.name}, copied from the package's repository", read_text(path))
            for path in sorted(fallback.iterdir())
            if path.is_file()
        ]
        texts = [(origin, text) for origin, text in texts if text]
    if not texts:
        raise Failure(
            f"{package['name']} {package['version']} ({package.get('license') or 'no license field'}) ships no license text. "
            f"Copy the license file from {package.get('repository') or 'its repository'} "
            f"into {shown(fallback)}/"
        )
    return texts, True


def in_tree_texts():
    """License and notice files committed under crates/, which sit beside
    files that were taken from other projects and are built into rime."""
    proc = subprocess.run(["git", "ls-files", "-z", "--", "crates"], cwd=ROOT, stdout=subprocess.PIPE)
    if proc.returncode != 0:
        raise Failure("git ls-files failed; run this in a git checkout")
    found = []
    for name in sorted(filter(None, proc.stdout.decode("utf-8").split("\0"))):
        relative = Path(name)
        if is_license_file(relative) and (ROOT / relative).is_file():
            text = read_text(ROOT / relative)
            if text:
                found.append((relative.as_posix(), text))
    # A notice that was renamed, emptied, deleted or never committed would
    # drop out of the output without a trace, and the executables would still
    # contain the files it covers.
    have = {path for path, _ in found}
    for path in IN_TREE_NOTICES:
        if path not in have:
            raise Failure(
                f"{path} is missing, empty or not committed. It is the notice for files from another project "
                "that are built into the executables, and a release cannot go out without it"
            )
    return found


def build(targets):
    packages = {}
    workspace = set()
    linked = set()
    used = set()
    for target in targets:
        meta = cargo_metadata(target)
        packages.update((package["id"], package) for package in meta["packages"])
        workspace.update(meta["workspace_members"])
        linked |= reachable(meta, {"normal"})
        used |= reachable(meta, {"normal", "build"})

    rime = next(package for package in packages.values() if package["name"] == BIN_PACKAGE)
    # MPL-2.0 asks that whoever gets an executable is told where the source
    # of the covered files is. The executables are published bare, so this
    # file is where that is said.
    if not rime.get("repository"):
        raise Failure(
            f"{BIN_PACKAGE} has no `repository` in its Cargo.toml, so the notices cannot say where the source is"
        )
    source = f"{rime['repository'].rstrip('/')}, tag v{rime['version']}"
    outside = sorted((packages[i] for i in used - workspace), key=lambda p: (p["name"].lower(), p["version"]))

    # One entry per distinct text. Many packages ship the same Apache-2.0 file.
    numbers = {}
    carried_by = []
    entries = []
    fallbacks = []
    for package in outside:
        label = f"{package['name']} {package['version']}"
        if not package.get("license") and not package.get("license_file"):
            raise Failure(f"{label} declares no license")
        texts, from_fallback = package_texts(package)
        if from_fallback:
            fallbacks.append(label)
        refs = []
        for origin, text in texts:
            number = numbers.setdefault(text, len(numbers) + 1)
            if number > len(carried_by):
                carried_by.append([])
            carried_by[number - 1].append(f"{label}: {origin}")
            refs.append(f"[{number}] {origin}")
        entries.append((package, package["id"] in linked, refs))

    tree = []
    for path, text in in_tree_texts():
        number = numbers.setdefault(text, len(numbers) + 1)
        if number > len(carried_by):
            carried_by.append([])
        carried_by[number - 1].append(f"this repository: {path}")
        tree.append((path, number))

    out = []
    out.append(f"Third-party notices for rime {rime['version']}")
    out.append(RULE)
    out.append("")
    out.append(f"rime is licensed under {rime.get('license') or 'the terms in the file LICENSE'}. The text is in the file LICENSE,")
    out.append("published beside this one.")
    out.append("")
    # The release workflow looks for this line, written exactly like this,
    # before it publishes.
    out.append(f"Source code: {source}")
    out.append("")
    out.append("That is the source of rime itself and of the files from other projects that")
    out.append("are kept in its repository and built into the executables. Those files are")
    out.append("listed after part 1. The source of a package in part 1 is at the address its")
    out.append("entry gives.")
    out.append("")
    out.append("The rime executables are built from the packages listed in part 1. A package")
    out.append("marked as a build dependency runs while the executables are built, and may")
    out.append("supply source that is compiled into them. Part 2 holds every license file")
    out.append("those packages ship and the notices of the files from other projects; a")
    out.append("number in brackets points at a text in part 2.")
    out.append("")
    out.append("The executables also contain the Rust standard library, which is not a")
    out.append("package in Cargo.lock and so is not listed here. It is licensed under")
    out.append("MIT OR Apache-2.0. Its copyright notices, and those of the code it is built")
    out.append("from, are in the file COPYRIGHT-library.html, published beside this one. That")
    out.append("file is taken unchanged from the Rust toolchain the executables are built")
    out.append("with.")
    out.append("")
    out.append("This file is generated by scripts/third-party-notices.py from Cargo.lock,")
    out.append("for the targets " + ", ".join(targets) + ".")
    out.append("")
    out.append("")
    out.append("Part 1: packages")
    out.append(RULE)
    for package, is_linked, refs in entries:
        out.append("")
        out.append(f"{package['name']} {package['version']}")
        out.append(f"    license: {package.get('license') or 'see the license file'}")
        if package.get("repository"):
            out.append(f"    source:  {package['repository']}")
        out.append("    use:     " + ("dependency" if is_linked else "build dependency"))
        for ref in refs:
            out.append(f"    text:    {ref}")
    if tree:
        out.append("")
        out.append("")
        out.append("Files in the rime repository that come from other projects")
        out.append(RULE)
        out.append("")
        out.append("The executables contain these files too. Each notice or license file below")
        out.append("sits in the source tree beside the files it covers, and says what they are")
        out.append("and where they come from. Their source is the rime repository, at the")
        out.append("address and tag given at the top of this file.")
        out.append("")
        for path, number in tree:
            out.append(f"{path}")
            out.append(f"    text:    [{number}]")
    out.append("")
    out.append("")
    out.append("Part 2: license texts and notices")
    out.append(RULE)
    for text, number in sorted(numbers.items(), key=lambda item: item[1]):
        out.append("")
        out.append(RULE)
        out.append(f"[{number}]")
        for carrier in carried_by[number - 1]:
            out.append(f"    {carrier}")
        out.append(RULE)
        out.append("")
        out.append(text)
        out.append("")

    summary = (
        f"{len(entries)} packages ({sum(1 for _, is_linked, _ in entries if is_linked)} dependencies, "
        f"{sum(1 for _, is_linked, _ in entries if not is_linked)} build dependencies), "
        f"{len(tree)} notice or license file{'' if len(tree) == 1 else 's'} from the source tree, "
        f"{len(numbers)} distinct texts"
    )
    return "\n".join(out) + "\n", summary, fallbacks


def main():
    parser = argparse.ArgumentParser(description="Write the third-party notices file for a release.")
    parser.add_argument("--target", action="append", required=True, help="a target triple the release builds; repeat for each")
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()

    try:
        text, summary, fallbacks = build(args.target)
    except Failure as failure:
        prefix = "::error::" if os.environ.get("GITHUB_ACTIONS") == "true" else "third-party-notices: error: "
        print(prefix + str(failure), file=sys.stderr)
        return 1

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_bytes(text.encode("utf-8"))
    print(f"third-party-notices: wrote {args.output}: {summary}")
    for label in fallbacks:
        print(f"third-party-notices: {label} ships no license text; used the copy in {shown(FALLBACK_DIR)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
