import assert from "node:assert/strict"
import { execFileSync } from "node:child_process"
import { mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import test from "node:test"
import { createHash } from "node:crypto"
import { agreesWithRegistry, containsExecutable } from "./pin-opencode.mjs"

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

test("a package holding it as an empty file is not", (t) => {
  // The installer refuses a zero-length entry, so a release whose executable is
  // a placeholder would pin cleanly here and then fail for every user on every
  // platform. The mode says "regular file" and says nothing about the size.
  const tarball = archive(t, (contents) => {
    writeFileSync(join(contents, "package/bin/opencode"), "")
  })
  assert.equal(containsExecutable(tarball), false)
})

test("a package whose entries are written with a leading ./ is pinnable", (t) => {
  // The size is measured by extracting the entry, so the name asked for has to
  // be the one the archive actually stores. An archive written as `./package`
  // lists its entries that way, and asking tar for `package/bin/opencode`
  // instead could come back with nothing.
  const root = mkdtempSync(join(tmpdir(), "nessa-pin-"))
  t.after(() => rmSync(root, { recursive: true, force: true }))
  const contents = join(root, "contents")
  mkdirSync(join(contents, "package/bin"), { recursive: true })
  writeFileSync(join(contents, "package/bin/opencode"), "binary")
  const tarball = join(root, "archive.tgz")
  execFileSync("tar", ["-czf", tarball, "-C", contents, "./package"])

  assert.equal(containsExecutable(tarball), true)
})

/** An archive on disk, with npm's own checksums for exactly those bytes. */
function measured(t, body) {
  const root = mkdtempSync(join(tmpdir(), "nessa-pin-"))
  t.after(() => rmSync(root, { recursive: true, force: true }))
  const path = join(root, "archive.tgz")
  writeFileSync(path, body)
  return {
    path,
    dist: {
      integrity: `sha512-${createHash("sha512").update(body).digest("base64")}`,
      shasum: createHash("sha1").update(body).digest("hex"),
    },
  }
}

test("an archive matching what the registry published is accepted", (t) => {
  const { path, dist } = measured(t, "the archive bytes")
  assert.doesNotThrow(() => agreesWithRegistry(path, dist, "opencode@1.0.0"))
})

test("an archive that is not what the registry published is refused", (t) => {
  // The only check in the chain that can catch a download arriving wrong.
  // Everything after this verifies that the *same* bytes arrive again, so a bad
  // measurement here would be pinned permanently and verify perfectly forever.
  const { path, dist } = measured(t, "the archive bytes")
  writeFileSync(path, "different bytes")

  assert.throws(
    () => agreesWithRegistry(path, dist, "opencode@1.0.0"),
    /does not match the integrity/,
  )
  assert.throws(
    () => agreesWithRegistry(path, { shasum: dist.shasum }, "opencode@1.0.0"),
    /does not match the shasum/,
  )
})

test("metadata without checksums is not itself a failure", (t) => {
  // Both fields are optional in the registry's own schema. A release that omits
  // them is still pinnable — the digest this script measures is what the
  // guarantee rests on, and this is corroboration on top of it.
  const { path } = measured(t, "the archive bytes")
  assert.doesNotThrow(() => agreesWithRegistry(path, {}, "opencode@1.0.0"))
  assert.doesNotThrow(() => agreesWithRegistry(path, undefined, "opencode@1.0.0"))
})

test("an integrity algorithm this script does not know is passed over", (t) => {
  // `createHash` throws on a name it does not recognise, and a registry that
  // starts publishing a new one should not break pinning outright.
  const { path } = measured(t, "the archive bytes")
  assert.doesNotThrow(() =>
    agreesWithRegistry(path, { integrity: "sha3-512-AAAA" }, "opencode@1.0.0"),
  )
})
