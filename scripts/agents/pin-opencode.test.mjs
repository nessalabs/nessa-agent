import assert from "node:assert/strict"
import { execFileSync } from "node:child_process"
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
import test from "node:test"
import { createHash } from "node:crypto"
import {
  EXECUTABLE,
  PLATFORMS,
  agreesWithRegistry,
  executableDigest,
  sameBinaryUnderDifferentClaims,
} from "./pin-opencode.mjs"

/**
 * Bytes that begin the way a program does, which pinning now requires.
 *
 * ELF, because these fixtures stand in for the Linux packages and one magic
 * number is enough to be the thing rather than merely the right length.
 */
function program(tail = "") {
  return Buffer.concat([Buffer.from([0x7f, 0x45, 0x4c, 0x46]), Buffer.from(tail)])
}

/** What the executable entry's bytes hash to, which is now what pinning returns. */
function sha256(contents) {
  return createHash("sha256").update(contents).digest("hex")
}

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
    writeFileSync(join(contents, "package/bin/opencode"), program("binary"))
  })
  assert.equal(executableDigest(tarball), sha256(program("binary")))
})

test("a package holding it as a symbolic link is not", (t) => {
  // A link lists under its own name and carries no data, so pinning one would
  // write a pin that installs an empty file.
  const tarball = archive(t, (contents) => {
    writeFileSync(join(contents, "package/bin/real"), "binary")
    symlinkSync("real", join(contents, "package/bin/opencode"))
  })
  assert.equal(executableDigest(tarball), null)
})

test("a package holding a directory of that name is not", (t) => {
  const tarball = archive(t, (contents) => {
    mkdirSync(join(contents, "package/bin/opencode"))
    writeFileSync(join(contents, "package/bin/opencode/inner"), "binary")
  })
  assert.equal(executableDigest(tarball), null)
})

test("a package that does not hold it at all is not", (t) => {
  const tarball = archive(t, (contents) => {
    writeFileSync(join(contents, "package/bin/somethingelse"), "binary")
  })
  assert.equal(executableDigest(tarball), null)
})

test("a name that merely ends in the executable's is not it", (t) => {
  const tarball = archive(t, (contents) => {
    mkdirSync(dirname(join(contents, "package/bin/extra/bin/opencode")), {
      recursive: true,
    })
    writeFileSync(join(contents, "package/bin/extra/bin/opencode"), "binary")
  })
  assert.equal(executableDigest(tarball), null)
})

test("a package holding it as an empty file is not", (t) => {
  // The installer refuses a zero-length entry, so a release whose executable is
  // a placeholder would pin cleanly here and then fail for every user on every
  // platform. The mode says "regular file" and says nothing about the size.
  const tarball = archive(t, (contents) => {
    writeFileSync(join(contents, "package/bin/opencode"), "")
  })
  assert.equal(executableDigest(tarball), null)
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
  writeFileSync(join(contents, "package/bin/opencode"), program("binary"))
  const tarball = join(root, "archive.tgz")
  execFileSync("tar", ["-czf", tarball, "-C", contents, "./package"])

  assert.equal(executableDigest(tarball), sha256(program("binary")))
})

test("a package holding a file that is not a program is not pinnable", (t) => {
  // The failure that succeeds. Every one of these archives holds a
  // `package.json`, and an entry naming it would pass every other check here
  // and then be installed and reported as the tested Opencode runtime.
  const tarball = archive(t, (contents) => {
    writeFileSync(
      join(contents, "package/bin/opencode"),
      JSON.stringify({ name: "opencode", bin: { opencode: "./bin/opencode" } }),
    )
  })
  assert.equal(executableDigest(tarball), null)
})

test("a program in any of the shapes these platforms ship is pinnable", (t) => {
  // Mach-O, thin in either byte order and fat. Linux is covered by every other
  // test in this file; these are the ones a macOS package would arrive as, and
  // refusing one of them would refuse a real release.
  for (const magic of [
    [0xcf, 0xfa, 0xed, 0xfe],
    [0xfe, 0xed, 0xfa, 0xcf],
    [0xca, 0xfe, 0xba, 0xbe],
  ]) {
    const bytes = Buffer.from([...magic, 0x00])
    const tarball = archive(t, (contents) => {
      writeFileSync(join(contents, "package/bin/opencode"), bytes)
    })
    assert.equal(executableDigest(tarball), sha256(bytes), `${magic}`)
  }
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

test("an archive matching what the registry published is accepted", async (t) => {
  const { path, dist } = measured(t, "the archive bytes")
  await assert.doesNotReject(() => agreesWithRegistry(path, dist, "opencode@1.0.0"))
})

test("an archive that is not what the registry published is refused", async (t) => {
  // The only check in the chain that can catch a download arriving wrong.
  // Everything after this verifies that the *same* bytes arrive again, so a bad
  // measurement here would be pinned permanently and verify perfectly forever.
  const { path, dist } = measured(t, "the archive bytes")
  writeFileSync(path, "different bytes")

  await assert.rejects(
    () => agreesWithRegistry(path, dist, "opencode@1.0.0"),
    /does not match the integrity/,
  )
  await assert.rejects(
    () => agreesWithRegistry(path, { shasum: dist.shasum }, "opencode@1.0.0"),
    /does not match the shasum/,
  )
})

test("metadata without checksums is not itself a failure", async (t) => {
  // Both fields are optional in the registry's own schema. A release that omits
  // them is still pinnable — the digest this script measures is what the
  // guarantee rests on, and this is corroboration on top of it.
  const { path } = measured(t, "the archive bytes")
  await assert.doesNotReject(() => agreesWithRegistry(path, {}, "opencode@1.0.0"))
  await assert.doesNotReject(() => agreesWithRegistry(path, undefined, "opencode@1.0.0"))
})

test("an integrity algorithm this script does not know is passed over", async (t) => {
  // `createHash` throws on a name it does not recognise, and a registry that
  // starts publishing a new one should not break pinning outright.
  const { path } = measured(t, "the archive bytes")
  await assert.doesNotReject(() =>
    agreesWithRegistry(path, { integrity: "sha3-512-AAAA" }, "opencode@1.0.0"),
  )
})

/** One measured build, as the generator records it after hashing the entry. */
function build(name, { libc = null, requiresAvx2 = false, digest = "a" } = {}) {
  return { package: name, libc, requiresAvx2, digest }
}

test("two builds claiming different processors over the same binary are refused", (t) => {
  // The case this exists for, and the one 1.18.31 is actually in:
  // `opencode-linux-x64` and `opencode-linux-x64-baseline` hold a byte-identical
  // executable, so one of the two pins states something untrue of the file it
  // points at. Which one cannot be told from the bytes, so pinning stops.
  assert.throws(
    () =>
      sameBinaryUnderDifferentClaims([
        build("opencode-linux-x64", { libc: "gnu", requiresAvx2: true }),
        build("opencode-linux-x64-baseline", { libc: "gnu", requiresAvx2: false }),
      ]),
    /hold the same package\/bin\/opencode.*avx2=false.*avx2=true|hold the same package\/bin\/opencode.*avx2=true.*avx2=false/s,
  )
})

test("two builds claiming different c libraries over the same binary are refused", (t) => {
  // The same fault in its other shape. A glibc build and a musl build cannot be
  // the same file, so if they measure the same one of the two names is wrong —
  // and the machine that gets the wrong one dies in the loader.
  assert.throws(
    () =>
      sameBinaryUnderDifferentClaims([
        build("opencode-linux-x64", { libc: "gnu" }),
        build("opencode-linux-x64-musl", { libc: "musl" }),
      ]),
    /hold the same package\/bin\/opencode/,
  )
})

test("builds that differ, and ones that agree about what they are, pin", (t) => {
  // Nine distinct binaries is the ordinary case. And two entries that measure
  // the same *and* claim the same are not this fault: that is a duplicate
  // platform, which the compiled-in reader refuses on its own terms.
  assert.doesNotThrow(() =>
    sameBinaryUnderDifferentClaims([
      build("opencode-linux-x64", { libc: "gnu", requiresAvx2: true, digest: "a" }),
      build("opencode-linux-x64-baseline", { libc: "gnu", digest: "b" }),
      build("opencode-darwin-arm64", { digest: "c" }),
    ]),
  )
  assert.doesNotThrow(() =>
    sameBinaryUnderDifferentClaims([
      build("opencode-darwin-arm64", { digest: "a" }),
      build("opencode-darwin-arm64-again", { digest: "a" }),
    ]),
  )
})

/**
 * The checked-in pin file is what this generator would write.
 *
 * Everything else in this file tests the generator. Nothing tested the file it
 * produces, and the file is the thing that ships: it is checked in, it is
 * compiled into the server, and it is editable by hand. A pin whose claims no
 * longer match the table they came from is exactly the case the Rust side
 * cannot see either — it reads the file as ground truth.
 *
 * The three fields compared are the three the generator does not measure. The
 * digest, the URL and the version come off the registry and can only be
 * rechecked by fetching, which is what `pin-opencode` is for; the platform,
 * the C library, the AVX2 requirement and the entry path come out of
 * `PLATFORMS` and `EXECUTABLE` and are copied verbatim, so a file that
 * disagrees with them was edited after it was generated.
 *
 * The set is compared too, not just the entries in it. A dropped build leaves
 * every remaining entry agreeing perfectly and a whole class of machine with
 * nothing to install, and an added one is a claim the generator never made.
 */
test("the pinned releases are the ones this table describes", () => {
  const pins = JSON.parse(
    readFileSync(
      join(
        dirname(fileURLToPath(import.meta.url)),
        "../../crates/nessa-server/data/agent-releases.json",
      ),
      "utf8",
    ),
  ).agents.opencode
  const named = (url) => url.split("/")[3]

  assert.deepEqual(
    pins.map((pin) => named(pin.archiveUrl)).sort(),
    PLATFORMS.map((platform) => platform.package).sort(),
    "the pin file and the generator's table describe different builds",
  )

  for (const pin of pins) {
    const platform = PLATFORMS.find((entry) => entry.package === named(pin.archiveUrl))
    assert.deepEqual(
      {
        operatingSystem: pin.operatingSystem,
        architecture: pin.architecture,
        libc: pin.libc,
        requiresAvx2: pin.requiresAvx2,
        executable: pin.executable,
      },
      {
        operatingSystem: platform.operatingSystem,
        architecture: platform.architecture,
        libc: platform.libc,
        requiresAvx2: platform.requiresAvx2,
        executable: EXECUTABLE,
      },
      `${platform.package} is pinned as something other than what the table says it is`,
    )
  }
})
