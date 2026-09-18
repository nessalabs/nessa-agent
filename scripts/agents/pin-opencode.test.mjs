import assert from "node:assert/strict"
import { execFileSync } from "node:child_process"
import { mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import test from "node:test"
import { containsExecutable } from "./pin-opencode.mjs"

/** A gzip tar built around one entry, laid out the way the package is. */
function archive(t, build) {
  const root = mkdtempSync(join(tmpdir(), "nessa-pin-"))
  t.after(() => rmSync(root, { recursive: true, force: true }))
  const contents = join(root, "contents")
  mkdirSync(join(contents, "package/bin"), { recursive: true })
  build(contents)
  const tarball = join(root, "archive.tgz")
  execFileSync("tar", ["-czf", tarball, "-C", contents, "package"])
  return tarball
}

test("a package holding the executable as a file is pinnable", (t) => {
  const tarball = archive(t, (contents) => {
    writeFileSync(join(contents, "package/bin/opencode"), "binary")
  })
  assert.equal(containsExecutable(tarball), true)
})

test("a package holding it as a symbolic link is not", (t) => {
  // A link lists under its own name and carries no data, so pinning one would
  // write a pin that installs an empty file.
  const tarball = archive(t, (contents) => {
    writeFileSync(join(contents, "package/bin/real"), "binary")
    symlinkSync("real", join(contents, "package/bin/opencode"))
  })
  assert.equal(containsExecutable(tarball), false)
})

test("a package holding a directory of that name is not", (t) => {
  const tarball = archive(t, (contents) => {
    mkdirSync(join(contents, "package/bin/opencode"))
    writeFileSync(join(contents, "package/bin/opencode/inner"), "binary")
  })
  assert.equal(containsExecutable(tarball), false)
})

test("a package that does not hold it at all is not", (t) => {
  const tarball = archive(t, (contents) => {
    writeFileSync(join(contents, "package/bin/somethingelse"), "binary")
  })
  assert.equal(containsExecutable(tarball), false)
})

test("a name that merely ends in the executable's is not it", (t) => {
  const tarball = archive(t, (contents) => {
    mkdirSync(dirname(join(contents, "package/bin/extra/bin/opencode")), {
      recursive: true,
    })
    writeFileSync(join(contents, "package/bin/extra/bin/opencode"), "binary")
  })
  assert.equal(containsExecutable(tarball), false)
})
