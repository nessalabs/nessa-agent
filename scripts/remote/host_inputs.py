"""What a macOS dev host was built from, and whether a tree would build it again (#358).

  host_inputs.py from-depinfo <file.d> <root>  # CI: the files a build read, root-relative
  host_inputs.py key <listing> <inputs>        # here: those files' key in a tree

`from-depinfo` reads Cargo's dep-info for the built binary: every workspace
source, include_* target, and build-script input Cargo tracks for a rebuild.
Registry crates live outside the root and are pinned by Cargo.lock, so they are
left out.

`key` hashes, for one tree, the object id of every listed input together with
every file that decides what Cargo would compile even when no build listed it
(MANIFESTS): a build.rs or rust-toolchain.toml added since is a different build.
The listing is `git ls-tree -r -t -z --format='%(objectname) %(path)'`, so a
directory an input names is keyed by its tree id, which changes when anything
inside it is added or edited.
"""

import hashlib
import os
import sys

MANIFEST_NAMES = {"Cargo.toml", "Cargo.lock", "build.rs", "rust-toolchain", "rust-toolchain.toml"}


def is_manifest(path):
    return os.path.basename(path) in MANIFEST_NAMES or path.startswith(".cargo/")


def from_depinfo(depinfo, root):
    text = open(depinfo, encoding="utf-8").read()
    # The first rule is `target: dep dep ...`; spaces in a path are escaped and a
    # long rule continues across lines ending in a backslash.
    rule = text.split("\n\n", 1)[0].replace("\\\n", " ")
    deps = rule.split(":", 1)[1] if ":" in rule else ""
    root = os.path.normpath(root)
    inputs = set()
    for dep in deps.replace("\\ ", "\0").split():
        path = os.path.normpath(dep.replace("\0", " "))
        if path.startswith(root + os.sep):
            inputs.add(os.path.relpath(path, root))
    return sorted(inputs)


def key(listing, inputs):
    ids = {}
    for entry in open(listing, "rb").read().split(b"\0"):
        if entry:
            object_id, path = entry.decode("utf-8").split(" ", 1)
            ids[path] = object_id
    wanted = {line for line in open(inputs, encoding="utf-8").read().split("\n") if line}
    wanted.update(path for path in ids if is_manifest(path))
    digest = hashlib.sha1()
    for path in sorted(wanted):
        digest.update(f"{path}\0{ids.get(path, 'absent')}\n".encode("utf-8"))
    return digest.hexdigest()


if __name__ == "__main__":
    if sys.argv[1:2] == ["from-depinfo"] and len(sys.argv) == 4:
        print("\n".join(from_depinfo(sys.argv[2], sys.argv[3])))
    elif sys.argv[1:2] == ["key"] and len(sys.argv) == 4:
        print(key(sys.argv[2], sys.argv[3]))
    else:
        sys.exit(__doc__)
