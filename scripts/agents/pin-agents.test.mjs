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
  AGENTS,
  EXECUTABLE,
  agreesWithLockfile,
  agreesWithRegistry,
  entryName,
  executableDigest,
  kind,
  lockedDependency,
  ownerMayRun,
  releaseFiles,
  storedName,
  sameBinaryUnderDifferentClaims,
} from "./pin-agents.mjs"

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
  assert.deepEqual(executableDigest(tarball), { digest: sha256(program("binary")) })
})

test("a package holding it as a symbolic link is not", (t) => {
  // A link lists under its own name and carries no data, so pinning one would
  // write a pin that installs an empty file.
  const tarball = archive(t, (contents) => {
    writeFileSync(join(contents, "package/bin/real"), "binary")
    symlinkSync("real", join(contents, "package/bin/opencode"))
  })
  assert.deepEqual(executableDigest(tarball), {
    refusal:
      "holds package/bin/opencode as a symbolic link rather than as a regular file",
  })
})

test("a listing line names its entry whatever kind of entry it is", () => {
  // The readers under `executableDigest`, asked of lines rather than of an
  // archive, because which kind of entry tar *writes* is not this file's to
  // decide. A hard link is the case that showed it: `tar -czf` stores whichever
  // of two linked files it walks first as the regular one, so an archive built
  // to hold one is a coin toss across platforms — it came out the other way up
  // on two CI runners from the way it did locally. The parsing is the claim,
  // and it is deterministic.
  //
  // The GNU lines are `tar (GNU tar) 1.35` output, copied from a real archive.
  // The BSD ones differ only in the columns before the name — a link count
  // where GNU writes `owner/group` — which is the difference these readers
  // exist to be indifferent to.
  for (const [why, line, name, sort] of [
    [
      "a regular file",
      "-rw-r--r-- root/root         5 2026-09-21 00:48 package/bin/opencode",
      "package/bin/opencode",
      null,
    ],
    [
      "a hard link, whose last field is its target",
      "hrw-r--r-- root/root         0 2026-09-21 00:48 package/bin/opencode link to package/bin/real",
      "package/bin/opencode",
      "a hard link",
    ],
    [
      "a hard link as bsd tar prints it",
      "hrw-r--r--  2 root wheel 0 Sep 21 00:48 package/bin/opencode link to package/bin/real",
      "package/bin/opencode",
      "a hard link",
    ],
    [
      "a symbolic link, whose last field is its target",
      "lrwxrwxrwx root/root         0 2026-09-21 00:48 package/bin/opencode -> real",
      "package/bin/opencode",
      "a symbolic link",
    ],
    [
      "a directory, which lists with a trailing slash",
      "drwxr-xr-x root/root         0 2026-09-21 00:48 package/bin/opencode/",
      "package/bin/opencode",
      "a directory",
    ],
    [
      "an archive written as ./package",
      "-rw-r--r-- root/root         5 2026-09-21 00:48 ./package/bin/opencode",
      "package/bin/opencode",
      null,
    ],
    [
      "a name that merely ends in the executable's",
      "-rw-r--r-- root/root         5 2026-09-21 00:48 package/bin/extra/bin/opencode",
      "package/bin/extra/bin/opencode",
      null,
    ],
  ]) {
    assert.equal(entryName(line), name, why)
    if (sort) assert.equal(kind(line), sort, why)
  }
})

test("the name asked of tar is the one the archive stores", () => {
  // Extraction has to use the stored spelling, not the pin's: asking for
  // `package/bin/opencode` in an archive written as `./package` comes back
  // with nothing. The two readers differ by exactly that.
  const stored = "-rw-r--r-- root/root         5 2026-09-21 00:48 ./package/bin/opencode"

  assert.equal(storedName(stored), "./package/bin/opencode")
  assert.equal(entryName(stored), "package/bin/opencode")
})

test("a package holding a directory of that name is not", (t) => {
  const tarball = archive(t, (contents) => {
    mkdirSync(join(contents, "package/bin/opencode"))
    writeFileSync(join(contents, "package/bin/opencode/inner"), "binary")
  })
  assert.deepEqual(executableDigest(tarball), {
    refusal: "holds package/bin/opencode as a directory rather than as a regular file",
  })
})

test("a package that does not hold it at all is not", (t) => {
  const tarball = archive(t, (contents) => {
    writeFileSync(join(contents, "package/bin/somethingelse"), "binary")
  })
  assert.deepEqual(executableDigest(tarball), {
    refusal: "does not hold package/bin/opencode at all",
  })
})

test("a name that merely ends in the executable's is not it", (t) => {
  const tarball = archive(t, (contents) => {
    mkdirSync(dirname(join(contents, "package/bin/extra/bin/opencode")), {
      recursive: true,
    })
    writeFileSync(join(contents, "package/bin/extra/bin/opencode"), "binary")
  })
  assert.deepEqual(executableDigest(tarball), {
    refusal: "does not hold package/bin/opencode at all",
  })
})

test("a package holding it as an empty file is not", (t) => {
  // The installer refuses a zero-length entry, so a release whose executable is
  // a placeholder would pin cleanly here and then fail for every user on every
  // platform. The mode says "regular file" and says nothing about the size.
  const tarball = archive(t, (contents) => {
    writeFileSync(join(contents, "package/bin/opencode"), "")
  })
  assert.deepEqual(executableDigest(tarball), {
    refusal: "holds package/bin/opencode as an empty file",
  })
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

  assert.deepEqual(executableDigest(tarball), { digest: sha256(program("binary")) })
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
  assert.deepEqual(executableDigest(tarball), {
    refusal: "holds package/bin/opencode, whose bytes are not a program",
  })
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
    assert.deepEqual(executableDigest(tarball), { digest: sha256(bytes) }, `${magic}`)
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
 * The fields compared are the ones the generator does not measure. The digest,
 * the size, the URL and the version come off the registry and can only be
 * rechecked by fetching, which is what this script is for; the platform, the C
 * library and the AVX2 requirement are copied verbatim out of the table, so a
 * file that disagrees with them was edited after it was generated.
 *
 * The set is compared too, not just the entries in it. A dropped build leaves
 * every remaining entry agreeing perfectly and a whole class of machine with
 * nothing to install, and an added one is a claim the generator never made.
 */
function pinFile() {
  return JSON.parse(
    readFileSync(
      join(
        dirname(fileURLToPath(import.meta.url)),
        "../../crates/nessa-server/data/agent-releases.json",
      ),
      "utf8",
    ),
  )
}

test("every agent this generator pins is in the pin file, and no others", () => {
  assert.deepEqual(
    Object.keys(pinFile().agents).sort(),
    AGENTS.map((agent) => agent.name).sort(),
    "the pin file and the generator's table describe different agents",
  )
})

test("the pinned releases are the ones this table describes", () => {
  const pins = pinFile().agents

  for (const agent of AGENTS) {
    const releases = pins[agent.name]
    assert.ok(releases?.length, `${agent.name} is not pinned`)
    assert.equal(
      releases.length,
      agent.builds.length,
      `${agent.name} is pinned for a different number of builds than the table names`,
    )

    for (const [at, pin] of releases.entries()) {
      // Compared by position, because the generator writes the builds in the
      // order the table lists them and nothing reorders them afterwards.
      const build = agent.builds[at]
      assert.deepEqual(
        {
          operatingSystem: pin.operatingSystem,
          architecture: pin.architecture,
          libc: pin.libc,
          requiresAvx2: pin.requiresAvx2,
        },
        {
          operatingSystem: build.operatingSystem,
          architecture: build.architecture,
          libc: build.libc,
          requiresAvx2: build.requiresAvx2,
        },
        `${build.package} is pinned as something other than what the table says it is`,
      )
      assert.ok(
        pin.archiveUrl.startsWith("https://"),
        `${build.package} is pinned over something other than https`,
      )
      assert.ok(
        Number.isInteger(pin.archiveBytes) && pin.archiveBytes > 0,
        `${build.package} is pinned without a measured archive length`,
      )
      assert.match(
        pin.archiveDigest,
        /^[0-9a-f]{64}$/,
        `${build.package} is pinned without a sha-256`,
      )

      // The one field the table does state about contents: which file is
      // launched. Its role has to be `launch` and there has to be exactly one.
      const launches = pin.files.filter((file) => file.role === "launch")
      assert.deepEqual(
        launches.map((file) => file.path),
        [agent.launch],
        `${build.package} launches something other than what the table names`,
      )
      for (const file of pin.files)
        assert.ok(
          ["launch", "helper", "document"].includes(file.role),
          `${build.package} pins ${file.path} in a role the server cannot read`,
        )
      // A "launch" agent pins one file; a "package" agent pins more than one,
      // which is the whole reason the distinction exists.
      if (agent.install === "launch")
        assert.equal(
          pin.files.length,
          1,
          `${build.package} pins more than the one program the table says it installs`,
        )
      else
        assert.ok(
          pin.files.length > 1,
          `${build.package} is pinned as a package and installs one file`,
        )
      // Sorted, because that is the order `ReleaseContents` canonicalises to:
      // a file written in another order would read back as different contents
      // and cost a re-download to reach the state already on the disk.
      assert.deepEqual(
        pin.files.map((file) => file.path),
        [...pin.files.map((file) => file.path)].sort(),
        `${build.package} pins its files in an order the server would reorder`,
      )
    }
  }
})

test("claude and codex are pinned at the versions the harness lockfiles install", () => {
  // The pin that matters most and is easiest to get wrong. Their JavaScript
  // ships inside the application and expects the native package `npm ci`
  // resolved beside it, so a pin naming any other version would install a
  // binary the bundled wrapper is not the wrapper for.
  const pins = pinFile().agents
  for (const agent of AGENTS) {
    if (agent.version.from !== "lockfile") continue
    const lockfile = JSON.parse(
      readFileSync(
        join(
          dirname(fileURLToPath(import.meta.url)),
          "../../crates/nessa-sdk/harnesses",
          agent.version.harness,
          "package-lock.json",
        ),
        "utf8",
      ),
    )
    const locked = lockedDependency(lockfile, agent.version.dependency)
    for (const pin of pins[agent.name]) {
      assert.equal(
        pin.version,
        locked.version,
        `${agent.name} is pinned at a version the harness lockfile does not install`,
      )
      assert.equal(
        pin.archiveUrl,
        locked.resolved,
        `${agent.name} is pinned at an archive the harness lockfile does not install`,
      )
    }
  }
})

test("a lockfile that does not hold the dependency is an error rather than a guess", () => {
  assert.throws(
    () => lockedDependency({ packages: {} }, "@openai/codex-darwin-arm64"),
    /is not in the lockfile/,
  )
  assert.deepEqual(
    lockedDependency(
      {
        packages: {
          "node_modules/@openai/codex-darwin-arm64": {
            version: "0.154.0-darwin-arm64",
            resolved:
              "https://registry.npmjs.org/@openai/codex/-/codex-0.154.0-darwin-arm64.tgz",
            integrity: "sha512-abc",
          },
        },
      },
      "@openai/codex-darwin-arm64",
    ),
    {
      version: "0.154.0-darwin-arm64",
      resolved:
        "https://registry.npmjs.org/@openai/codex/-/codex-0.154.0-darwin-arm64.tgz",
      integrity: "sha512-abc",
    },
  )
})

test("a coordinate that resolves elsewhere than the lockfile is refused", () => {
  // The pin and the bundled JavaScript describing two different builds is the
  // one failure unbundling must not introduce.
  const locked = {
    version: "0.154.0-darwin-arm64",
    resolved: "https://registry.npmjs.org/@openai/codex/-/codex-0.154.0-darwin-arm64.tgz",
    integrity: "sha512-right",
  }
  assert.doesNotThrow(() =>
    agreesWithLockfile(
      locked,
      { tarball: locked.resolved, integrity: "sha512-right" },
      "codex",
    ),
  )
  assert.throws(
    () =>
      agreesWithLockfile(
        locked,
        { tarball: "https://registry.npmjs.org/@openai/codex/-/codex-other.tgz" },
        "codex",
      ),
    /different builds/,
  )
  assert.throws(
    () =>
      agreesWithLockfile(
        locked,
        { tarball: locked.resolved, integrity: "sha512-wrong" },
        "codex",
      ),
    /integrity the harness lockfile recorded/,
  )
  // Opencode has no lockfile to agree with, and that is not a failure.
  assert.doesNotThrow(() => agreesWithLockfile(undefined, { tarball: "x" }, "opencode"))
})

/**
 * What a release installs, read off a listing.
 *
 * Lines rather than an archive, for the reason the parsing tests above give:
 * which kind of entry `tar -czf` writes is not this file's to decide, and the
 * claim being tested is the reading.
 */
const CODEX_LISTING = [
  "drwxr-xr-x  0 root   root        0 Oct 26  1985 package/",
  "-rwxr-xr-x  0 root   root  2226552 Oct 26  1985 package/vendor/aarch64-apple-darwin/bin/codex",
  "-rwxr-xr-x  0 root   root     4030 Oct 26  1985 package/vendor/aarch64-apple-darwin/codex-path/rg",
  "-rw-r--r--  0 root   root      517 Oct 26  1985 package/package.json",
].join("\n")

test("a package pins every regular file, with the roles the listing shows", () => {
  assert.deepEqual(
    releaseFiles(CODEX_LISTING, {
      launch: "package/vendor/aarch64-apple-darwin/bin/codex",
      install: "package",
    }).files,
    [
      { path: "package/package.json", role: "document" },
      { path: "package/vendor/aarch64-apple-darwin/bin/codex", role: "launch" },
      { path: "package/vendor/aarch64-apple-darwin/codex-path/rg", role: "helper" },
    ],
    "a package's files, sorted, with the executable bit deciding helper from document",
  )
})

test("a launch-shaped release pins the one program and nothing else", () => {
  // Opencode's shape. The archive holds metadata too; pinning it would claim
  // it matters.
  assert.deepEqual(
    releaseFiles(
      [
        "-rwxr-xr-x  0 root   root  4600961 Oct 26  1985 package/bin/opencode",
        "-rw-r--r--  0 root   root      140 Oct 26  1985 package/package.json",
      ].join("\n"),
      { launch: EXECUTABLE, install: "launch" },
    ).files,
    [{ path: EXECUTABLE, role: "launch" }],
  )
})

test("a package holding a link is refused rather than pinned around", () => {
  // The installer refuses a link entry outright, so pinning one would fail for
  // every user rather than here, where a maintainer can still read the
  // packaging change that introduced it.
  const listing = [
    "-rwxr-xr-x  0 root   root  2226552 Oct 26  1985 package/vendor/aarch64-apple-darwin/bin/codex",
    "lrwxrwxrwx  0 root   root        0 Oct 26  1985 package/vendor/rg -> /usr/bin/rg",
  ].join("\n")
  assert.deepEqual(
    releaseFiles(listing, {
      launch: "package/vendor/aarch64-apple-darwin/bin/codex",
      install: "package",
    }),
    {
      refusal: "holds package/vendor/rg as a symbolic link rather than as a regular file",
    },
  )
})

test("a program the archive does not mark as one is refused", () => {
  // The listing is the only thing that says which files are programs, and the
  // installer takes the role from the pin rather than from the archive — so a
  // release that shipped its binary unexecutable would install something that
  // cannot start, with nothing downstream to notice.
  assert.deepEqual(
    releaseFiles("-rw-r--r--  0 root   root  4600961 Oct 26  1985 package/bin/opencode", {
      launch: EXECUTABLE,
      install: "launch",
    }),
    { refusal: "holds package/bin/opencode, which is not marked as a program" },
  )
  assert.ok(ownerMayRun("-rwxr-xr-x  0 root   root  1 Oct 26  1985 package/bin/opencode"))
  assert.ok(
    !ownerMayRun("-rw-r--r--  0 root   root  1 Oct 26  1985 package/package.json"),
  )
})

test("a package that does not hold its program at all is refused", () => {
  assert.deepEqual(
    releaseFiles("-rw-r--r--  0 root   root  517 Oct 26  1985 package/package.json", {
      launch: "package/vendor/bin/codex",
      install: "package",
    }),
    { refusal: "does not hold package/vendor/bin/codex at all" },
  )
})
